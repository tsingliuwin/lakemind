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

/// Copy a folder or file to the active workspace directory, then scan and register as views.
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
            return Err(AppError::new(format!("Source path does not exist: {path}")));
        }

        // 1. Resolve workspace dir
        let ws_dir = resolve_workspace_dir(&workspace)
            .map_err(|e| AppError::new(e))?;
        
        // Ensure workspace dir exists
        std::fs::create_dir_all(&ws_dir)
            .map_err(|e| AppError::new(format!("Failed to create workspace directory: {e}")))?;

        // 2. Determine target path inside workspace
        let file_name = src_path.file_name()
            .ok_or_else(|| AppError::new("Invalid file name".to_string()))?;
        let dst_path = ws_dir.join(file_name);

        // 3. Copy if not already inside the workspace dir
        let canonical_src = src_path.canonicalize().ok().unwrap_or_else(|| src_path.clone());
        let canonical_ws = ws_dir.canonicalize().ok().unwrap_or_else(|| ws_dir.clone());
        
        let target_path = if canonical_src.starts_with(&canonical_ws) {
            // Already inside workspace directory, use src_path
            src_path
        } else {
            // Copy to workspace
            copy_recursive(&src_path, &dst_path)
                .map_err(|e| AppError::new(format!("Failed to copy file/folder to workspace: {e}")))?;
            dst_path
        };

        // 4. Scan and register the copied folder/file in DuckDB
        let guard = conn.blocking_lock();
        let _ = guard.execute("INSTALL delta; LOAD delta;", []);

        let entries = scan::scan_path(&target_path);
        let mut created = Vec::with_capacity(entries.len());
        for e in &entries {
            match register::register(&guard, e) {
                Ok(t) => created.push(t),
                Err(err) => eprintln!("skip source {}: {err}", e.label),
            }
        }

        // Merge into the registry (dedupe by view name)
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
    pub is_modified: bool,
}

fn get_git_modified_files(dir: &std::path::Path) -> Option<std::collections::HashSet<String>> {
    use std::process::Command;
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut modified = std::collections::HashSet::new();
    for line in stdout.lines() {
        if line.len() > 3 {
            let rel_file = line[3..].trim();
            if let Ok(abs_path) = dir.join(rel_file).canonicalize() {
                modified.insert(abs_path.to_string_lossy().to_string());
            } else {
                modified.insert(dir.join(rel_file).to_string_lossy().to_string());
            }
        }
    }
    Some(modified)
}

fn is_path_modified(p: &std::path::Path, modified_files: &std::collections::HashSet<String>) -> bool {
    let p_str = p.to_string_lossy().to_string();
    let resolved_p_str = p.canonicalize().ok()
        .map(|cp| cp.to_string_lossy().to_string())
        .unwrap_or(p_str);

    if modified_files.contains(&resolved_p_str) {
        return true;
    }
    if p.is_dir() {
        let prefix = if resolved_p_str.ends_with('/') || resolved_p_str.ends_with('\\') {
            resolved_p_str.clone()
        } else {
            #[cfg(target_os = "windows")]
            { format!("{}\\", resolved_p_str) }
            #[cfg(not(target_os = "windows"))]
            { format!("{}/", resolved_p_str) }
        };
        for m in modified_files {
            if m.starts_with(&prefix) {
                return true;
            }
        }
    }
    false
}

/// Read the direct children of a workspace folder and check their git modification status
#[tauri::command]
pub async fn read_directory(path: String) -> Result<Vec<FileItem>, String> {
    let resolved_path = resolve_workspace_dir(&path)?;
    if !resolved_path.exists() {
        return Err(format!("Directory does not exist: {}", resolved_path.display()));
    }
    
    // Get git modified files
    let modified_files = get_git_modified_files(&resolved_path).unwrap_or_default();

    let mut items = Vec::new();
    let entries = std::fs::read_dir(&resolved_path)
        .map_err(|e| format!("Failed to read directory: {}", e))?;
    for entry in entries {
        if let Ok(entry) = entry {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = p.is_dir();
            
            // Check modified status
            let has_modified = is_path_modified(&p, &modified_files);
            
            items.push(FileItem {
                name,
                path: p.to_string_lossy().to_string(),
                is_dir,
                is_modified: has_modified,
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
