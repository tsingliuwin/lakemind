//! DuckDB access layer.
//!
//! Sub-modules:
//! - [`pathutil`]   – path / identifier hygiene
//! - [`scan`]       – filesystem classifier → [`scan::ScanEntry`]
//! - [`register`]   – file → DuckLake table (small) or zero-copy VIEW (large)
//! - [`schema`]     – `DESCRIBE` + parquet-metadata row count
//! - [`execute`]    – ad-hoc SELECT with a safety row cap
//! - [`lake`]       – DuckLake connection management (load + ATTACH + USE)

pub mod execute;
pub mod lake;
pub mod naming;
pub mod pathutil;
pub mod register;
pub mod scan;
pub mod schema;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SourceKind, StorageKind};
    use crate::state::AppState;

    /// Smoke: the bundled DuckDB builds, opens, and runs SQL.
    #[test]
    fn execute_select_one() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let res = execute::run_query(&conn, "SELECT 1 AS n", Some(100)).unwrap();
        assert_eq!(res.columns, vec!["n".to_string()]);
        assert_eq!(res.rows.len(), 1);
        assert_eq!(res.rows[0][0], serde_json::json!(1));
        assert!(!res.truncated);
    }

    /// Row cap is enforced and the `truncated` flag is raised.
    #[test]
    fn execute_enforces_row_cap() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let res = execute::run_query(&conn, "SELECT * FROM range(1000)", Some(10)).unwrap();
        assert_eq!(res.rows.len(), 10);
        assert!(res.truncated);
    }

    #[test]
    fn test_list_tables() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t1 (id INT);", []).unwrap();
        conn.execute("CREATE VIEW v1 AS SELECT * FROM t1;", []).unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT table_name, 'table' as type FROM duckdb_tables() WHERE schema_name = 'main' AND NOT internal
                 UNION ALL
                 SELECT view_name as table_name, 'view' as type FROM duckdb_views() WHERE schema_name = 'main' AND NOT internal
                 ORDER BY table_name",
            )
            .unwrap();

        struct DbTable {
            name: String,
            kind: String,
        }
        let rows = stmt
            .query_map([], |row| Ok(DbTable { name: row.get(0)?, kind: row.get(1)? }))
            .unwrap();
        let mut db_tables = Vec::new();
        for r in rows {
            db_tables.push(r.unwrap());
        }
        assert_eq!(db_tables.len(), 2);
        assert_eq!(db_tables[0].name, "t1");
        assert_eq!(db_tables[0].kind, "table");
        assert_eq!(db_tables[1].name, "v1");
        assert_eq!(db_tables[1].kind, "view");
    }

    /// A bare trailing semicolon must not break the wrapper subquery.
    #[test]
    fn execute_tolerates_trailing_semicolon() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let res = execute::run_query(&conn, "SELECT 42;", None).unwrap();
        assert_eq!(res.rows[0][0], serde_json::json!(42));
    }

    /// CSV scan → register (materialized table) → query, end to end.
    #[test]
    fn scan_register_csv_and_query() {
        let dir = tempdir();
        let csv = dir.path().join("people.csv");
        std::fs::write(&csv, "id,name\n1,alice\n2,bob\n3,carol\n").unwrap();

        let conn = duckdb::Connection::open_in_memory().unwrap();
        let entries = scan::scan_path(&csv, false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Csv);

        let table = register::register(&conn, &entries[0], StorageKind::Table, None).unwrap();
        assert!(table.columns.iter().any(|c| c.name == "name"), "columns: {:?}", table.columns);
        assert_eq!(table.storage, StorageKind::Table);

        let res =
            execute::run_query(&conn, &format!("SELECT count(*) AS n FROM \"{}\"", table.name), None).unwrap();
        assert_eq!(res.rows[0][0], serde_json::json!(3));
    }

    /// Parquet scan → register (materialized table) → count via parquet_metadata fast path.
    #[test]
    fn scan_register_parquet_and_count() {
        let dir = tempdir();
        let pq = dir.path().join("data.parquet");
        write_parquet(&pq);

        let conn = duckdb::Connection::open_in_memory().unwrap();
        let entries = scan::scan_path(dir.path(), false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Parquet);

        let table = register::register(&conn, &entries[0], StorageKind::Table, None).unwrap();
        assert!(table.row_count_estimate.is_some());
        // write_parquet produced 5 rows; the fast path must report the true count.
        assert_eq!(table.row_count_estimate.unwrap(), 5);
    }

    /// The workspace scan must NOT descend into DuckLake's own output — neither
    /// the hidden `.lake/` store nor a legacy top-level `lake_data/`. Otherwise
    /// every materialized table's parquet gets re-registered with an extra `s_`
    /// prefix on each sync (s_x → s_s_x → …).
    #[test]
    fn scan_prunes_lake_data_dir() {
        let dir = tempdir();
        let ws = dir.path();
        std::fs::write(ws.join("sales.csv"), "id,name\n1,a\n").unwrap();
        // legacy top-level lake_data
        let ld = ws.join("lake_data").join("main").join("s_sales");
        std::fs::create_dir_all(&ld).unwrap();
        write_parquet(&ld.join("data.parquet"));
        // hidden .lake store (current layout) — also must not be scanned
        let hidden = ws.join(".lake").join("lake_data").join("main").join("s_hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        write_parquet(&hidden.join("data.parquet"));

        let entries = scan::scan_path(ws, true);
        let names: Vec<&str> = entries.iter().map(|e| e.view_name.as_str()).collect();
        assert!(names.contains(&"s_sales"), "real csv source must be found: {:?}", names);
        assert!(
            !names.iter().any(|n| n.starts_with("s_s_")),
            "lake_data / .lake parquet must NOT be re-scanned: {:?}",
            names
        );
    }

    /// DuckLake end-to-end acceptance — the key spike for the storage layer:
    ///   * the ducklake extension loads and a lake attaches
    ///   * a materialized TABLE persists across a reconnect
    ///   * a zero-copy VIEW over an external parquet works
    ///   * catalog + data files are created on disk
    ///
    /// Requires network on first run (to `INSTALL ducklake`). If this fails with
    /// a "ducklake" extension error, the bundled DuckDB version is too old and
    /// `duckdb-rs` must be upgraded.
    #[test]
    fn ducklake_materializes_and_persists() {
        let dir = tempdir();
        let ws = dir.path();
        let conn = AppState::open_workspace(ws).expect("open workspace lake");

        // materialized table from a CSV
        let csv = ws.join("people.csv");
        std::fs::write(&csv, "id,name\n1,alice\n2,bob\n").unwrap();
        let entries = scan::scan_path(&csv, false);
        assert_eq!(entries.len(), 1);
        register::register(&conn, &entries[0], StorageKind::Table, None).unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM s_people", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        assert!(ws.join(".lake").join("lake.sqlite").exists(), "catalog file must exist");
        assert!(ws.join(".lake").join("lake_data").is_dir(), "data dir must exist");

        // reconnect: the materialized table must survive (DuckLake is persistent)
        drop(conn);
        let conn2 = AppState::open_workspace(ws).expect("reopen lake");
        let n2: i64 = conn2.query_row("SELECT count(*) FROM s_people", [], |r| r.get(0)).unwrap();
        assert_eq!(n2, 2, "materialized table must persist across reconnect");

        // zero-copy VIEW over an external parquet
        let pq = ws.join("ext.parquet");
        conn2
            .execute(
                &format!(
                    "COPY (SELECT i AS id FROM range(5) t(i)) TO '{}' (FORMAT PARquet)",
                    pq.display()
                ),
                [],
            )
            .unwrap();
        let view_entry = scan::ScanEntry {
            label: "ext".into(),
            view_name: "s_ext".into(),
            kind: SourceKind::Parquet,
            path: pq.to_string_lossy().to_string(),
            scan_path: pq.to_string_lossy().to_string(),
            partition_keys: Vec::new(),
            file_size: u64::MAX,
            mtime: 0,
            sheet: None,
        };
        let vt = register::register(&conn2, &view_entry, StorageKind::View, None).unwrap();
        let vn: i64 = conn2.query_row("SELECT count(*) FROM s_ext", [], |r| r.get(0)).unwrap();
        assert_eq!(vn, 5);
        assert_eq!(vt.storage, StorageKind::View);
    }

    /// Real-data acceptance against a large sharded parquet lake (the H4 count
    /// bug's trigger shape). `#[ignore]` — only runs where the data exists.
    #[test]
    #[ignore]
    fn acceptance_demomind() {
        let demo = std::path::PathBuf::from(r"E:\rustproject\demomind");
        if !demo.exists() {
            eprintln!("skipped: demomind not present at {}", demo.display());
            return;
        }

        let dir = tempdir();
        let conn = AppState::open_workspace(dir.path()).unwrap();
        let t_scan = std::time::Instant::now();
        let entries = scan::scan_path(&demo, false);
        let scan_ms = t_scan.elapsed().as_millis();
        println!("=== SCAN: {} entries in {} ms", entries.len(), scan_ms);

        for e in &entries {
            let t_reg = std::time::Instant::now();
            // Large sharded lake → zero-copy view
            let table = register::register(&conn, e, StorageKind::View, None).unwrap();
            let truth: i64 = conn
                .query_row(&format!("SELECT count(*) FROM \"{}\"", table.name), [], |r| r.get(0))
                .unwrap();
            println!(
                "=== REGISTER {}: {} cols, estimate={}, truth={}, match={}, register={} ms",
                table.name,
                table.columns.len(),
                table.row_count_estimate.unwrap_or(-1),
                truth,
                table.row_count_estimate == Some(truth),
                t_reg.elapsed().as_millis()
            );
        }
    }

    /// Incremental sync via `register_workspace_sources_inner`: a new file is
    /// registered + mapped; deleting the file drops the table and the mapping.
    /// `#[ignore]` — requires the ducklake extension (network on first run).
    #[test]
    #[ignore]
    fn register_workspace_sources_incremental() {
        let dir = tempdir();
        let ws_path = dir.path().to_string_lossy().to_string();
        let state = AppState::default();

        let csv = dir.path().join("users_test.csv");
        std::fs::write(&csv, "id,name\n1,Alice\n2,Bob\n").unwrap();

        tauri::async_runtime::block_on(async {
            // First sync: ingest + map
            let tables = crate::commands::register_workspace_sources_inner(ws_path.clone(), &state)
                .await
                .unwrap();
            assert_eq!(tables.len(), 1);
            assert_eq!(tables[0].name, "s_users_test");

            // mapping row exists
            let sqlite = crate::db::get_db_conn().unwrap();
            let rec = crate::db::get_source_by_table(&sqlite, &ws_path, "s_users_test").unwrap();
            assert!(rec.is_some(), "source mapping must be recorded");

            // Second sync: cached, no re-ingest
            let again = crate::commands::register_workspace_sources_inner(ws_path.clone(), &state)
                .await
                .unwrap();
            assert_eq!(again.len(), 1);

            // Delete the file → orphan cleanup
            std::fs::remove_file(&csv).unwrap();
            let after = crate::commands::register_workspace_sources_inner(ws_path.clone(), &state)
                .await
                .unwrap();
            assert_eq!(after.len(), 0, "orphan table must be dropped");

            let rec2 = crate::db::get_source_by_table(&sqlite, &ws_path, "s_users_test").unwrap();
            assert!(rec2.is_none(), "orphan mapping must be removed");
        });
    }

    // --- helpers ------------------------------------------------------------

    /// A temp directory that removes itself on drop.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "lakemind_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    /// Write a tiny single-column parquet file using DuckDB itself.
    fn write_parquet(path: &std::path::Path) {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let sql = format!(
            "COPY (SELECT i AS id FROM range(5) tbl(i)) TO '{}' (FORMAT PARquet);",
            path.to_string_lossy().replace('\\', "/")
        );
        conn.execute(&sql, []).unwrap();
    }
}
