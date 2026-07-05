//! Data transfer objects shared between Rust and the SolidJS frontend.
//!
//! These structs are the wire format: `src/lib/types.ts` mirrors them 1:1.
//! Keep both in sync when changing.

use serde::{Deserialize, Serialize};

/// Physical flavor of a SOURCE table. Mirrors the scan classifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Parquet,
    Csv,
    Json,
    Delta,
    Excel,
    Table,
    View,
    Postgres,
    Mysql,
    Sqlite,
}

/// How a SOURCE is stored in the DuckLake. Drives the import strategy and the
/// file↔table mapping (see `db::sources` and `commands::import_file_to_workspace`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    /// Materialized into a DuckLake table (small-file imports): data copied
    /// into `<workspace>/lake_data/` as parquet. Persistent + queryable offline.
    Table,
    /// Zero-copy VIEW over an external `read_*` path (large-file imports):
    /// the source file is NOT copied; the view reads it in place.
    View,
    /// A table/view the user created via SQL, not from a file import. Tracked
    /// so it shows up in the Data tree, but has no backing file mapping.
    Custom,
}

impl Default for StorageKind {
    fn default() -> Self {
        Self::Table
    }
}

impl StorageKind {
    /// Parse from the string stored in the SQLite `sources.storage` column.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "view" => Self::View,
            "custom" => Self::Custom,
            _ => Self::Table,
        }
    }
    /// The string written into the SQLite `sources.storage` column.
    pub fn to_db_str(&self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::View => "view",
            Self::Custom => "custom",
        }
    }
}

/// A single column's metadata, produced by `DESCRIBE`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// DuckDB type name, e.g. "BIGINT", "VARCHAR", "TIMESTAMP".
    pub r#type: String,
    pub null: bool,
}

/// One registered SOURCE. Backs the left sidebar Data tree. Mirrors a row in
/// the SQLite `sources` table, enriched with live column metadata from DuckLake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTable {
    /// Sanitized table/view name actually used in SQL, e.g. `s_sales`.
    pub name: String,
    /// Human-friendly name shown in the tree (derived from file/folder).
    pub label: String,
    pub kind: SourceKind,
    /// How this source is stored: DuckLake table / zero-copy view / user custom.
    #[serde(default)]
    pub storage: StorageKind,
    /// Original filesystem path the user dropped (empty for `Custom`).
    pub path: String,
    /// Glob / path expression handed to DuckDB's `read_*` function.
    pub scan_path: String,
    /// Hive partition keys detected from the directory layout, if any.
    pub partition_keys: Vec<String>,
    /// Fast estimate (parquet row-group metadata) or full count; `None` until computed.
    pub row_count_estimate: Option<i64>,
    pub columns: Vec<ColumnInfo>,
    /// Whether this table is a materialized sample of a larger remote table.
    pub is_sampled: bool,
    /// The full row count on the remote database, if this is a sample.
    pub full_row_count: Option<i64>,
}

/// Result of an ad-hoc SQL execution. Mirrors PRD §4.1 `SqlResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
