//! Application-wide singleton state, owned by `tauri::State`.
//!
//! M1 is a single workspace, so there is exactly one in-memory DuckDB
//! connection guarded by a tokio mutex. All command implementations acquire
//! the lock and dispatch the blocking DuckDB call onto `spawn_blocking` so a
//! long query never stalls Tauri's async runtime.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::model::SourceTable;

/// One DuckDB connection + the SOURCE registry. Cloning is cheap (Arc).
#[derive(Clone)]
pub struct AppState {
    pub conn: Arc<Mutex<duckdb::Connection>>,
    /// Registered SOURCE tables, in insertion order. Backs `list_sources`.
    pub sources: Arc<Mutex<Vec<SourceTable>>>,
}

impl AppState {
    /// Open an in-memory DuckDB database and set sane memory limits.
    pub fn new() -> Result<Self, duckdb::Error> {
        let conn = duckdb::Connection::open_in_memory()?;
        // Cap DuckDB's working memory so we don't balloon on a careless
        // `SELECT *` over a 50GB file. 4GB is a conservative laptop default;
        // DuckDB will spill to disk rather than OOM. M2 will expose this.
        conn.execute_batch(
            "PRAGMA memory_limit='4GB';\n\
             PRAGMA threads=8;",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            sources: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

impl Default for AppState {
    fn default() -> Self {
        // Used by the state initializer; a failed DB open is fatal at startup
        // so panicking here is acceptable.
        Self::new().expect("failed to open in-memory DuckDB connection")
    }
}
