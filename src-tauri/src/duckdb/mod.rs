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
        let entries = scan::scan_path(&csv);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Csv);

        let table = register::register(&conn, &entries[0]).unwrap();
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
        let entries = scan::scan_path(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SourceKind::Parquet);

        let table = register::register(&conn, &entries[0]).unwrap();
        assert!(table.row_count_estimate.is_some());
        assert!(table.row_count_estimate.unwrap() >= 1);
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
}
