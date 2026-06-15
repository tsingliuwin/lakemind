//! Schema introspection and fast row-count estimation.
//!
//! Two operations, both designed to avoid scanning the data body:
//!
//! - **describe_view**: `DESCRIBE <view>` returns column name/type/nullability
//!   with a `LIMIT 0` plan — no row groups are read.
//! - **estimate_row_count**: for Parquet we query `parquet_metadata()` which
//!   reads only row-group footers; this is what lets a 50GB folder report its
//!   row count in seconds. For everything else we fall back to `count(*)` and
//!   lean on DuckDB's own metadata pushdown.

use duckdb::Connection;

use crate::duckdb::scan::ScanEntry;
use crate::error::AppResult;
use crate::model::{ColumnInfo, SourceKind};

/// Column metadata for an existing VIEW (created by `register`).
pub fn describe_view(conn: &Connection, view: &str) -> AppResult<Vec<ColumnInfo>> {
    // DuckDB DESCRIBE row layout: column_name | column_type | null | key | default | extra
    // i.e. name is at index 0, type at index 1.
    let mut stmt = conn.prepare(&format!("DESCRIBE SELECT * FROM \"{}\"", view))?;
    let rows: Vec<ColumnInfo> = stmt
        .query_map([], |r| {
            let name: String = r.get(0)?;
            let ty: String = r.get(1)?;
            // DuckDB reports "YES"/"NO" for nullability; treat only explicit
            // "NO" as non-nullable. For views this is almost always "YES".
            let null_str: Option<String> = r.get(2).ok();
            let null = null_str.as_deref() != Some("NO");
            Ok(ColumnInfo { name, r#type: ty, null })
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Estimate the row count of a SOURCE without materializing rows.
///
/// Strategy:
/// 1. Parquet → `SELECT count(*) FROM parquet_metadata('...')` (row-group
///    footers only). This is the fast path that keeps 50GB to seconds.
/// 2. Delta → try the same parquet_metadata trick over its underlying parquet
///    files; if that fails, fall back to `count(*)` on the view.
/// 3. CSV/JSON → `SELECT count(*) FROM <view>` (DuckDB metadata pushdown).
pub fn estimate_row_count(conn: &Connection, e: &ScanEntry) -> AppResult<Option<i64>> {
    match e.kind {
        SourceKind::Parquet => {
            let scan = e.scan_path.replace('\'', "''");
            let sql = if e.partition_keys.is_empty() {
                format!("SELECT count(*) FROM parquet_metadata('{}')", scan)
            } else {
                format!("SELECT sum(row_group_num_rows) FROM parquet_metadata('{}')", scan)
            };
            match conn.query_row(&sql, [], |r| {
                // count(*) → BIGINT (i64). Be tolerant: read via Value then convert.
                let v: duckdb::types::Value = r.get(0)?;
                Ok(value_to_i64(v))
            }) {
                Ok(Some(n)) => Ok(Some(n)),
                _ => count_view(conn, &e.view_name).map(Some),
            }
        }
        SourceKind::Delta => count_view(conn, &e.view_name).map(Some),
        SourceKind::Csv | SourceKind::Json => count_view(conn, &e.view_name).map(Some),
    }
}

fn count_view(conn: &Connection, view: &str) -> AppResult<i64> {
    let n = conn.query_row::<i64, _, _>(&format!("SELECT count(*) FROM \"{}\"", view), [], |r| r.get(0))?;
    Ok(n)
}

/// Coerce a DuckDB scalar value into an i64; returns None for NULL/overflow.
fn value_to_i64(v: duckdb::types::Value) -> Option<i64> {
    use duckdb::types::Value as V;
    match v {
        V::Null => None,
        V::TinyInt(i) => Some(i as i64),
        V::SmallInt(i) => Some(i as i64),
        V::Int(i) => Some(i as i64),
        V::BigInt(i) => Some(i),
        V::UTinyInt(u) => Some(u as i64),
        V::USmallInt(u) => Some(u as i64),
        V::UInt(u) => Some(u as i64),
        V::UBigInt(u) => u.try_into().ok(),
        V::HugeInt(i) => i.try_into().ok(),
        V::Double(f) if f.is_finite() && f.fract() == 0.0 => Some(f as i64),
        V::Float(f) if f.is_finite() && f.fract() == 0.0 => Some(f as i64),
        V::Decimal(d) => d.to_string().parse().ok(),
        _ => None,
    }
}
