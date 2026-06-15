//! Ad-hoc SQL execution with a safety row cap.
//!
//! `run_query` wraps a user statement in a defensive `LIMIT` so a careless
//! `SELECT *` over a 50GB table cannot OOM the frontend. The cap is applied
//! by wrapping the query as a subquery; DuckDB's optimizer folds the wrapper.

use std::time::Instant;

use duckdb::types::Value as DuckValue;

use crate::error::AppResult;
use crate::model::SqlResult;

/// Run a SELECT and return a row-capped [`SqlResult`]. `cap` of `None` means
/// no cap (the caller must opt in explicitly via the UI).
pub fn run_query(conn: &duckdb::Connection, sql: &str, cap: Option<usize>) -> AppResult<SqlResult> {
    let start = Instant::now();

    // Wrap the user's SQL as a subquery so an arbitrary statement (SELECT,
    // UNION, CTE, even one that already has LIMIT) can be safely capped.
    // DuckDB folds the wrapper away during optimization.
    let inner = sql.trim().trim_end_matches(';');
    let wrapped = match cap {
        Some(n) => format!("SELECT * FROM ({inner}) AS _lakemind_q LIMIT {n}"),
        None => format!("SELECT * FROM ({inner}) AS _lakemind_q"),
    };

    let mut stmt = conn.prepare(&wrapped)?;
    // Execute once to populate the statement's schema metadata; the schema is
    // only available *after* execution. We then re-read rows via raw_query
    // (which does not re-execute) so we can borrow stmt immutably for schema.
    stmt.execute([])?;
    let schema = stmt.schema();
    let column_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    let column_types: Vec<String> = schema.fields().iter().map(|f| format!("{}", f.data_type())).collect();
    let col_count = column_names.len();

    let mut rows_out: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut iter = stmt.raw_query();
    while let Some(row) = iter.next()? {
        let mut out = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let val: DuckValue = row.get(i)?;
            out.push(duck_value_to_json(val));
        }
        rows_out.push(out);
    }

    let truncated = cap.map_or(false, |n| rows_out.len() >= n);
    let row_count = rows_out.len();

    Ok(SqlResult {
        columns: column_names,
        column_types,
        rows: rows_out,
        row_count,
        truncated,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// Map a DuckDB runtime value to JSON, preserving nulls and numeric precision.
fn duck_value_to_json(v: DuckValue) -> serde_json::Value {
    match v {
        DuckValue::Null => serde_json::Value::Null,
        DuckValue::Boolean(b) => serde_json::Value::Bool(b),
        DuckValue::TinyInt(i) => num_i64(i as i64),
        DuckValue::SmallInt(i) => num_i64(i as i64),
        DuckValue::Int(i) => num_i64(i as i64),
        DuckValue::BigInt(i) => num_i64(i),
        // HugeInt overflows f64/i64; stringify to preserve precision.
        DuckValue::HugeInt(i) => serde_json::Value::String(i.to_string()),
        DuckValue::UTinyInt(u) => num_u64(u as u64),
        DuckValue::USmallInt(u) => num_u64(u as u64),
        DuckValue::UInt(u) => num_u64(u as u64),
        DuckValue::UBigInt(u) => num_u64(u),
        // f64 → JSON Number via serde_json's from_f64 (handles NaN/Inf as null).
        DuckValue::Float(f) => serde_json::Number::from_f64(f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DuckValue::Double(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DuckValue::Decimal(d) => serde_json::Value::String(d.to_string()),
        DuckValue::Timestamp(unit, micros) => serde_json::Value::String(format_ts(unit, micros)),
        DuckValue::Text(s) => serde_json::Value::String(s),
        DuckValue::Blob(b) => {
            let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            serde_json::Value::String(hex)
        }
        DuckValue::Date32(days) => serde_json::Value::String(format_date(days)),
        DuckValue::Time64(unit, v) => serde_json::Value::String(format_time(unit, v)),
        DuckValue::Interval { months, days, nanos } => {
            serde_json::Value::String(format!("{months} months {days} days {nanos} ns"))
        }
        // Lists / structs / enums / arrays / maps / unions → lossless-ish JSON.
        DuckValue::List(items) => serde_json::Value::Array(items.into_iter().map(duck_value_to_json).collect()),
        DuckValue::Enum(s) => serde_json::Value::String(s),
        DuckValue::Struct(map) => {
            // OrderedMap is a Vec<(String, Value)> newtype with only borrowed
            // iter(); clone the pairs out so we can own the values.
            let mut obj = serde_json::Map::new();
            for (k, val) in map.iter().cloned() {
                obj.insert(k, duck_value_to_json(val));
            }
            serde_json::Value::Object(obj)
        }
        DuckValue::Array(items) => serde_json::Value::Array(items.into_iter().map(duck_value_to_json).collect()),
        DuckValue::Map(map) => {
            let mut arr = Vec::new();
            for (k, val) in map.iter().cloned() {
                let mut entry = serde_json::Map::new();
                entry.insert("key".to_string(), duck_value_to_json(k));
                entry.insert("value".to_string(), duck_value_to_json(val));
                arr.push(serde_json::Value::Object(entry));
            }
            serde_json::Value::Array(arr)
        }
        DuckValue::Union(inner) => duck_value_to_json(*inner),
    }
}

fn num_i64(i: i64) -> serde_json::Value {
    serde_json::Number::from(i).into()
}
fn num_u64(u: u64) -> serde_json::Value {
    serde_json::Number::from(u).into()
}

// --- temporal formatting ----------------------------------------------------
//
// DuckDB hands us raw integers (micros, days). We render them to ISO-ish
// strings good enough for a read-only table cell. M2+ may switch to a typed
// channel that keeps these as structured values.

use duckdb::types::TimeUnit;

fn format_ts(unit: TimeUnit, raw: i64) -> String {
    // DuckDB's Timestamp stores microseconds regardless of declared unit here.
    let _ = unit;
    let micros = raw;
    let secs = micros.div_euclid(1_000_000);
    let rem_us = micros.rem_euclid(1_000_000);
    civil_from_secs(secs, rem_us)
}

fn format_time(unit: TimeUnit, raw: i64) -> String {
    let _ = unit;
    let micros = raw.rem_euclid(86_400 * 1_000_000);
    let tod = micros / 1_000_000;
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    let us = micros % 1_000_000;
    format!("{:02}:{:02}:{:02}.{:06}", h, m, s, us)
}

fn format_date(days_since_epoch: i32) -> String {
    let z = days_since_epoch as i64 + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", year, month, d)
}

fn civil_from_secs(secs: i64, rem_us: i64) -> String {
    let (days, tod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
        year, month, d, h, m, s, rem_us
    )
}
