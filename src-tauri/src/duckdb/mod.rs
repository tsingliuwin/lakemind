//! DuckDB access layer.
//!
//! Sub-modules:
//! - [`pathutil`]   – path / identifier hygiene
//! - [`scan`]       – filesystem classifier → [`scan::ScanEntry`]
//! - [`register`]   – `CREATE VIEW` over a scan entry (zero-copy SOURCE)
//! - [`schema`]     – `DESCRIBE` + parquet-metadata row count
//! - [`execute`]    – ad-hoc SELECT with a safety row cap

pub mod execute;
pub mod pathutil;
pub mod register;
pub mod scan;
pub mod schema;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;

    /// Smoke: the bundled DuckDB builds, opens, and runs SQL. This is the
    /// single most important gate for M1 — if this fails, nothing else matters.
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

        let mut stmt = conn.prepare(
            "SELECT table_name, 'table' as type FROM duckdb_tables() WHERE schema_name = 'main' AND NOT internal
             UNION ALL
             SELECT view_name as table_name, 'view' as type FROM duckdb_views() WHERE schema_name = 'main' AND NOT internal
             ORDER BY table_name"
        ).unwrap();

        struct DbTable {
            name: String,
            kind: String,
        }

        let rows = stmt.query_map([], |row| {
            Ok(DbTable {
                name: row.get(0)?,
                kind: row.get(1)?,
            })
        }).unwrap();

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

    /// CSV scan → register → query, end to end. This is the M1 main path.
    #[test]
    fn scan_register_csv_and_query() {
        let dir = tempdir();
        let csv = dir.path().join("people.csv");
        std::fs::write(&csv, "id,name\n1,alice\n2,bob\n3,carol\n").unwrap();

        let conn = duckdb::Connection::open_in_memory().unwrap();
        let entries = scan::scan_path(&csv, false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Csv);

        let table = register::register(&conn, &entries[0]).unwrap();
        
        let mut stmt = conn.prepare("SELECT view_name, schema_name, internal FROM duckdb_views()").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(0).unwrap();
            let schema: String = row.get(1).unwrap();
            let internal: bool = row.get(2).unwrap();
            println!("view_name: {}, schema_name: {}, internal: {}", name, schema, internal);
        }

        assert!(table.columns.iter().any(|c| c.name == "name"), "columns: {:?}", table.columns);

        let res = execute::run_query(&conn, &format!("SELECT count(*) AS n FROM \"{}\"", table.name), None).unwrap();
        assert_eq!(res.rows[0][0], serde_json::json!(3));
    }

    /// Parquet scan → register → count via the parquet_metadata fast path.
    #[test]
    fn scan_register_parquet_and_count() {
        let dir = tempdir();
        let pq = dir.path().join("data.parquet");
        write_parquet(&pq);

        let conn = duckdb::Connection::open_in_memory().unwrap();
        let entries = scan::scan_path(dir.path(), false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Parquet);

        let table = register::register(&conn, &entries[0]).unwrap();
        assert!(table.row_count_estimate.is_some());
        // write_parquet produced 5 rows (range(5)); the fast path must report
        // the true row count, not the number of row groups (H4 regression guard).
        assert_eq!(table.row_count_estimate.unwrap(), 5);
    }

    /// Real-data acceptance against E:\rustproject\demomind (5 sharded .parq,
    /// ~900MB total, no Hive partitions — the H4 bug's exact trigger shape).
    /// Marked #[ignore] so it only runs on a machine that has the data.
    #[test]
    #[ignore]
    fn acceptance_demomind() {
        let demo = std::path::PathBuf::from(r"E:\rustproject\demomind");
        if !demo.exists() {
            eprintln!("skipped: demomind not present at {}", demo.display());
            return;
        }

        let conn = duckdb::Connection::open_in_memory().unwrap();
        let t_scan = std::time::Instant::now();
        let entries = scan::scan_path(&demo, false);
        let scan_ms = t_scan.elapsed().as_millis();
        println!("=== SCAN: {} entries in {} ms", entries.len(), scan_ms);
        for e in &entries {
            println!(
                "  - {} [{:?}] partition_keys={:?} scan_path={}",
                e.label, e.kind, e.partition_keys, e.scan_path
            );
        }

        for e in &entries {
            let t_reg = std::time::Instant::now();
            let table = register::register(&conn, e).unwrap();
            let reg_ms = t_reg.elapsed().as_millis();

            // Ground truth: an actual count(*) over the view.
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
                reg_ms
            );
            for c in table.columns.iter().take(8) {
                println!("    {} : {}", c.name, c.r#type);
            }

            // Probe parquet_metadata() semantics to settle the H4 count formula.
            // The 427× over-count (== column count) implies each row-group is
            // exposed as N rows (one per column chunk); we need the per-row-group
            // count taken DISTINCTLY, not summed across column chunks.
            let scan_glob = e.scan_path.replace('\'', "''");
            let probe_sqls = [
                ("row count of parquet_metadata", format!("SELECT count(*) FROM parquet_metadata('{}')", scan_glob)),
                ("sum(row_group_num_rows)", format!("SELECT sum(row_group_num_rows) FROM parquet_metadata('{}')", scan_glob)),
                ("distinct row_group_id", format!("SELECT count(DISTINCT row_group_id) FROM parquet_metadata('{}')", scan_glob)),
                ("distinct row_group_num_rows", format!("SELECT sum(row_group_num_rows) FROM (SELECT DISTINCT row_group_id, row_group_num_rows FROM parquet_metadata('{}'))", scan_glob)),
            ];
            for (label, sql) in probe_sqls {
                let n: i64 = match conn.query_row(&sql, [], |r| r.get::<_, duckdb::types::Value>(0)) {
                    Ok(v) => probe_value_to_i64(v),
                    Err(err) => {
                        println!("    PROBE {} = ERR {}", label, err);
                        continue;
                    }
                };
                println!("    PROBE {} = {}", label, n);
            }

            // Execute a capped SELECT to measure read throughput.
            let t_q = std::time::Instant::now();
            let res =
                execute::run_query(&conn, &format!("SELECT * FROM \"{}\"", table.name), Some(1000)).unwrap();
            let q_ms = t_q.elapsed().as_millis();
            println!(
                "=== QUERY: {} rows returned (truncated={}) in {} ms",
                res.row_count, res.truncated, q_ms
            );
        }
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

    /// Write a tiny single-column parquet file using DuckDB itself (no extra
    /// crates needed).
    fn write_parquet(path: &std::path::Path) {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let sql = format!(
            "COPY (SELECT i AS id FROM range(5) tbl(i)) TO '{}' (FORMAT PARquet);",
            path.to_string_lossy().replace('\\', "/")
        );
        conn.execute(&sql, []).unwrap();
    }

    /// Coerce a DuckDB scalar (from a count/sum probe) into i64.
    fn probe_value_to_i64(v: duckdb::types::Value) -> i64 {
        use duckdb::types::Value as V;
        match v {
            V::Null => -1,
            V::TinyInt(i) => i as i64,
            V::SmallInt(i) => i as i64,
            V::Int(i) => i as i64,
            V::BigInt(i) => i,
            V::HugeInt(i) => i as i64,
            V::UTinyInt(u) => u as i64,
            V::USmallInt(u) => u as i64,
            V::UInt(u) => u as i64,
            V::UBigInt(u) => u as i64,
            V::Double(f) if f.is_finite() => f as i64,
            V::Float(f) if f.is_finite() => f as i64,
            V::Decimal(d) => d.to_string().parse().unwrap_or(-1),
            _ => -1,
        }
    }

    #[test]
    fn test_scan_default_project() {
        let root = std::path::Path::new("C:/Users/lyq/.lakemind/DefaultProject");
        assert!(root.exists(), "Root directory C:/Users/lyq/.lakemind/DefaultProject does not exist!");
        let entries = scan::scan_path(root, true);
        println!("Scan entries: {:#?}", entries);
        let conn = duckdb::Connection::open_in_memory().unwrap();
        for e in &entries {
            match register::register(&conn, e) {
                Ok(t) => println!("Successfully registered table through fallback! Columns: {:?}", t.columns),
                Err(err) => panic!("Failed to register {}: {}", e.label, err),
            }
        }
    }

    #[test]
    fn test_register_workspace_sources_incremental() {
        let temp = tempdir();
        let ws_path = temp.path();

        // 1. Create a dummy CSV file in the workspace
        let csv_path = ws_path.join("users_test.csv");
        std::fs::write(&csv_path, "id,name\n1,Alice\n2,Bob\n").unwrap();

        // 2. Initialize AppState
        let state = crate::state::AppState::new().unwrap();

        // Run the commands via block_on
        tauri::async_runtime::block_on(async {
            // First register: should ingest the table
            let ws_str = ws_path.to_string_lossy().to_string();
            let tables = crate::commands::register_workspace_sources_inner(ws_str.clone(), &state)
                .await
                .unwrap();
            
            assert_eq!(tables.len(), 1);
            assert_eq!(tables[0].name, "s_users_test");
            assert_eq!(tables[0].row_count_estimate, Some(2));

            // Verify lake.duckdb is created physically
            let db_file = ws_path.join("lake.duckdb");
            assert!(db_file.exists(), "lake.duckdb database file must exist on disk");

            // Verify we can query the table in DuckDB
            {
                let guard = state.conn.lock().await;
                let count: i64 = guard
                    .query_row("SELECT count(*) FROM s_users_test", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 2);
            }

            // Second register (incremental scan): table exists, should not ingest again
            let tables_cached = crate::commands::register_workspace_sources_inner(ws_str.clone(), &state)
                .await
                .unwrap();
            assert_eq!(tables_cached.len(), 1);
            assert_eq!(tables_cached[0].name, "s_users_test");

            // Delete the physical file
            std::fs::remove_file(&csv_path).unwrap();

            // Register again: should drop the orphan table and clear sources
            let tables_after_delete = crate::commands::register_workspace_sources_inner(ws_str.clone(), &state)
                .await
                .unwrap();
            assert_eq!(tables_after_delete.len(), 0);

            // Verify the table was physically dropped
            {
                let guard = state.conn.lock().await;
                let count_tables: i64 = guard
                    .query_row(
                        "SELECT count(*) FROM duckdb_tables() WHERE table_name = 's_users_test'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(count_tables, 0, "Table must be dropped from database");
            }
        });
    }

    #[test]
    fn test_scan_register_excel_and_query() {
        let excel_path = std::path::Path::new("test_excel.xlsx");
        if !excel_path.exists() {
            eprintln!("skipped: test_excel.xlsx not present");
            return;
        }
        let excel_file = std::fs::canonicalize(excel_path).unwrap();

        // Load excel extension
        let conn = duckdb::Connection::open_in_memory().unwrap();
        if let Err(e) = conn.execute("INSTALL excel;", []) {
            println!("INSTALL excel failed: {}", e);
        }
        if let Err(e) = conn.execute("LOAD excel;", []) {
            println!("LOAD excel failed: {}", e);
        }

        let entries = scan::scan_path(&excel_file, false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Excel);

        let table = register::register(&conn, &entries[0]).unwrap();

        // The columns must contain "月份", "2026年GMV", "2025年GMV" because of our smart scoring strategy select.
        assert!(
            table.columns.iter().any(|c| c.name == "月份"),
            "Columns: {:?}",
            table.columns
        );
        assert!(
            table.columns.iter().any(|c| c.name == "2026年GMV"),
            "Columns: {:?}",
            table.columns
        );

        // Run query over registered table
        let count_sql = format!("SELECT count(*) FROM \"{}\"", table.name);
        let res = execute::run_query(&conn, &count_sql, None).unwrap();
        let row_count = res.rows[0][0].as_i64().unwrap();
        println!("Registered Excel rows count: {}", row_count);
        assert!(row_count > 0);
    }
}
