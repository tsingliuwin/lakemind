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
    /// Worksheet name for multi-sheet Excel files. `None` for single-sheet
    /// files and non-Excel sources. When set, this table is one of several
    /// registered from the same `.xlsx` file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
}

// ---------------------------------------------------------------------------
// Unified logging
// ---------------------------------------------------------------------------

/// Severity level for the unified log store. Mirrors `tracing` levels. Stored
/// as a lowercase TEXT column in SQLite and sent over the wire as a lowercase
/// string — keep `src/lib/types.ts` `UnifiedLog` in sync.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Map a `tracing::Level` to our coarse-grained store level.
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            _ => Self::Debug,
        }
    }
    /// Lowercase string written into the SQLite `logs.level` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
    /// Parse from the string stored in the `logs.level` column; falls back to Info.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "debug" => Self::Debug,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// Normalized log category. Collapses the old ad-hoc `[warmup]` / `[sync]` /
/// `[run_query]` / `[boot]` / `[link]` / `[unlink]` / `[db_tables]` / `[timeout]`
/// / `[xlsx]` / `ducklake:` prefixes into a fixed taxonomy that the multi-tab
/// console and the future log-analysis module filter on.
#[allow(dead_code)]
pub const LOG_CATEGORIES: &[&str] =
    &["query", "import", "agent", "sync", "duckdb", "link", "system", "ui"];

/// One row of the unified `logs` table. The wire format mirrored 1:1 by
/// `src/lib/types.ts` `UnifiedLog`. `detail` is a free-form JSON object for
/// structured fields (sql / rowCount / elapsedMs / error / ...) that vary per
/// category — the message field stays a single-line human summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    /// Row id (autoincrement in SQLite). `None` on insert, filled after write /
    /// on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Unix-ms timestamp.
    pub ts: i64,
    pub level: LogLevel,
    /// One of [`LOG_CATEGORIES`].
    pub category: String,
    /// Single-line human-readable summary.
    pub message: String,
    /// Opaque JSON object of structured detail (sql/rowCount/elapsedMs/error/...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    /// Associated workspace path (`None` for global / startup logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Associated task id (agent logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// Filter clause for `db::query_logs`. Every field is optional; `None` means
/// "no constraint on this dimension".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFilter {
    /// Restrict to these categories (OR). `None` / empty = all categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// Restrict to these levels (OR). `None` / empty = all levels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<String>>,
    /// Inclusive lower bound (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_ts: Option<i64>,
    /// Inclusive upper bound (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_ts: Option<i64>,
    /// Substring match against `message` (case-insensitive LIKE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Page size. Defaults to 200 when unset by the backend.
    pub limit: i64,
    /// Page offset.
    #[serde(default)]
    pub offset: i64,
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
