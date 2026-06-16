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
    let sql = build_create_view_sql(e);

    // Drop first in case the same name was registered in a prior session.
    let drop_sql = format!("DROP VIEW IF EXISTS \"{}\";", e.view_name);
    conn.execute(&drop_sql, [])?;
    conn.execute(&sql, [])?;

    // Populate metadata. Schema is essentially free; row-count uses the
    // parquet_metadata fast path where possible.
    let columns = schema::describe_view(conn, &e.view_name)?;
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

/// Compose the `CREATE VIEW` statement for an entry. The view name is quoted
/// so arbitrary sanitized labels are safe; the scan path is single-quoted and
/// has any embedded single quotes doubled to prevent literal escape tricks.
fn build_create_view_sql(e: &ScanEntry) -> String {
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
    };
    format!("CREATE VIEW \"{}\" AS SELECT * FROM {};", e.view_name, inner)
}
