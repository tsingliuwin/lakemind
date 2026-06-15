//! Tauri command handlers — the M1 wire surface.
//!
//! Four commands, each a thin async wrapper that locks the shared DuckDB
//! connection and dispatches the blocking work onto `spawn_blocking`:
//!
//! | Command          | Purpose                                            |
//! |------------------|----------------------------------------------------|
//! | `register_folder`| Scan a dropped path and create all SOURCE views    |
//! | `list_sources`   | Return the current SOURCE registry                 |
//! | `describe_table` | Column metadata for one view                       |
//! | `execute_sql`    | Run an ad-hoc SELECT (row-capped) and return rows  |

use std::path::PathBuf;

use tauri::State;

use crate::duckdb::{execute, register, scan, schema};
use crate::error::{AppError, AppResult};
use crate::model::{ColumnInfo, SourceTable, SqlResult};
use crate::state::AppState;

/// Scan a folder (or single file) and register every detected SOURCE as a
/// DuckDB VIEW. Returns the freshly registered tables.
#[tauri::command]
pub async fn register_folder(path: String, state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    // Clone the Arcs out of `state` so the closures can be `'static` and move
    // onto the blocking thread pool. `tauri::State` itself borrows, so it
    // cannot cross the spawn boundary directly.
    let conn = state.conn.clone();
    let sources = state.sources.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let root = PathBuf::from(&path);
        if !root.exists() {
            return Err(AppError::new(format!("path does not exist: {path}")));
        }

        let guard = conn.blocking_lock();

        // Load the Delta extension up front; no-op if unused. `bundled`
        // DuckDB ships the parquet/csv/json readers built-in, but delta needs
        // the autoloaded extension. We let DuckDB fetch it from its repo
        // (online); offline users get a clear error from the register step.
        let _ = guard.execute("INSTALL delta; LOAD delta;", []);

        let entries = scan::scan_path(&root);
        let mut created = Vec::with_capacity(entries.len());
        for e in &entries {
            match register::register(&guard, e) {
                Ok(t) => created.push(t),
                // A single failing source (e.g. a malformed CSV) must not abort
                // the whole scan; skip it and surface nothing — list_sources
                // will reflect only the successful views.
                Err(err) => eprintln!("skip source {}: {err}", e.label),
            }
        }

        // Merge into the registry (dedupe by view name).
        let mut registry = sources.blocking_lock();
        for t in &created {
            registry.retain(|existing| existing.name != t.name);
            registry.push(t.clone());
        }
        Ok(created)
    })
    .await;

    match join {
        Ok(Ok(created)) => Ok(created),
        Ok(Err(e)) => Err(e.to_string()),
        Err(join_err) => Err(format!("task join error: {join_err}")),
    }
}

/// Return all currently registered SOURCE tables.
#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    let registry = state.sources.lock().await;
    Ok(registry.clone())
}

/// Column metadata for a single registered view.
#[tauri::command]
pub async fn describe_table(name: String, state: State<'_, AppState>) -> Result<Vec<ColumnInfo>, String> {
    run_blocking(state, move |conn| {
        let view = sanitize_ident(&name)?;
        schema::describe_view(conn, &view)
    })
    .await
}

/// Run an ad-hoc SELECT and return a row-capped [`SqlResult`].
#[tauri::command]
pub async fn execute_sql(sql: String, row_cap: Option<usize>, state: State<'_, AppState>) -> Result<SqlResult, String> {
    run_blocking(state, move |conn| execute::run_query(conn, sql.trim(), row_cap)).await
}

// --- internals -------------------------------------------------------------

/// Lock the connection on a blocking thread, then run `f`. This keeps the
/// async runtime free while a long DuckDB query is executing.
async fn run_blocking<T, F>(state: State<'_, AppState>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&duckdb::Connection) -> AppResult<T> + Send + 'static,
{
    let conn = state.conn.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        let guard = conn.blocking_lock();
        f(&guard)
    })
    .await
    {
        Ok(inner) => inner.map_err(|e| e.to_string()),
        Err(join_err) => Err(format!("task join error: {join_err}")),
    }
}

/// Reject anything that would let a caller break out of a quoted identifier.
/// We only ever embed sanitized names, but defence in depth is cheap.
fn sanitize_ident(name: &str) -> Result<String, AppError> {
    if name.is_empty() || name.contains('"') || name.contains('\0') {
        return Err(AppError::new("invalid table name"));
    }
    Ok(name.to_string())
}
