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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::db::get_home_dir;
use crate::duckdb::lake;
use crate::error::AppResult;
use crate::model::SourceTable;

/// User's decision on whether a pending DDL operation should proceed.
#[derive(Debug, Clone)]
pub struct ConfirmDecision {
    pub approved: bool,
}

/// A DDL tool invocation parked in "变更前确认" mode, waiting for the user to
/// approve or cancel it from the UI. The `oneshot::Sender` is used to resume the
/// blocked tool `call()`.
pub struct PendingConfirmation {
    pub tx: oneshot::Sender<ConfirmDecision>,
}

/// One DuckDB session connection + a cache of the current workspace's sources.
#[derive(Clone)]
pub struct AppState {
    /// In-memory DuckDB session with the current workspace's DuckLake attached
    /// as the default catalog (`USE lake`). Rebuilt on workspace switch.
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// Pre-extracted interrupt handle for the current connection. Stored
    /// separately so we can fire `interrupt()` without locking `conn` — which
    /// is critical when a previous hard-timeout query's background thread is
    /// still holding the conn lock.
    pub interrupt_handle: Arc<std::sync::Mutex<std::sync::Arc<duckdb::InterruptHandle>>>,
    /// Cache of the current workspace's registered sources. The authoritative
    /// source of truth is the global SQLite `sources` table; this is a
    /// convenience mirror so the UI doesn't re-query on every render.
    pub sources: Arc<Mutex<Vec<SourceTable>>>,
    /// Absolute path of the workspace directory currently attached.
    pub workspace_dir: Arc<Mutex<PathBuf>>,
    /// The workspace key (`workspaces.path`) currently attached, e.g. "DefaultProject".
    /// Used to key into the SQLite `sources` mapping table.
    pub workspace_path: Arc<Mutex<String>>,
    /// DDL tool calls parked in "变更前确认" mode, keyed by `{task_id}:{tool_call_id}`.
    /// Each entry holds a oneshot sender that resumes the blocked tool once the
    /// user approves or cancels from the UI (via `resolve_tool_confirmation`).
    pub pending_confirmations: Arc<Mutex<HashMap<String, PendingConfirmation>>>,
    /// Aborted task IDs. Inserted by `abort_chat`; checked by `run_stream_loop`
    /// each iteration so a long-running stream stops promptly.
    pub aborted_tasks: Arc<Mutex<HashSet<String>>>,
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

    pub fn new() -> AppResult<Self> {
        let ws = default_workspace_dir();
        // Open a blank connection instantly. Sane PRAGMAs.
        let conn = duckdb::Connection::open_in_memory()?;
        let _ = conn.execute_batch(
            "PRAGMA memory_limit='4GB';\n\
             PRAGMA threads=1;",
        );
        let ih = conn.interrupt_handle();

        let conn_arc = Arc::new(Mutex::new(conn));
        let state = Self {
            conn: conn_arc.clone(),
            interrupt_handle: Arc::new(std::sync::Mutex::new(ih)),
            sources: Arc::new(Mutex::new(Vec::new())),
            workspace_dir: Arc::new(Mutex::new(ws.clone())),
            workspace_path: Arc::new(Mutex::new("DefaultProject".to_string())),
            pending_confirmations: Arc::new(Mutex::new(HashMap::new())),
            aborted_tasks: Arc::new(Mutex::new(HashSet::new())),
        };

        // Spawn a background task to asynchronously initialize the extensions and attach DefaultProject
        let conn_clone = conn_arc;
        let ws_clone = ws;
        tauri::async_runtime::spawn(async move {
            let res = tauri::async_runtime::spawn_blocking(move || -> AppResult<()> {
                let guard = conn_clone.blocking_lock();
                lake::ensure_ducklake_loaded(&guard)?;
                lake::attach_workspace_lake(&guard, &ws_clone)?;
                let _ = crate::db::attach_workspace_connections(&guard, "DefaultProject");
                Ok(())
            }).await;
            if let Err(e) = res {
                eprintln!("Background workspace initialization panicked: {e}");
            } else if let Ok(Err(e)) = res {
                eprintln!("Background workspace initialization failed: {e}");
            }
        });

        Ok(state)
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
