//! Data transfer objects shared between Rust and the SolidJS frontend.
//!
//! These structs are the *single source of truth* for the M1 wire format:
//! `src/lib/types.ts` mirrors them 1:1. Keep both in sync when changing.

use serde::{Deserialize, Serialize};

/// Physical flavor of a SOURCE table. Mirrors the scan classifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Parquet,
    Csv,
    Json,
    Delta,
    Table,
    View,
}

/// A single column's metadata, produced by `DESCRIBE`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    /// DuckDB type name, e.g. "BIGINT", "VARCHAR", "TIMESTAMP".
    pub r#type: String,
    pub null: bool,
}

/// One registered read-only SOURCE table. Backs the left sidebar tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceTable {
    /// Sanitized view name actually used in SQL, e.g. `s_sales`.
    pub name: String,
    /// Human-friendly name shown in the tree (derived from file/folder).
    pub label: String,
    pub kind: SourceKind,
    /// Original filesystem path the user dropped.
    pub path: String,
    /// Glob / path expression handed to DuckDB's `read_*` function.
    pub scan_path: String,
    /// Hive partition keys detected from the directory layout, if any.
    pub partition_keys: Vec<String>,
    /// Fast estimate (parquet row-group metadata) or full count; `None` until computed.
    pub row_count_estimate: Option<i64>,
    pub columns: Vec<ColumnInfo>,
}

/// Result of an ad-hoc SQL execution. Mirrors PRD §4.1 `SqlResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    /// Rows as heterogeneous JSON values (numbers, strings, null, ...).
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Number of rows actually returned (== rows.len()).
    pub row_count: usize,
    /// True when a SELECT exceeded the row cap and was truncated.
    pub truncated: bool,
    pub elapsed_ms: u64,
}
