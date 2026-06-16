//! Application-wide singleton state, owned by `tauri::State`.
//!
//! The DuckDB connection is a **persistent file-backed database** that lives
//! in its workspace directory at `<workspace>/lake.duckdb`. Each workspace has
//! its own isolated set of materialized tables that survive restarts. Tables
//! are restored on workspace load via `register_workspace_sources` (which
//! reuses existing tables) / `list_duckdb_tables`.
//!
//! Workspace switching is handled by `commands::switch_workspace_db`, which
//! swaps the connection under this mutex. At startup we open the default
//! workspace's lake file directly (no more in-memory DB — that lost everything
//! on restart).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::db::get_home_dir;
use crate::model::SourceTable;

/// One DuckDB connection (per-workspace, persistent) + the SOURCE registry.
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// Registered SOURCE tables, in insertion order. Backs `list_sources`.
    /// In-memory cache for the *current* workspace; the authoritative source
    /// of truth is the DuckDB lake file itself.
    pub sources: Arc<Mutex<Vec<SourceTable>>>,
}

impl AppState {
    /// Open (or create) a persistent DuckDB lake at `lake_file` and apply sane
    /// PRAGMAs. The parent directory is created if missing.
    pub fn open_persistent(lake_file: &std::path::Path) -> Result<duckdb::Connection, duckdb::Error> {
        if let Some(parent) = lake_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = duckdb::Connection::open(lake_file)?;
        let _ = conn.execute_batch(
            "PRAGMA memory_limit='4GB';\n\
             PRAGMA threads=8;",
        );
        Ok(conn)
    }

    /// Initialize against the default workspace's lake file
    /// (`~/.lakemind/DefaultProject/lake.duckdb`). Used at startup; the
    /// frontend switches to the user's chosen workspace via
    /// `register_workspace_sources`, which calls `switch_workspace_db`.
    pub fn new() -> Result<Self, duckdb::Error> {
        let lake_file = default_workspace_lake_file();
        let conn = Self::open_persistent(&lake_file)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            sources: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

/// The default workspace's lake file path: `~/.lakemind/DefaultProject/lake.duckdb`.
fn default_workspace_lake_file() -> PathBuf {
    let mut home = get_home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.push(".lakemind");
    home.push("DefaultProject");
    let _ = std::fs::create_dir_all(&home);
    home.join("lake.duckdb")
}

impl Default for AppState {
    fn default() -> Self {
        // A failed DB open at startup is fatal; panicking here is acceptable.
        Self::new().expect("failed to open persistent DuckDB lake")
    }
}
