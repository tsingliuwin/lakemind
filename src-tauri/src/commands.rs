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
///
/// Concurrency model: the pure-FS scan runs OUTSIDE the connection lock, so a
/// slow directory walk never blocks `execute_sql`. Only the `CREATE VIEW` +
/// schema/row-count introspection happen inside the (short) lock window. The
/// Delta extension is loaded lazily and only if a Delta source is actually
/// present — offline users with no Delta data are never blocked on the network.
#[tauri::command]
pub async fn register_folder(path: String, state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    let conn = state.conn.clone();
    let sources = state.sources.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let root = PathBuf::from(&path);
        if !root.exists() {
            return Err(AppError::new(format!("路径不存在: {path}")));
        }

        // 1. Pure filesystem scan — no DuckDB lock needed. This is the slow
        //    part on big trees and must not block concurrent queries.
        let entries = scan::scan_path(&root);

        // 2. Short critical section: load delta only if needed, then create
        //    the views + introspect. Hold the lock only across these DB ops.
        let guard = conn.blocking_lock();
        let needs_delta = entries
            .iter()
            .any(|e| e.kind == crate::model::SourceKind::Delta);
        if needs_delta {
            // Best-effort: try INSTALL then LOAD. Offline failure degrades
            // gracefully (those Delta sources just fail to register, others
            // still succeed). Never panics or hangs on the network.
            let _ = guard.execute("INSTALL delta;", []);
            let _ = guard.execute("LOAD delta;", []);
        }

        let mut created = Vec::with_capacity(entries.len());
        for e in &entries {
            match register::register(&guard, e) {
                Ok(t) => created.push(t),
                // A single failing source (e.g. a malformed CSV) must not abort
                // the whole scan; skip it but log the reason.
                Err(err) => eprintln!("skip source {}: {err}", e.label),
            }
        }
        drop(guard); // release the connection lock before touching the registry

        // 3. Merge into the registry (separate lock, after DB lock released).
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

fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()
}

fn resolve_workspace_dir(workspace: &str) -> Result<PathBuf, String> {
    if workspace.starts_with("~/") || workspace == "~" {
        let mut home = get_home_dir().ok_or_else(|| "Could not find home directory".to_string())?;
        if workspace.len() > 2 {
            home.push(&workspace[2..]);
        }
        return Ok(home);
    }
    let path = PathBuf::from(workspace);
    if path.is_absolute() {
        return Ok(path);
    }
    
    let mut home = get_home_dir().ok_or_else(|| "Could not find home directory".to_string())?;
    home.push(".lakemind");
    home.push(workspace);
    Ok(home)
}

fn select_directory_native() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose folder with prompt \"请选择工作区目录\")")
            .output()
            .ok()?;
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(path_str);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let ps_script = "[System.Reflection.Assembly]::LoadWithPartialName('System.windows.forms') | Out-Null; $g = New-Object System.Windows.Forms.FolderBrowserDialog; $g.ShowDialog() | Out-Null; $g.SelectedPath";
        let output = Command::new("powershell")
            .arg("-Command")
            .arg(ps_script)
            .output()
            .ok()?;
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(path_str);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let output = Command::new("zenity")
            .arg("--file-selection")
            .arg("--directory")
            .arg("--title=请选择工作区目录")
            .output()
            .ok()?;
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(path_str);
            }
        }
    }
    None
}

/// Open a native platform directory picker and return the selected directory path.
#[tauri::command]
pub async fn select_directory() -> Result<Option<String>, String> {
    Ok(select_directory_native())
}

fn copy_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_recursive(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Recursively total the byte size of a path (file or directory tree).
fn path_total_size(p: &std::path::Path) -> u64 {
    let md = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if md.is_file() {
        return md.len();
    }
    let mut total: u64 = 0;
    for entry in walkdir::WalkDir::new(p).follow_links(false) {
        if let Ok(e) = entry {
            if e.file_type().is_file() {
                if let Ok(m) = e.metadata() {
                    total += m.len();
                }
            }
        }
    }
    total
}

/// Above this threshold (bytes) a dropped folder/file is registered IN-PLACE
/// (zero-copy) instead of being physically copied into the workspace. Prevents
/// a 50GB lake from being duplicated onto the local disk on import.
const WORKSPACE_COPY_THRESHOLD: u64 = 200 * 1024 * 1024; // 200 MB

/// Bring a folder/file into the workspace, then scan + register as views.
///
/// Two modes by size:
/// - **small (≤ threshold)**: physically copy into the workspace directory
///   (matches the "工作区放 CSV" product intent for manageable files).
/// - **large (> threshold)**: register IN-PLACE via zero-copy CREATE VIEW to
///   avoid duplicating gigabytes on disk.
#[tauri::command]
pub async fn import_file_to_workspace(
    workspace: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    let conn = state.conn.clone();
    let sources = state.sources.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let src_path = PathBuf::from(&path);
        if !src_path.exists() {
            return Err(AppError::new(format!("源路径不存在: {path}")));
        }

        // Decide copy-vs-inplace based on total size.
        let total = path_total_size(&src_path);
        let ws_dir = resolve_workspace_dir(&workspace).map_err(AppError::new)?;
        std::fs::create_dir_all(&ws_dir)
            .map_err(|e| AppError::new(format!("无法创建工作区目录: {e}")))?;

        let target_path = if total > WORKSPACE_COPY_THRESHOLD {
            // Large: register in-place, do NOT copy.
            src_path
        } else {
            let canonical_src = src_path.canonicalize().unwrap_or_else(|_| src_path.clone());
            let canonical_ws = ws_dir.canonicalize().unwrap_or_else(|_| ws_dir.clone());
            if canonical_src.starts_with(&canonical_ws) {
                // Already inside the workspace dir.
                src_path
            } else {
                let file_name = src_path
                    .file_name()
                    .ok_or_else(|| AppError::new("无效文件名".to_string()))?;
                let dst_path = ws_dir.join(file_name);
                copy_recursive(&src_path, &dst_path)
                    .map_err(|e| AppError::new(format!("拷贝到工作区失败: {e}")))?;
                dst_path
            }
        };

        // Scan (FS, no lock) then short critical section for CREATE VIEW.
        let entries = scan::scan_path(&target_path);
        let guard = conn.blocking_lock();
        let needs_delta = entries
            .iter()
            .any(|e| e.kind == crate::model::SourceKind::Delta);
        if needs_delta {
            let _ = guard.execute("INSTALL delta;", []);
            let _ = guard.execute("LOAD delta;", []);
        }
        let mut created = Vec::with_capacity(entries.len());
        for e in &entries {
            match register::register(&guard, e) {
                Ok(t) => created.push(t),
                Err(err) => eprintln!("skip source {}: {err}", e.label),
            }
        }
        drop(guard);

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

#[derive(serde::Serialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Read the direct children of a workspace folder (no git status — that was a
/// misplaced code-IDE concern; LakeMind analyzes data files, not source repos).
#[tauri::command]
pub async fn read_directory(path: String) -> Result<Vec<FileItem>, String> {
    let resolved_path = resolve_workspace_dir(&path)?;
    if !resolved_path.exists() {
        return Err(format!("目录不存在: {}", resolved_path.display()));
    }

    let mut items = Vec::new();
    let entries = std::fs::read_dir(&resolved_path)
        .map_err(|e| format!("读取目录失败: {e}"))?;
    for entry in entries {
        if let Ok(entry) = entry {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = p.is_dir();
            items.push(FileItem {
                name,
                path: p.to_string_lossy().to_string(),
                is_dir,
            });
        }
    }
    // Sort: directories first, then alphabetical
    items.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });
    Ok(items)
}
