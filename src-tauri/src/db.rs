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

/// Establish connection to sqlite database
pub fn get_db_conn() -> Result<Connection, String> {
    let db_path = get_db_path()?;
    Connection::open(&db_path).map_err(|e| format!("Failed to open SQLite database: {e}"))
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
}

/// Insert or update a source mapping (keyed by `(workspace_path, table_name)`).
pub fn upsert_source(conn: &Connection, ws_path: &str, r: &SourceRecord) -> Result<(), String> {
    let keys = serde_json::to_string(&r.partition_keys).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO sources (workspace_path, table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_path, table_name) DO UPDATE SET
            label=excluded.label, kind=excluded.kind, storage=excluded.storage,
            file_path=excluded.file_path, scan_path=excluded.scan_path,
            partition_keys=excluded.partition_keys",
        rusqlite::params![ws_path, r.table_name, r.label, r.kind, r.storage, r.file_path, r.scan_path, keys, r.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// All source mappings for one workspace, in creation order.
pub fn list_sources(conn: &Connection, ws_path: &str) -> Result<Vec<SourceRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at
             FROM sources WHERE workspace_path = ? ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([ws_path], |row| {
            let keys_json: String = row.get(6)?;
            let partition_keys: Vec<String> =
                serde_json::from_str(&keys_json).unwrap_or_default();
            Ok(SourceRecord {
                table_name: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                storage: row.get(3)?,
                file_path: row.get(4)?,
                scan_path: row.get(5)?,
                partition_keys,
                created_at: row.get(7)?,
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
            "SELECT table_name, label, kind, storage, file_path, scan_path, partition_keys, created_at
             FROM sources WHERE workspace_path = ? AND table_name = ?",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(rusqlite::params![ws_path, table_name], |row| {
            let keys_json: String = row.get(6)?;
            let partition_keys: Vec<String> =
                serde_json::from_str(&keys_json).unwrap_or_default();
            Ok(SourceRecord {
                table_name: row.get(0)?,
                label: row.get(1)?,
                kind: row.get(2)?,
                storage: row.get(3)?,
                file_path: row.get(4)?,
                scan_path: row.get(5)?,
                partition_keys,
                created_at: row.get(7)?,
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
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create tasks table: {e}"))?;

    // Migrate tasks table to add model_id column if it doesn't exist
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN model_id TEXT;", []);

    // sources: the file ↔ table ↔ storage mapping (NEW)
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
            PRIMARY KEY (workspace_path, table_name),
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create sources table: {e}"))?;

    // config: key/value user settings (NEW)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("Failed to create config table: {e}"))?;

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
