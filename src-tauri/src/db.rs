//! Global metadata store: `~/.lakemind/lakemind.db` (SQLite via `rusqlite`).
//!
//! Holds three registries that are *not* data themselves:
//!   * `workspaces` — registered workspace directories
//!   * `tasks`      — per-workspace SQL/chat task index (content lives in files)
//!   * `sources`    — the **file ↔ table mapping**: for each registered SOURCE,
//!                    which file produced it, under what name, and how it is
//!                    stored (DuckLake table vs zero-copy view vs user custom)
//!   * `config`     — key/value user settings (e.g. the zero-copy threshold)
//!
//! DuckLake itself already persists table data + catalog metadata; this DB adds
//! the *business* mapping layer on top (so the UI can tie a tree file node to
//! its table, show how it is stored, and rebuild views after restart).

use std::fs;
use std::path::PathBuf;
use rusqlite::Connection;

use crate::duckdb::lake;

/// Get the system home directory
pub fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()
}

/// Get the global config path ~/.lakemind/
pub fn get_lakemind_dir() -> Result<PathBuf, String> {
    let mut path = get_home_dir().ok_or("Could not resolve home directory".to_string())?;
    path.push(".lakemind");
    Ok(path)
}

/// Get the global sqlite database file path ~/.lakemind/lakemind.db
pub fn get_db_path() -> Result<PathBuf, String> {
    let mut path = get_lakemind_dir()?;
    path.push("lakemind.db");
    Ok(path)
}

/// Establish connection to sqlite database.
///
/// Each call opens a fresh connection (the app fans out concurrent reads/writes
/// from many commands). Two pragmas make that safe under concurrency:
/// - `busy_timeout = 5000`: wait up to 5s for a lock instead of failing
///   instantly with SQLITE_BUSY when two connections race for the write lock.
/// - `journal_mode = WAL`: readers never block writers (and vice-versa), so
///   concurrent commands don't serialize on the DB file.
pub fn get_db_conn() -> Result<Connection, String> {
    let db_path = get_db_path()?;
    let conn = Connection::open(&db_path)
        .map_err(|e| format!("Failed to open SQLite database: {e}"))?;
    let _ = conn.pragma_update(None, "busy_timeout", 5000);
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    Ok(conn)
}

// ---------------------------------------------------------------------------
// sources: the file ↔ table mapping
// ---------------------------------------------------------------------------

/// One row of the `sources` table — the persistent file↔table↔storage mapping.
/// `kind`/`storage` are stored as lowercase strings (see `SourceKind`/
/// `StorageKind` serde forms and `StorageKind::to_db_str`).
#[derive(Debug, Clone)]
pub struct SourceRecord {
    pub table_name: String,
    pub label: String,
    pub kind: String,
    pub storage: String,
    pub file_path: String,
    pub scan_path: String,
    pub partition_keys: Vec<String>,
    pub created_at: i64,
    /// How `table_name` was generated: `"legacy"` (pre-naming-module),
    /// `"fallback"` (pinyin, LLM unavailable/failed), or `"llm"` (LLM-generated
    /// and cached). Only `legacy`/`fallback` trigger a re-evaluation on sync.
    pub name_source: String,
    /// Source file mtime (ms since epoch). When it changes vs. the on-disk file,
    /// `sync_entries` rebuilds the source so downstream objects see fresh data.
    pub file_mtime: i64,
    /// Source file size in bytes (sum for multi-file globs). Companion to mtime.
    pub file_size: i64,
    /// Cached column structure (JSON). Valid as long as the fingerprint matches;
    /// refreshed on rebuild. Lets the startup list skip `describe_view`.
    pub columns: Vec<crate::model::ColumnInfo>,
    /// Cached row count. Valid as long as the fingerprint matches; refreshed on
    /// rebuild. `None` when never computed.
    pub row_count: Option<i64>,
    /// Whether this source is a materialized sample of a larger remote table.
    pub is_sampled: bool,
    /// The full row count on the remote database, if this is a sample.
    pub full_row_count: Option<i64>,
    /// Materialization status of this source:
    /// `"sampled"` — a small sample of a remote table (aggregation misleads).
    /// `"partial"` — partially materialized (resume / on-demand); still
    ///   incomplete, so aggregation still misleads.
    /// `"full"`    — fully materialized; aggregation is safe.
    /// `None`/empty is treated as `sampled` (when `is_sampled`) or `full`.
    pub materialize_status: Option<String>,
    /// For maxcompute partitioned tables: JSON array of already-materialized
    /// partition specs (e.g. `["ds=20250701","ds=20250702"]`), used for resume.
    /// `None` for non-partitioned tables / non-maxcompute sources.
    pub materialized_partitions: Option<String>,
    /// Worksheet name for multi-sheet Excel files. `None` for single-sheet
    /// files and non-Excel sources. Part of the source identity key together
    /// with `scan_path`, so the same `.xlsx` can back multiple rows.
    pub sheet: Option<String>,
}

/// Status strings stored in `sources.materialize_status`.
pub mod mat_status {
    pub const SAMPLED: &str = "sampled";
    pub const PARTIAL: &str = "partial";
    pub const FULL: &str = "full";
}

impl SourceRecord {
    /// True iff aggregating this table's local copy would mislead — i.e. it is
    /// a sample OR only partially materialized. `full` returns false.
    pub fn aggregation_misleads(&self) -> bool {
        let full = mat_status::FULL;
        let partial = mat_status::PARTIAL;
        let sampled = mat_status::SAMPLED;
        match self.materialize_status.as_deref() {
            Some(s) if s == full => false,
            Some(s) if s == partial || s == sampled => true,
            // Legacy rows (NULL status): fall back to the is_sampled flag.
            None => self.is_sampled,
            // Unknown status string: treat as misleading to be safe.
            Some(_) => true,
        }
    }

    /// For maxcompute sources, extract the remote `project.table` reference
    /// from `file_path` (`maxcompute://{conn_id}/{project}/{table}`).
    /// Returns `None` for non-maxcompute sources or malformed paths.
    pub fn maxcompute_remote_ref(&self) -> Option<String> {
        if self.kind != "maxcompute" {
            return None;
        }
        // file_path = "maxcompute://{conn_id}/{project}/{table}"
        let segs: Vec<&str> = self.file_path.split('/').collect();
        // segs: ["maxcompute:", "", "{conn_id}", "{project}", "{table}"]
        if segs.len() >= 5 {
            let project = segs[3];
            let table = segs[4];
            if !project.is_empty() && !table.is_empty() {
                return Some(format!("{project}.{table}"));
            }
        }
        None
    }
}

/// Insert or update a source mapping (keyed by `(workspace_path, table_name)`).
pub fn upsert_source(conn: &Connection, ws_path: &str, r: &SourceRecord) -> Result<(), String> {
    let keys = serde_json::to_string(&r.partition_keys).unwrap_or_else(|_| "[]".into());
    let cols = serde_json::to_string(&r.columns).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO sources (workspace_path, table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at, name_source, file_mtime, file_size, columns, row_count, is_sampled, full_row_count, materialize_status, materialized_partitions, sheet)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_path, table_name) DO UPDATE SET
            label=excluded.label, kind=excluded.kind, storage=excluded.storage,
            file_path=excluded.file_path, scan_path=excluded.scan_path,
            partition_keys=excluded.partition_keys, name_source=excluded.name_source,
            file_mtime=excluded.file_mtime, file_size=excluded.file_size,
            columns=excluded.columns, row_count=excluded.row_count,
            is_sampled=excluded.is_sampled, full_row_count=excluded.full_row_count,
            materialize_status=excluded.materialize_status,
            materialized_partitions=excluded.materialized_partitions, sheet=excluded.sheet",
        rusqlite::params![
            ws_path, r.table_name, r.label, r.kind, r.storage, r.file_path, r.scan_path, keys,
            r.created_at, r.name_source, r.file_mtime, r.file_size, cols, r.row_count,
            if r.is_sampled { 1 } else { 0 }, r.full_row_count, r.materialize_status,
            r.materialized_partitions, r.sheet
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// All source mappings for one workspace, in creation order.
pub fn list_sources(conn: &Connection, ws_path: &str) -> Result<Vec<SourceRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at, name_source, file_mtime, file_size, columns, row_count, is_sampled, full_row_count, materialize_status, materialized_partitions, sheet
             FROM sources WHERE workspace_path = ? ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([ws_path], |row| {
            let keys_json: String = row.get(6)?;
            let partition_keys: Vec<String> =
                serde_json::from_str(&keys_json).unwrap_or_default();
            let cols_json: String = row.get(11).unwrap_or_else(|_| "[]".to_string());
            let columns: Vec<crate::model::ColumnInfo> =
                serde_json::from_str(&cols_json).unwrap_or_default();
            let is_sampled_val: i32 = row.get(13).unwrap_or(0);
            Ok(SourceRecord {
                table_name: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                storage: row.get(3)?,
                file_path: row.get(4)?,
                scan_path: row.get(5)?,
                partition_keys,
                created_at: row.get(7)?,
                name_source: row.get(8).unwrap_or_else(|_| "legacy".to_string()),
                file_mtime: row.get(9).unwrap_or(0),
                file_size: row.get(10).unwrap_or(0),
                columns,
                row_count: row.get(12).ok(),
                is_sampled: is_sampled_val != 0,
                full_row_count: row.get(14).ok(),
                materialize_status: row.get(15).ok(),
                materialized_partitions: row.get(16).ok(),
                sheet: row.get(17).ok(),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        if let Ok(rec) = r {
            out.push(rec);
        }
    }
    Ok(out)
}

/// Look up a single source mapping by table name (within a workspace).
#[allow(dead_code)]
pub fn get_source_by_table(
    conn: &Connection,
    ws_path: &str,
    table_name: &str,
) -> Result<Option<SourceRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at, name_source, file_mtime, file_size, columns, row_count, is_sampled, full_row_count, materialize_status, materialized_partitions, sheet
             FROM sources WHERE workspace_path = ? AND table_name = ?",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(rusqlite::params![ws_path, table_name], |row| {
            let keys_json: String = row.get(6)?;
            let partition_keys: Vec<String> =
                serde_json::from_str(&keys_json).unwrap_or_default();
            let cols_json: String = row.get(11).unwrap_or_else(|_| "[]".to_string());
            let columns: Vec<crate::model::ColumnInfo> =
                serde_json::from_str(&cols_json).unwrap_or_default();
            let is_sampled_val: i32 = row.get(13).unwrap_or(0);
            Ok(SourceRecord {
                table_name: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                storage: row.get(3)?,
                file_path: row.get(4)?,
                scan_path: row.get(5)?,
                partition_keys,
                created_at: row.get(7)?,
                name_source: row.get(8).unwrap_or_else(|_| "legacy".to_string()),
                file_mtime: row.get(9).unwrap_or(0),
                file_size: row.get(10).unwrap_or(0),
                columns,
                row_count: row.get(12).ok(),
                is_sampled: is_sampled_val != 0,
                full_row_count: row.get(14).ok(),
                materialize_status: row.get(15).ok(),
                materialized_partitions: row.get(16).ok(),
                sheet: row.get(17).ok(),
            })
        })
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(Ok(rec)) => Ok(Some(rec)),
        _ => Ok(None),
    }
}

/// Remove one source mapping (does NOT touch DuckLake; the caller drops the
/// table/view there). Safe if the row is absent.
pub fn delete_source_by_table(
    conn: &Connection,
    ws_path: &str,
    table_name: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM sources WHERE workspace_path = ? AND table_name = ?",
        rusqlite::params![ws_path, table_name],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// object_defs: persistent definition + input fingerprint for agent objects
// ---------------------------------------------------------------------------

/// One row of `object_defs` — the persisted SELECT that built an agent-created
/// table/view, plus a hash over all its upstream inputs' fingerprints. When the
/// upstream fingerprint is unchanged and the lake object still exists, the
/// CREATE can be skipped (incremental build). When the lake is rebuilt after a
/// crash, this record lets the object be recreated from `select_sql`.
#[derive(Debug, Clone)]
pub struct ObjectDef {
    pub table_name: String,
    /// "table" or "view" — matches StorageKind::to_db_str semantics.
    pub kind: String,
    pub select_sql: String,
    pub input_hash: String,
    pub created_at: i64,
    /// Cached column structure (JSON). Valid as long as input_hash matches.
    pub columns: Vec<crate::model::ColumnInfo>,
    /// Cached row count. Valid as long as input_hash matches.
    pub row_count: Option<i64>,
}

/// Insert or update an object definition (keyed by `(workspace_path, table_name)`).
pub fn upsert_object_def(conn: &Connection, ws_path: &str, d: &ObjectDef) -> Result<(), String> {
    let cols = serde_json::to_string(&d.columns).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO object_defs (workspace_path, table_name, kind, select_sql, input_hash, created_at, columns, row_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_path, table_name) DO UPDATE SET
            kind=excluded.kind, select_sql=excluded.select_sql,
            input_hash=excluded.input_hash, created_at=excluded.created_at,
            columns=excluded.columns, row_count=excluded.row_count",
        rusqlite::params![ws_path, d.table_name, d.kind, d.select_sql, d.input_hash, d.created_at, cols, d.row_count],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Look up an object definition by table name within a workspace.
pub fn get_object_def(
    conn: &Connection,
    ws_path: &str,
    table_name: &str,
) -> Result<Option<ObjectDef>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT table_name, kind, select_sql, input_hash, created_at, columns, row_count
             FROM object_defs WHERE workspace_path = ? AND table_name = ?",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(rusqlite::params![ws_path, table_name], |row| {
            let cols_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
            let columns: Vec<crate::model::ColumnInfo> =
                serde_json::from_str(&cols_json).unwrap_or_default();
            Ok(ObjectDef {
                table_name: row.get(0)?,
                kind: row.get(1)?,
                select_sql: row.get(2)?,
                input_hash: row.get(3)?,
                created_at: row.get(4)?,
                columns,
                row_count: row.get(6).ok(),
            })
        })
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(Ok(d)) => Ok(Some(d)),
        _ => Ok(None),
    }
}

/// Remove one object definition. Safe if the row is absent.
pub fn delete_object_def(
    conn: &Connection,
    ws_path: &str,
    table_name: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM object_defs WHERE workspace_path = ? AND table_name = ?",
        rusqlite::params![ws_path, table_name],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// All object definitions for one workspace, in creation order.
pub fn list_object_defs(conn: &Connection, ws_path: &str) -> Result<Vec<ObjectDef>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT table_name, kind, select_sql, input_hash, created_at, columns, row_count
             FROM object_defs WHERE workspace_path = ? ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([ws_path], |row| {
            let cols_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
            let columns: Vec<crate::model::ColumnInfo> =
                serde_json::from_str(&cols_json).unwrap_or_default();
            Ok(ObjectDef {
                table_name: row.get(0)?,
                kind: row.get(1)?,
                select_sql: row.get(2)?,
                input_hash: row.get(3)?,
                created_at: row.get(4)?,
                columns,
                row_count: row.get(6).ok(),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        if let Ok(d) = r {
            out.push(d);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// config: key/value user settings
// ---------------------------------------------------------------------------

/// Read a config value. Returns `None` for missing or empty values.
pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM config WHERE key = ?", [key], |r| r.get(0))
        .ok();
    Ok(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Set (upsert) a config value.
pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO config (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The zero-copy import threshold in bytes, from config or the default.
#[allow(dead_code)]
pub fn get_zero_copy_threshold(conn: &Connection) -> Result<u64, String> {
    match get_config(conn, lake::THRESHOLD_CONFIG_KEY)? {
        Some(s) => s.parse::<u64>().map_err(|e| format!("invalid threshold config: {e}")),
        None => Ok(lake::DEFAULT_ZERO_COPY_THRESHOLD),
    }
}

// ---------------------------------------------------------------------------
// schema initialization
// ---------------------------------------------------------------------------

/// Initialize central directory structure and table schemas. Idempotent.
pub fn init_global_db() -> Result<(), String> {
    let lakemind_dir = get_lakemind_dir()?;

    // Content directories for task files.
    let sqls_dir = lakemind_dir.join("sqls");
    let chats_dir = lakemind_dir.join("chats");
    fs::create_dir_all(&sqls_dir).map_err(|e| format!("Failed to create sqls directory: {e}"))?;
    fs::create_dir_all(&chats_dir)
        .map_err(|e| format!("Failed to create chats directory: {e}"))?;

    let conn = get_db_conn()?;
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);

    // workspaces registry
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspaces (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create workspaces table: {e}"))?;

    // tasks index (content lives in sqls/<id>.sql or chats/<id>.json)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            workspace_path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            saved INTEGER NOT NULL,
            model_id TEXT,
            token_usage TEXT,
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create tasks table: {e}"))?;

    // Migrate tasks table to add model_id column if it doesn't exist
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN model_id TEXT;", []);
    // Migrate tasks table to add token_usage column if it doesn't exist
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN token_usage TEXT;", []);

    // sources: the file ↔ table ↔ storage mapping. file_mtime/file_size form
    // the source fingerprint; columns/row_count are cached DuckLake metadata
    // valid as long as the fingerprint matches (refreshed on rebuild). All part
    // of the base schema — no ALTER migration.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS sources (
            workspace_path TEXT NOT NULL,
            table_name     TEXT NOT NULL,
            label          TEXT NOT NULL,
            kind           TEXT NOT NULL,
            storage        TEXT NOT NULL,
            file_path      TEXT NOT NULL DEFAULT '',
            scan_path      TEXT NOT NULL DEFAULT '',
            partition_keys TEXT NOT NULL DEFAULT '[]',
            created_at     INTEGER NOT NULL,
            name_source    TEXT NOT NULL DEFAULT 'legacy',
            file_mtime     INTEGER NOT NULL DEFAULT 0,
            file_size      INTEGER NOT NULL DEFAULT 0,
            columns        TEXT NOT NULL DEFAULT '[]',
            row_count      INTEGER,
            is_sampled     INTEGER NOT NULL DEFAULT 0,
            full_row_count INTEGER,
            materialize_status TEXT,
            sheet           TEXT,
            PRIMARY KEY (workspace_path, table_name),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create sources table: {e}"))?;

    // Migrate sources table to add is_sampled / full_row_count columns if they don't exist
    let _ = conn.execute("ALTER TABLE sources ADD COLUMN is_sampled INTEGER NOT NULL DEFAULT 0;", []);
    let _ = conn.execute("ALTER TABLE sources ADD COLUMN full_row_count INTEGER;", []);
    // materialize_status: 'sampled' | 'partial' | 'full'. Added for resume /
    // on-demand materialization. Backfills existing rows from is_sampled so the
    // status reflects reality on first read after migration.
    if conn.execute("ALTER TABLE sources ADD COLUMN materialize_status TEXT;", []).is_ok() {
        let _ = conn.execute(
            "UPDATE sources SET materialize_status = CASE WHEN is_sampled = 1 THEN 'sampled' ELSE 'full' END WHERE materialize_status IS NULL;",
            [],
        );
    }
    // sheet: worksheet name for multi-sheet Excel files. NULL for single-sheet
    // files and non-Excel sources. Added for multi-sheet Excel support; old rows
    // get NULL (treated as single-sheet, preserving legacy behavior).
    let _ = conn.execute("ALTER TABLE sources ADD COLUMN sheet TEXT;", []);
    // materialized_partitions: JSON array of already-materialized MaxCompute
    // partition specs, used for resume of partitioned-table materialization.
    // NULL for non-maxcompute sources and non-partitioned tables.
    let _ = conn.execute("ALTER TABLE sources ADD COLUMN materialized_partitions TEXT;", []);

    // config: key/value user settings (NEW)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create config table: {e}"))?;

    // object_defs: persistent definition + input fingerprint for agent-created
    // tables/views (t_/v_/tmp_/tmp_v_). Lets incremental builds skip re-running
    // CREATE TABLE AS when upstream inputs are unchanged, and rebuilds the
    // object after a lake crash-recovery that would otherwise lose it.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS object_defs (
            workspace_path  TEXT NOT NULL,
            table_name      TEXT NOT NULL,
            kind            TEXT NOT NULL,
            select_sql      TEXT NOT NULL,
            input_hash      TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            columns         TEXT NOT NULL DEFAULT '[]',
            row_count       INTEGER,
            PRIMARY KEY (workspace_path, table_name),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create object_defs table: {e}"))?;

    // db_connections: external database connections
    conn.execute(
        "CREATE TABLE IF NOT EXISTS db_connections (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            db_type       TEXT NOT NULL,
            host          TEXT NOT NULL,
            port          INTEGER NOT NULL,
            database_name TEXT NOT NULL,
            username      TEXT NOT NULL,
            password      TEXT NOT NULL,
            ssl_mode      TEXT NOT NULL DEFAULT 'disable',
            created_at    INTEGER NOT NULL,
            options       TEXT
        )",
        [],
    )
    .map_err(|e| format!("Failed to create db_connections table: {e}"))?;

    // Sidecar DB types (MaxCompute, future generic JDBC) store type-specific
    // params (endpoint/project/region/driver-coord/...) as JSON in `options`.
    // Idempotent ALTER for databases created before this column existed.
    let _ = conn.execute("ALTER TABLE db_connections ADD COLUMN options TEXT;", []);

    // workspace_connections: many-to-many relationship
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspace_connections (
            workspace_path TEXT NOT NULL,
            connection_id  TEXT NOT NULL,
            PRIMARY KEY (workspace_path, connection_id),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE,
            FOREIGN KEY(connection_id) REFERENCES db_connections(id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create workspace_connections table: {e}"))?;

    // db_connection_tables: cached list of tables/views per external connection.
    // Listing tables from a remote (e.g. Neon postgres) via DuckDB's catalog
    // scans the server's system tables and takes ~2s per query, so we cache the
    // result locally and only re-hit the server on explicit refresh / first use.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS db_connection_tables (
            connection_id  TEXT NOT NULL,
            schema_name    TEXT NOT NULL,
            table_name     TEXT NOT NULL,
            kind           TEXT NOT NULL,
            cached_at      INTEGER NOT NULL,
            PRIMARY KEY (connection_id, schema_name, table_name),
            FOREIGN KEY(connection_id) REFERENCES db_connections(id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create db_connection_tables table: {e}"))?;

    // logs: the unified, queryable log store. Every log line from the backend
    // (tracing) and the frontend (append_log command) lands here, indexed for
    // the multi-tab console's time/level/category filters and the future
    // log-analysis module. No FK to workspaces — logs are retained independently
    // so deleting a workspace doesn't erase its history.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS logs (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            ts       INTEGER NOT NULL,
            level    TEXT NOT NULL,
            category TEXT NOT NULL,
            message  TEXT NOT NULL,
            detail   TEXT,
            workspace TEXT,
            task_id  TEXT
        )",
        [],
    )
    .map_err(|e| format!("Failed to create logs table: {e}"))?;
    // First indexes in the codebase: cover the console's and the analysis
    // module's dominant access paths — newest-first time scans, per-category
    // and per-level filtered listings.
    let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_logs_ts ON logs(ts DESC);", []);
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_logs_cat_ts ON logs(category, ts DESC);",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_logs_level_ts ON logs(level, ts DESC);",
        [],
    );

    // Seed the default workspace on first run.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .unwrap_or(0);
    if count == 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        conn.execute(
            "INSERT INTO workspaces (path, name, created_at) VALUES ('DefaultProject', 'DefaultProject', ?)",
            [now],
        )
        .map_err(|e| format!("Failed to insert default workspace: {e}"))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// db_connections CRUD and workspace linking operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionRecord {
    pub id: String,
    pub name: String,
    pub db_type: String, // "postgres" | "mysql" | "sqlite" | "maxcompute"
    pub host: String,
    pub port: i32,
    pub database_name: String, // for sqlite: the local file path; host/port/user/password unused
    pub username: String,
    pub password: String,
    pub ssl_mode: String,
    pub created_at: i64,
    /// JSON of type-specific params for sidecar DB types (MaxCompute: endpoint,
    /// project, region, tunnel_endpoint, driver_coord, concurrency). None for
    /// ATTACH-based types (postgres/mysql/sqlite). AK/SK reuse username/password.
    pub options: Option<String>,
}

/// `db_type` values whose external connectivity is provided by a Java sidecar
/// (not via DuckDB ATTACH). They store params in `DbConnectionRecord.options`
/// and skip the ATTACH/DETACH path entirely.
pub fn is_sidecar_db_type(db_type: &str) -> bool {
    db_type == "maxcompute"
}

/// Parsed MaxCompute-specific options stored in `DbConnectionRecord.options`.
/// AK/SK are read from the record's `username`/`password` (the generic credential
/// slots); the rest lives in this JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MaxcomputeOpts {
    pub endpoint: String,
    pub project: String,
    pub region: Option<String>,
    pub tunnel_endpoint: Option<String>,
    /// Maven coordinate, e.g. "com.aliyun.odps:odps-jdbc:3.9.3". `#[serde(default)]`
    /// so a record that omits it still parses; `maxcompute_opts` fills the standard.
    #[serde(default)]
    pub driver_coord: String,
    /// Parallel download sessions for bulk materialize (spike-validated optimum ~5–6).
    pub concurrency: Option<u32>,
}

impl DbConnectionRecord {
    /// Parse `options` JSON as MaxcomputeOpts, defaulting driver_coord if empty.
    /// `username`/`password` hold AK_ID/AK_SECRET (not part of opts).
    pub fn maxcompute_opts(&self) -> MaxcomputeOpts {
        let mut o: MaxcomputeOpts = self
            .options
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        if o.driver_coord.is_empty() {
            o.driver_coord = "com.aliyun.odps:odps-jdbc:3.9.3".to_string();
        }
        o
    }

    /// Fully-qualified MaxCompute table reference (`project.table`); passes
    /// through if `table` already contains a dot.
    pub fn maxcompute_table_ref(&self, table: &str) -> String {
        let p = self.maxcompute_opts().project;
        if table.contains('.') {
            table.to_string()
        } else {
            format!("{p}.{table}")
        }
    }
}

pub fn create_db_connection(conn: &Connection, r: &DbConnectionRecord) -> Result<(), String> {
    conn.execute(
        "INSERT INTO db_connections (id, name, db_type, host, port, database_name, username, password, ssl_mode, created_at, options)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![r.id, r.name, r.db_type, r.host, r.port, r.database_name, r.username, r.password, r.ssl_mode, r.created_at, r.options],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn update_db_connection(conn: &Connection, r: &DbConnectionRecord) -> Result<(), String> {
    conn.execute(
        "UPDATE db_connections SET name=?, db_type=?, host=?, port=?, database_name=?, username=?, password=?, ssl_mode=?, options=?
         WHERE id=?",
        rusqlite::params![r.name, r.db_type, r.host, r.port, r.database_name, r.username, r.password, r.ssl_mode, r.options, r.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_db_connection(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM db_connections WHERE id = ?", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_db_connections(conn: &Connection) -> Result<Vec<DbConnectionRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, db_type, host, port, database_name, username, password, ssl_mode, created_at, options FROM db_connections ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DbConnectionRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                db_type: row.get(2)?,
                host: row.get(3)?,
                port: row.get(4)?,
                database_name: row.get(5)?,
                username: row.get(6)?,
                password: row.get(7)?,
                ssl_mode: row.get(8)?,
                created_at: row.get(9)?,
                options: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn get_db_connection(conn: &Connection, id: &str) -> Result<Option<DbConnectionRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, db_type, host, port, database_name, username, password, ssl_mode, created_at, options FROM db_connections WHERE id = ?")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([id], |row| {
            Ok(DbConnectionRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                db_type: row.get(2)?,
                host: row.get(3)?,
                port: row.get(4)?,
                database_name: row.get(5)?,
                username: row.get(6)?,
                password: row.get(7)?,
                ssl_mode: row.get(8)?,
                created_at: row.get(9)?,
                options: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    if let Some(r) = rows.next() {
        Ok(Some(r.map_err(|e| e.to_string())?))
    } else {
        Ok(None)
    }
}

pub fn link_connection_to_workspace(conn: &Connection, ws_path: &str, conn_id: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO workspace_connections (workspace_path, connection_id) VALUES (?, ?)",
        [ws_path, conn_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn unlink_connection_from_workspace(conn: &Connection, ws_path: &str, conn_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM workspace_connections WHERE workspace_path = ? AND connection_id = ?",
        [ws_path, conn_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_workspace_connections(conn: &Connection, ws_path: &str) -> Result<Vec<DbConnectionRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT c.id, c.name, c.db_type, c.host, c.port, c.database_name, c.username, c.password, c.ssl_mode, c.created_at, c.options
                  FROM db_connections c
                  JOIN workspace_connections wc ON c.id = wc.connection_id
                  WHERE wc.workspace_path = ?
                  ORDER BY c.created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([ws_path], |row| {
            Ok(DbConnectionRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                db_type: row.get(2)?,
                host: row.get(3)?,
                port: row.get(4)?,
                database_name: row.get(5)?,
                username: row.get(6)?,
                password: row.get(7)?,
                ssl_mode: row.get(8)?,
                created_at: row.get(9)?,
                options: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// A cached table/view entry for an external connection.
#[derive(Debug, Clone)]
pub struct CachedDbTable {
    pub schema: String,
    pub name: String,
    pub kind: String, // "table" | "view"
}

/// Replace the cached table list for `connection_id` with `items` (delete-then-
/// insert in one transaction so callers never see a partial cache).
pub fn save_db_connection_tables(
    conn: &mut Connection,
    connection_id: &str,
    items: &[CachedDbTable],
) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // rusqlite transaction: commits on Ok, rolls back on Err automatically.
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM db_connection_tables WHERE connection_id = ?",
        [connection_id],
    )
    .map_err(|e| e.to_string())?;
    for it in items {
        tx.execute(
            "INSERT OR IGNORE INTO db_connection_tables (connection_id, schema_name, table_name, kind, cached_at)
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![connection_id, it.schema, it.name, it.kind, now],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Read the cached table list for `connection_id`. Empty if never cached.
pub fn list_db_connection_tables_cache(
    conn: &Connection,
    connection_id: &str,
) -> Result<Vec<CachedDbTable>, String> {
    let mut stmt = conn
        .prepare("SELECT schema_name, table_name, kind FROM db_connection_tables WHERE connection_id = ? ORDER BY schema_name, table_name")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([connection_id], |row| {
            Ok(CachedDbTable {
                schema: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Drop the cached table list for `connection_id` (e.g. when the connection is
/// unlinked, so a future re-enable doesn't show a stale list).
pub fn clear_db_connection_tables_cache(conn: &Connection, connection_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM db_connection_tables WHERE connection_id = ?",
        [connection_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// DuckDB identifier-safe alias for an attached connection: `db_` + the
/// connection name reduced to `[A-Za-z0-9_]`. Keep this in sync with
/// `commands::workspace_attach_alias`.
pub fn workspace_attach_alias(name: &str) -> String {
    let safe = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>();
    format!("db_{safe}")
}

/// Build the DuckDB `ATTACH` statement for a connection record, parameterized
/// by `db_type`. Centralizes the per-driver connection-string format so that
/// `attach_one`, `test_connection_impl`, `list_connection_tables_impl`, and
/// `register_database_table` stay in sync.
///
/// - postgres: `host=.. port=.. dbname=.. user=.. password=.. [sslmode=..]`
/// - mysql:    `host=.. port=.. database=.. user=.. password=..`
/// - sqlite:   the local file path (stored in `database_name`); the connection
///             is a single file, so `host/port/user/password` are unused.
pub fn build_attach_sql(r: &DbConnectionRecord, conn_name: &str) -> String {
    if r.db_type == "sqlite" {
        // Escape single quotes in the file path.
        let path = r.database_name.replace('\'', "''");
        format!("ATTACH '{path}' AS {conn_name} (TYPE sqlite);")
    } else if r.db_type == "postgres" {
        let mut conn_str = format!(
            "host={} port={} dbname={} user={} password={}",
            r.host, r.port, r.database_name, r.username, r.password
        );
        if r.ssl_mode != "disable" {
            conn_str.push_str(&format!(" sslmode={}", r.ssl_mode));
        }
        format!("ATTACH '{}' AS {conn_name} (TYPE postgres);", conn_str)
    } else {
        // mysql (and any other network driver falling through)
        format!(
            "ATTACH 'host={} port={} database={} user={} password={}' AS {conn_name} (TYPE mysql);",
            r.host, r.port, r.database_name, r.username, r.password
        )
    }
}

/// ATTACH a single external database connection to a DuckDB session under the
/// alias `db_{safe_name}`. Loads (INSTALL/LOAD) the driver first.
pub fn attach_one(conn: &duckdb::Connection, r: &DbConnectionRecord) -> Result<(), String> {
    // Sidecar DB types (MaxCompute, future generic JDBC) don't ATTACH via
    // DuckDB — connectivity is on-demand through the Java sidecar. Skip ATTACH
    // entirely so workspace link / startup / switch don't try (and fail) to
    // `LOAD maxcompute` / ATTACH a non-DuckDB-extension source.
    if is_sidecar_db_type(&r.db_type) {
        tracing::info!(category = "link", "skipped ATTACH for sidecar connection: {}", r.name);
        return Ok(());
    }
    let load_sql = format!("LOAD {};", r.db_type);
    if conn.execute(&load_sql, []).is_err() {
        let install_sql = format!("INSTALL {};", r.db_type);
        let _ = conn.execute(&install_sql, []);
        conn.execute(&load_sql, []).map_err(|e| format!("加载驱动失败: {e}"))?;
    }

    let conn_name = workspace_attach_alias(&r.name);
    let attach_sql = build_attach_sql(r, &conn_name);

    conn.execute(&attach_sql, [])
        .map(|e| {
            tracing::info!(category = "link", "auto-attached database connection: {} AS {}", r.name, conn_name);
            e
        })
        .map_err(|e| format!("连接数据库失败: {e}"))?;
    Ok(())
}

/// DETACH a single external database connection (by name) from a DuckDB session.
/// Returns Err if the name isn't attached — callers may ignore that case.
pub fn detach_one(conn: &duckdb::Connection, name: &str) -> Result<(), String> {
    let conn_name = workspace_attach_alias(name);
    conn.execute(&format!("DETACH {conn_name};"), [])
        .map_err(|e| e.to_string())?;
    tracing::info!(category = "link", "detached database connection: {} AS {}", name, conn_name);
    Ok(())
}

pub fn attach_workspace_connections(conn: &duckdb::Connection, ws_path: &str) -> Result<(), String> {
    let sqlite = get_db_conn()?;
    let linked = list_workspace_connections(&sqlite, ws_path)?;
    for r in linked {
        if let Err(e) = attach_one(conn, &r) {
            tracing::warn!(category = "link", "auto-attach warning: failed to ATTACH connection {}: {e}", r.name);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// logs: the unified, queryable log store
// ---------------------------------------------------------------------------

/// Insert one log row. Returns the new autoincrement id.
///
/// Called from the tracing `SqliteEmitLayer` (every backend event) and the
/// `append_log` Tauri command (every frontend log). Failures here MUST be
/// non-fatal — logging can never take the app down — so callers swallow the
/// error; we keep the `Result` only for the rare `&Connection` misuse path.
pub fn insert_log(conn: &Connection, rec: &crate::model::LogRecord) -> Result<i64, String> {
    let detail = rec
        .detail
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
    conn.execute(
        "INSERT INTO logs (ts, level, category, message, detail, workspace, task_id)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            rec.ts,
            rec.level.as_str(),
            rec.category,
            rec.message,
            detail,
            rec.workspace,
            rec.task_id,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Query logs with optional filters, newest-first. Used by the console's
/// history load and the future log-analysis module. `limit` defaults to 200 when
/// non-positive.
pub fn query_logs(conn: &Connection, filter: &crate::model::LogFilter) -> Result<Vec<crate::model::LogRecord>, String> {
    let limit = if filter.limit <= 0 { 200 } else { filter.limit };
    let offset = filter.offset.max(0);

    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(cats) = &filter.categories {
        if !cats.is_empty() {
            let placeholders = vec!["?"; cats.len()].join(",");
            where_parts.push(format!("category IN ({placeholders})"));
            for c in cats {
                params.push(Box::new(c.clone()));
            }
        }
    }
    if let Some(levels) = &filter.levels {
        if !levels.is_empty() {
            let placeholders = vec!["?"; levels.len()].join(",");
            where_parts.push(format!("level IN ({placeholders})"));
            for l in levels {
                params.push(Box::new(l.clone()));
            }
        }
    }
    if let Some(from) = filter.from_ts {
        where_parts.push("ts >= ?".to_string());
        params.push(Box::new(from));
    }
    if let Some(to) = filter.to_ts {
        where_parts.push("ts <= ?".to_string());
        params.push(Box::new(to));
    }
    if let Some(kw) = filter.keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        where_parts.push("LOWER(message) LIKE LOWER(?)".to_string());
        params.push(Box::new(format!("%{}%", kw.trim())));
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let sql = format!(
        "SELECT id, ts, level, category, message, detail, workspace, task_id
         FROM logs {where_clause}
         ORDER BY ts DESC, id DESC
         LIMIT ? OFFSET ?"
    );
    params.push(Box::new(limit));
    params.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            let level_str: String = row.get(2)?;
            let detail_str: Option<String> = row.get(5).ok();
            let detail = detail_str
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(&s).ok());
            Ok(crate::model::LogRecord {
                id: Some(row.get(0)?),
                ts: row.get(1)?,
                level: crate::model::LogLevel::from_db_str(&level_str),
                category: row.get(3)?,
                message: row.get(4)?,
                detail,
                workspace: row.get(6).ok(),
                task_id: row.get(7).ok(),
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        if let Ok(rec) = r {
            out.push(rec);
        }
    }
    Ok(out)
}

/// Count logs matching the same filter (without LIMIT/OFFSET), for pagination.
/// Reserved for the future log-analysis module's paginated view.
#[allow(dead_code)]
pub fn count_logs(conn: &Connection, filter: &crate::model::LogFilter) -> Result<i64, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(cats) = &filter.categories {
        if !cats.is_empty() {
            let placeholders = vec!["?"; cats.len()].join(",");
            where_parts.push(format!("category IN ({placeholders})"));
            for c in cats {
                params.push(Box::new(c.clone()));
            }
        }
    }
    if let Some(levels) = &filter.levels {
        if !levels.is_empty() {
            let placeholders = vec!["?"; levels.len()].join(",");
            where_parts.push(format!("level IN ({placeholders})"));
            for l in levels {
                params.push(Box::new(l.clone()));
            }
        }
    }
    if let Some(from) = filter.from_ts {
        where_parts.push("ts >= ?".to_string());
        params.push(Box::new(from));
    }
    if let Some(to) = filter.to_ts {
        where_parts.push("ts <= ?".to_string());
        params.push(Box::new(to));
    }
    if let Some(kw) = filter.keyword.as_ref().filter(|s| !s.trim().is_empty()) {
        where_parts.push("LOWER(message) LIKE LOWER(?)".to_string());
        params.push(Box::new(format!("%{}%", kw.trim())));
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let sql = format!("SELECT COUNT(*) FROM logs {where_clause}");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    stmt.query_row(param_refs.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())
}

/// Delete logs. `before = None` clears ALL logs; `Some(ts)` deletes rows with
/// `ts < before` (used for retention, e.g. "older than 7 days").
pub fn clear_logs(conn: &Connection, before: Option<i64>) -> Result<(), String> {
    match before {
        Some(ts) => {
            conn.execute("DELETE FROM logs WHERE ts < ?", [ts])
                .map_err(|e| e.to_string())?;
        }
        None => {
            conn.execute("DELETE FROM logs", []).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Current Unix-ms timestamp. Local helper mirroring the inline calls elsewhere
/// in this file; kept here so log helpers are self-contained.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}


