//! Application-wide singleton state, owned by `tauri::State`.
//!
//! ## Persistence model
//!
//! The DuckDB *session* connection is **in-memory** (not a file). The sole
//! persistent store for tables/views is a **per-workspace DuckLake**:
//!   `<workspace>/lake.ducklake`  (catalog) + `<workspace>/lake_data/` (parquet).
//!
//! On startup we attach the default workspace's lake; the frontend switches to
//! the user's chosen workspace via `register_workspace_sources`, which rebuilds
//! the session connection and re-attaches that workspace's lake.
//!
//! Business-level mappings (which file → which table, how it is stored) live in
//! the global SQLite DB (`~/.lakemind/lakemind.db`), not in this struct. The
//! in-memory `sources` cache here is just a read-through mirror for the
//! *current* workspace.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::db::get_home_dir;
use crate::duckdb::lake;
use crate::error::AppResult;
use crate::model::SourceTable;

/// One DuckDB session connection + a cache of the current workspace's sources.
#[derive(Clone)]
pub struct AppState {
    /// In-memory DuckDB session with the current workspace's DuckLake attached
    /// as the default catalog (`USE lake`). Rebuilt on workspace switch.
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// Cache of the current workspace's registered sources. The authoritative
    /// source of truth is the global SQLite `sources` table; this is a
    /// convenience mirror so the UI doesn't re-query on every render.
    pub sources: Arc<Mutex<Vec<SourceTable>>>,
    /// Absolute path of the workspace directory currently attached.
    pub workspace_dir: Arc<Mutex<PathBuf>>,
    /// The workspace key (`workspaces.path`) currently attached, e.g. "DefaultProject".
    /// Used to key into the SQLite `sources` mapping table.
    pub workspace_path: Arc<Mutex<String>>,
}

impl AppState {
    /// Build a fresh in-memory connection, load ducklake, and attach the lake at
    /// `ws_dir` as the default catalog. Applies sane PRAGMAs.
    pub fn open_workspace(ws_dir: &std::path::Path) -> AppResult<duckdb::Connection> {
        let conn = duckdb::Connection::open_in_memory()?;
        // threads=1: DuckLake's SQLite catalog is single-writer. With threads>1
        // DuckDB parallelizes writes to the catalog's SQLite metadata and hits
        // "database is locked". Single-thread is fine for local analysis.
        let _ = conn.execute_batch(
            "PRAGMA memory_limit='4GB';\n\
             PRAGMA threads=1;",
        );
        lake::ensure_ducklake_loaded(&conn)?;
        lake::attach_workspace_lake(&conn, ws_dir)?;
        Ok(conn)
    }

    /// Initialize against the default workspace's lake at
    /// `~/.lakemind/DefaultProject/`. Used only at startup; the frontend then
    /// switches via `register_workspace_sources`.
    pub fn new() -> AppResult<Self> {
        let ws = default_workspace_dir();
        let conn = Self::open_workspace(&ws)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            sources: Arc::new(Mutex::new(Vec::new())),
            workspace_dir: Arc::new(Mutex::new(ws)),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
        })
    }
}

/// The default workspace directory: `~/.lakemind/DefaultProject/`.
fn default_workspace_dir() -> PathBuf {
    let mut home = get_home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.push(".lakemind");
    home.push("DefaultProject");
    home
}

impl Default for AppState {
    fn default() -> Self {
        // A failed startup (missing ducklake extension, bad home dir) is fatal.
        Self::new().expect("failed to open default workspace lake")
    }
}
