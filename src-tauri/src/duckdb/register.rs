//! SOURCE registration: turn a [`ScanEntry`] into a read-only DuckDB VIEW.
//!
//! This is the zero-copy core (PRD §3.2). We *never* copy source data; every
//! SOURCE is a `CREATE VIEW` over DuckDB's `read_*` table function, so a 50GB
//! parquet folder costs ~0 bytes of work-space until the user actually queries.

use duckdb::Connection;

use crate::duckdb::scan::ScanEntry;
use crate::duckdb::schema;
use crate::error::AppResult;
use crate::model::SourceTable;

/// Create the VIEW for one scan entry and return a fully-populated
/// `SourceTable` (with columns + row-count estimate already filled in).
pub fn register(conn: &Connection, e: &ScanEntry) -> AppResult<SourceTable> {
    let sql = build_create_table_sql(e);

    // Drop first in case the same name was registered in a prior session.
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", e.view_name);
    conn.execute(&drop_sql, [])?;

    let mut use_gbk = false;
    if let Err(err) = conn.execute(&sql, []) {
        let err_msg = err.to_string();
        if e.kind == crate::model::SourceKind::Csv
            && (err_msg.contains("encoding")
                || err_msg.contains("unicode")
                || err_msg.contains("utf-8")
                || err_msg.contains("UTF-8"))
        {
            use_gbk = true;
        } else {
            return Err(err.into());
        }
    }

    // Populate metadata. Schema is essentially free; row-count uses the
    // parquet_metadata fast path where possible.
    let columns = if use_gbk {
        let _ = conn.execute("INSTALL encodings;", []);
        if conn.execute("LOAD encodings;", []).is_ok() {
            let gbk_sql = format!(
                "CREATE TABLE \"{}\" AS SELECT * FROM read_csv_auto('{}', header = true, encoding = 'zh_CN.gbk');",
                e.view_name,
                e.scan_path.replace('\'', "''")
            );
            conn.execute(&drop_sql, [])?;
            conn.execute(&gbk_sql, [])?;
            schema::describe_view(conn, &e.view_name)?
        } else {
            conn.execute(&sql, [])?;
            schema::describe_view(conn, &e.view_name)?
        }
    } else {
        match schema::describe_view(conn, &e.view_name) {
            Ok(cols) => cols,
            Err(err) => {
                let err_msg = err.to_string();
                if e.kind == crate::model::SourceKind::Csv
                    && (err_msg.contains("encoding")
                        || err_msg.contains("unicode")
                        || err_msg.contains("utf-8")
                        || err_msg.contains("UTF-8"))
                {
                    let _ = conn.execute("INSTALL encodings;", []);
                    if conn.execute("LOAD encodings;", []).is_ok() {
                        let gbk_sql = format!(
                            "CREATE TABLE \"{}\" AS SELECT * FROM read_csv_auto('{}', header = true, encoding = 'zh_CN.gbk');",
                            e.view_name,
                            e.scan_path.replace('\'', "''")
                        );
                        conn.execute(&drop_sql, [])?;
                        conn.execute(&gbk_sql, [])?;
                        schema::describe_view(conn, &e.view_name)?
                    } else {
                        return Err(err);
                    }
                } else {
                    return Err(err);
                }
            }
        }
    };
    // Row-count estimation is best-effort: if it fails (e.g. glob mismatch,
    // metadata read error on a malformed file), do NOT abort registration —
    // the VIEW is still usable, just report an unknown count as None.
    let row_count_estimate = schema::estimate_row_count(conn, e).unwrap_or(None);

    Ok(SourceTable {
        name: e.view_name.clone(),
        label: e.label.clone(),
        kind: e.kind.clone(),
        path: e.path.clone(),
        scan_path: e.scan_path.clone(),
        partition_keys: e.partition_keys.clone(),
        row_count_estimate,
        columns,
    })
}

/// Compose the `CREATE TABLE` statement for an entry. The view name is quoted
/// so arbitrary sanitized labels are safe; the scan path is single-quoted and
/// has any embedded single quotes doubled to prevent literal escape tricks.
fn build_create_table_sql(e: &ScanEntry) -> String {
    let scan = e.scan_path.replace('\'', "''");
    let partition_clause = if e.partition_keys.is_empty() {
        String::new()
    } else {
        format!(", hive_partitioning = 1")
    };
    let inner = match e.kind {
        crate::model::SourceKind::Parquet => {
            format!("read_parquet('{scan}'{partition_clause})")
        }
        crate::model::SourceKind::Csv => {
            format!("read_csv_auto('{scan}', header = true)")
        }
        crate::model::SourceKind::Json => {
            format!("read_json_auto('{scan}')")
        }
        crate::model::SourceKind::Delta => {
            // Delta needs the delta extension; commands.rs ensures it is loaded.
            format!("delta('{scan}')")
        }
        crate::model::SourceKind::Table | crate::model::SourceKind::View => {
            unreachable!("Table/View sources are not registered from raw paths")
        }
    };
    format!("CREATE TABLE \"{}\" AS SELECT * FROM {};", e.view_name, inner)
}
