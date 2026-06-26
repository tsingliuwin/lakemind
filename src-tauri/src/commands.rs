//! Tauri command handlers.
//!
//! Groups:
//! * **Lake**  — import / register sources into the workspace's DuckLake, with a
//!   configurable zero-copy threshold (small → materialized TABLE, large → VIEW).
//!   Every registration is mirrored into the SQLite `sources` mapping table so
//!   the file↔table↔storage relationship is persistent and queryable.
//! * **Query** — describe / execute SQL (run inside `USE lake`, so `FROM s_x` works).
//! * **Config** — get/set user settings (e.g. the zero-copy threshold).
//! * **FS** — native directory picker, read a workspace folder.
//! * **Workspace / Task** — registry + per-workspace SQL/chat task persistence.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{State, Emitter};
use tokio::sync::Mutex;

use crate::db::{self, SourceRecord};
use crate::duckdb::{execute, naming, register, scan, schema};
use crate::error::{AppError, AppResult};
use crate::model::{SourceKind, SourceTable, SqlResult, StorageKind};
use crate::state::AppState;

// ===========================================================================
// Query commands
// ===========================================================================

/// Column metadata for a single registered table/view.
#[tauri::command]
pub async fn describe_table(name: String, state: State<'_, AppState>) -> Result<Vec<crate::model::ColumnInfo>, String> {
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

/// Return the in-memory source cache for the current workspace.
#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    Ok(state.sources.lock().await.clone())
}

/// SQL fragments to discover user-created tables/views inside the attached lake
/// (surfaces `custom` sources not tracked in the `sources` mapping). Queried
/// separately so one failing (e.g. `duckdb_views()` under DuckLake) cannot drop
/// the registered sources we already collected.
const CUSTOM_TABLE_SQLS: [&str; 2] = [
    "SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal",
    "SELECT view_name as table_name FROM duckdb_views() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal",
];

/// Fast table listing for instant UI render on startup / workspace switch.
///
/// Unlike `register_workspace_sources` (which scans the filesystem + syncs) and
/// `list_duckdb_tables` (which counts rows), this only:
///   * ensures the workspace's lake is attached (skipping re-attach when already
///     on this workspace), and
///   * reads table names + column structure (describe_view is a LIMIT-0 plan).
///
/// Row counts are left as `None` — they're filled in later by the async
/// `list_duckdb_tables`. File scanning + sync still run in the background via
/// `register_workspace_sources`. This lets the table list appear in ~instantly
/// instead of blocking on a full directory walk + row counts.
#[tauri::command]
pub async fn list_tables_fast(
    workspace_path: String,
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    // Attach this workspace's lake if we're not already on it. On a fresh launch
    // the default workspace is already attached (AppState::new), so this is a
    // no-op then; it only re-attaches on an explicit workspace switch.
    {
        let current = state.workspace_path.lock().await.clone();
        if current != workspace_path {
            let ws_dir = resolve_workspace_dir(&workspace_path)?;
            switch_workspace_lake(&state, workspace_path.clone(), ws_dir).await?;
        }
    }

    let ws_path = workspace_path;
    run_blocking(state, move |conn| {
        let sqlite = db::get_db_conn()?;
        let records = db::list_sources(&sqlite, &ws_path)?;

        let mut result = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for rec in &records {
            known.insert(rec.table_name.clone());
            // describe_view only; no count_rows.
            match build_source_table_from_record(conn, rec) {
                Ok(t) => result.push(t),
                Err(e) => eprintln!("fast list: skip source {} (metadata failed): {}", rec.table_name, e),
            }
        }

        // Custom tables/views from the lake catalog (no row count).
        for sql in CUSTOM_TABLE_SQLS {
            let Ok(mut stmt) = conn.prepare(sql) else { continue };
            let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else { continue };
            for name in rows.flatten() {
                if known.contains(&name) { continue; }
                known.insert(name.clone());
                let cols = schema::describe_view(conn, &name).unwrap_or_default();
                result.push(SourceTable {
                    name: name.clone(),
                    label: name,
                    kind: SourceKind::View,
                    storage: StorageKind::Custom,
                    path: String::new(),
                    scan_path: String::new(),
                    partition_keys: Vec::new(),
                    row_count_estimate: None,
                    columns: cols,
                });
            }
        }
        Ok(result)
    })
    .await
}

/// All tables/views for the current workspace: registered sources (from the
/// SQLite `sources` mapping, enriched with live column metadata) plus any custom
/// tables the user created via SQL (`storage = custom`). Best-effort throughout
/// — a missing lake object or a flaky catalog query never drops the set of
/// registered sources we already know about.
#[tauri::command]
pub async fn list_duckdb_tables(state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    let ws_path = state.workspace_path.lock().await.clone();
    // The source cache (hydrated by `register_workspace_sources`) already carries
    // every source's columns/metadata without row counts. We reuse it to avoid
    // re-running describe_view, and only fill in the (slow) row counts here.
    let cached: Vec<SourceTable> = state.sources.lock().await.clone();
    run_blocking(state, move |conn| {
        let sqlite = db::get_db_conn()?;
        let records = db::list_sources(&sqlite, &ws_path)?;

        // Index the cache by table name for O(1) lookup.
        let cache_by_name: HashMap<String, &SourceTable> =
            cached.iter().map(|t| (t.name.clone(), t)).collect();

        // 1. Registered sources (authoritative). Reuse the cached metadata
        //    (columns etc.) and only compute the row count — the part the fast
        //    initial render skipped.
        let mut result = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for rec in &records {
            known.insert(rec.table_name.clone());
            let mut t = if let Some(c) = cache_by_name.get(&rec.table_name) {
                (*c).clone()
            } else {
                match build_source_table_from_record(conn, rec) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("list: skip source {} (metadata failed): {}", rec.table_name, e);
                        continue;
                    }
                }
            };
            t.row_count_estimate = count_rows(conn, &t.name);
            result.push(t);
        }

        // 2. Custom tables/views not tracked in `sources`. Best-effort: each SQL
        //    is prepared/run independently; a failure (e.g. duckdb_views() under
        //    DuckLake) is logged and skipped, never propagated.
        for sql in CUSTOM_TABLE_SQLS {
            let Ok(mut stmt) = conn.prepare(sql) else {
                continue;
            };
            let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
                continue;
            };
            for name in rows.flatten() {
                if known.contains(&name) {
                    continue;
                }
                known.insert(name.clone());
                // Custom objects aren't in the source cache, so hydrate here.
                let cols = schema::describe_view(conn, &name).unwrap_or_default();
                let count = count_rows(conn, &name);
                result.push(SourceTable {
                    name: name.clone(),
                    label: name,
                    kind: SourceKind::View,
                    storage: StorageKind::Custom,
                    path: String::new(),
                    scan_path: String::new(),
                    partition_keys: Vec::new(),
                    row_count_estimate: count,
                    columns: cols,
                });
            }
        }
        Ok(result)
    })
    .await
}

// ===========================================================================
// Lake import + sync
// ===========================================================================

/// Bring a folder/file into the current workspace, then scan + register as
/// DuckLake tables/views. Size-based strategy (threshold from config):
///   * small (≤ threshold) → copy into workspace dir + materialized TABLE
///   * large (> threshold) → register in place + zero-copy VIEW
#[tauri::command]
pub async fn import_file_to_workspace(
    workspace: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    let src_path = PathBuf::from(&path);
    if !src_path.exists() {
        return Err(format!("源路径不存在: {path}"));
    }

    let ws_dir = resolve_workspace_dir(&workspace)?;
    std::fs::create_dir_all(&ws_dir).map_err(|e| format!("无法创建工作区目录: {e}"))?;

    // Always copy the source into the workspace so it shows up in the Files tree
    // and the project stays self-contained. (Zero-copy for very large files is a
    // future optimization — see `decide_storage`.)
    let target_path = copy_into_workspace_if_needed(&src_path, &ws_dir)?;

    // Scan the imported target (blocking), then run the shared sync (naming +
    // register/rename). The lake for this workspace is assumed already attached
    // (it is attached on workspace load via register_workspace_sources).
    let target_for_scan = target_path.clone();
    let entries = match tauri::async_runtime::spawn_blocking(move || scan::scan_path(&target_for_scan, false)).await {
        Ok(v) => v,
        Err(e) => return Err(format!("scan task join error: {e}")),
    };
    sync_entries(state.conn.clone(), state.sources.clone(), workspace, ws_dir, entries, false)
        .await
        .map_err(|e| e.to_string())
}

/// Switch to a workspace (re-attach its DuckLake) and incrementally sync sources
/// against the filesystem: register new files, drop orphans, rebuild any lake
/// object that went missing. Returns the workspace's full source list.
#[tauri::command]
pub async fn register_workspace_sources(
    workspace_path: String,
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    register_workspace_sources_inner(workspace_path, &state).await
}

pub async fn register_workspace_sources_inner(
    workspace_path: String,
    state: &AppState,
) -> Result<Vec<SourceTable>, String> {
    let ws_dir = resolve_workspace_dir(&workspace_path)?;

    // Rebuild the session connection and attach this workspace's DuckLake.
    switch_workspace_lake(state, workspace_path.clone(), ws_dir.clone()).await?;

    // Scan the workspace dir for the files currently present (blocking).
    let ws_dir_for_scan = ws_dir.clone();
    let entries = match tauri::async_runtime::spawn_blocking(move || scan::scan_path(&ws_dir_for_scan, true)).await {
        Ok(v) => v,
        Err(e) => return Err(format!("scan task join error: {e}")),
    };

    sync_entries(state.conn.clone(), state.sources.clone(), workspace_path, ws_dir, entries, true)
        .await
        .map_err(|e| e.to_string())
}

/// Registration coverage for one workspace, surfaced as the colored dot next to
/// the project name. `total` is what `scan_path` (same walk as the full sync,
/// including the parquet group/dedupe rules) says *should* exist; `registered`
/// is how many rows the SQLite `sources` mapping currently has. The frontend
/// can't compute `total` itself (lazy file tree + parquet grouping), so it must
/// come from here. Read-only: SQLite + filesystem walk, no DuckDB connection.
#[derive(serde::Serialize)]
pub struct RegisterStatus {
    pub total: usize,
    pub registered: usize,
    /// "all" | "partial" | "none"
    pub status: String,
}

#[tauri::command]
pub async fn workspace_register_status(
    workspace_path: String,
) -> Result<RegisterStatus, String> {
    let sqlite = db::get_db_conn().map_err(|e| e.to_string())?;
    let registered = db::list_sources(&sqlite, &workspace_path)
        .map_err(|e| e.to_string())?
        .len();

    let ws_dir = resolve_workspace_dir(&workspace_path)?;
    // spawn_blocking: scan_path walks the filesystem synchronously.
    let total = tokio::task::spawn_blocking(move || scan::scan_path(&ws_dir, true).len())
        .await
        .map_err(|e| format!("scan task join error: {e}"))?;

    let status = if total == 0 || registered == total {
        "all"
    } else if registered == 0 {
        "none"
    } else {
        "partial"
    };

    Ok(RegisterStatus { total, registered, status: status.to_string() })
}

/// Synchronize a set of scan entries against the persisted mappings: pick a
/// good ASCII table name for each (LLM → pinyin fallback), rename the lake
/// object when the name changed (matched by `scan_path`, so a name change does
/// NOT re-materialize), register brand-new sources, and refresh the in-memory
/// cache. Shared by workspace sync and single-file import.
///
/// `prune_orphans`: only **full workspace sync** (`register_workspace_sources`)
/// drops lake objects whose backing file is no longer on disk. A single-file
/// import passes `entries` = [that one file], so orphan pruning there would wipe
/// every *other* table in the workspace — that is exactly the "clicking one file
/// makes only its table show up" bug, so it must be `false` for imports.
async fn sync_entries(
    conn: Arc<Mutex<duckdb::Connection>>,
    sources_cache: Arc<Mutex<Vec<SourceTable>>>,
    ws_path: String,
    ws_dir: PathBuf,
    entries: Vec<scan::ScanEntry>,
    prune_orphans: bool,
) -> AppResult<Vec<SourceTable>> {
    use std::collections::{HashMap, HashSet};

    // 1. Load existing mappings, indexed by scan_path (stable file identity).
    let sqlite = db::get_db_conn()?;
    let existing = db::list_sources(&sqlite, &ws_path)?;
    let existing_by_scan: HashMap<String, SourceRecord> =
        existing.iter().map(|r| (r.scan_path.clone(), r.clone())).collect();
    let entry_scan_paths: HashSet<String> = entries.iter().map(|e| e.scan_path.clone()).collect();
    drop(sqlite);

    // 2. Decide each entry's target view name + name_source.
    //    - matched record already settled ("llm") → reuse its name, no LLM call.
    //    - otherwise (new / "legacy" / "fallback") → choose_name (LLM → pinyin),
    //      concurrent with per-call timeout.
    let mut decisions: Vec<(String, &'static str)> = vec![(String::new(), "fallback"); entries.len()];
    let need_choose: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let Some(rec) = existing_by_scan.get(&e.scan_path) {
                if rec.name_source == "llm" {
                    decisions[i] = (rec.table_name.clone(), "llm");
                    return None;
                }
            }
            Some((i, e.label.clone()))
        })
        .collect();
    let chosen = futures_util::future::join_all(
        need_choose.iter().map(|(i, label)| async move {
            let (name, src) = naming::choose_name(label).await;
            (*i, name, src)
        }),
    )
    .await;
    for (i, name, src) in chosen {
        decisions[i] = (name, src);
    }
    // Safety net: any entry that still has no name (shouldn't happen) → pinyin.
    for (i, e) in entries.iter().enumerate() {
        if decisions[i].0.is_empty() {
            decisions[i] = (naming::view_name(&e.label), "fallback");
        }
    }

    // 3. Blocking: dedup names, rename/create, hydrate, drop orphans.
    let join = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let now = now_ms();
        let sqlite = db::get_db_conn()?;

        // Dedup target names within this batch (append _2, _3, …).
        let mut used: HashSet<String> = HashSet::new();
        let final_names: Vec<String> = decisions
            .iter()
            .map(|(base, _)| {
                let mut name = base.clone();
                let mut suffix = 2;
                while used.contains(&name) {
                    name = format!("{}_{}", base, suffix);
                    suffix += 1;
                }
                used.insert(name.clone());
                name
            })
            .collect();

        let guard = conn.blocking_lock();
        load_extensions_if_needed(&guard, &entries);

        let mut result: Vec<SourceTable> = Vec::new();

        // Present sources: rename to the target name if it changed, else hydrate
        // (rebuild the lake object if it vanished).
        for (i, e) in entries.iter().enumerate() {
            let target = &final_names[i];
            let src = decisions[i].1;
            let matched = existing_by_scan.get(&e.scan_path).cloned();

            if let Some(rec) = matched {
                let storage = StorageKind::from_db_str(&rec.storage);
                let needs_rename = rec.table_name != *target;
                // A source is reused as-is only when its lake object still exists
                // AND its file fingerprint (mtime+size) is unchanged. A changed
                // file falls through to the rebuild branch so downstream objects
                // see fresh data (previously the change was silently ignored).
                let fingerprint_unchanged =
                    rec.file_mtime == e.mtime && rec.file_size == e.file_size as i64;
                if table_exists_in_lake(&guard, &rec.table_name) && fingerprint_unchanged {
                    if needs_rename {
                        // Rename preserves the data; no re-materialization.
                        if let Err(err) = rename_lake_object(&guard, &rec.table_name, target, storage) {
                            eprintln!("rename {} -> {} failed: {err}", rec.table_name, target);
                            if let Ok(t) = build_source_table_from_record(&guard, &rec) {
                                result.push(t);
                            }
                            continue;
                        }
                    }
                    // Update the record (name + label + name_source) and hydrate.
                    let mut updated = rec.clone();
                    updated.table_name = target.clone();
                    updated.label = e.label.clone();
                    updated.name_source = src.to_string();
                    let _ = db::upsert_source(&sqlite, &ws_path, &updated);
                    // upsert is keyed by (ws, table_name); if renamed, the old row
                    // under the old name is now stale — drop it.
                    if needs_rename {
                        let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                    }
                    if let Ok(t) = build_source_table_from_record(&guard, &updated) {
                        result.push(t);
                    }
                } else {
                    // Lake object vanished OR file changed — rebuild under the target name.
                    let mut work = if storage == StorageKind::Table {
                        materialize_into_workspace(e, &ws_dir)?
                    } else {
                        e.clone()
                    };
                    work.view_name = target.clone();
                    drop_lake_object(&guard, &rec.table_name);
                    match register::register(&guard, &work, storage) {
                        Ok(t) => {
                            let new_rec = source_record_from(&t, e, rec.created_at, src);
                            let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                            let _ = db::upsert_source(&sqlite, &ws_path, &new_rec);
                            result.push(t);
                        }
                        Err(err) => eprintln!("rebuild {} failed: {err}", e.label),
                    }
                }
            } else {
                // New source: register under the target name.
                let storage = decide_storage(e);
                let mut work = if storage == StorageKind::Table {
                    materialize_into_workspace(e, &ws_dir)?
                } else {
                    e.clone()
                };
                work.view_name = target.clone();
                match register::register(&guard, &work, storage) {
                    Ok(t) => {
                        let rec = source_record_from(&t, e, now, src);
                        let _ = db::upsert_source(&sqlite, &ws_path, &rec);
                        result.push(t);
                    }
                    Err(err) => eprintln!("register {} failed: {err}", e.label),
                }
            }
        }

        // Orphans: mapped but no longer on disk. Only on a full workspace sync —
        // never on a single-file import, where `entries` is just the imported file
        // and pruning would delete every other table in the workspace.
        if prune_orphans {
            for rec in &existing {
                if !entry_scan_paths.contains(&rec.scan_path) {
                    drop_lake_object(&guard, &rec.table_name);
                    let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                }
            }
        }

        drop(guard);

        // 4. Refresh the in-memory cache.
        let mut cache = sources_cache.blocking_lock();
        cache.clear();
        cache.extend(result.iter().cloned());
        Ok(result)
    })
    .await;

    match join {
        Ok(v) => v,
        Err(e) => Err(AppError::new(format!("task join error: {e}"))),
    }
}

// ===========================================================================
// Config commands
// ===========================================================================

/// Read a config value (None if unset).
#[tauri::command]
pub async fn get_app_config(key: String) -> Result<Option<String>, String> {
    let conn = db::get_db_conn()?;
    db::get_config(&conn, &key)
}

/// Write a config value.
#[tauri::command]
pub async fn set_app_config(key: String, value: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::set_config(&conn, &key, &value)
}

/// Read configurations from ~/.lakemind/settings.json
#[tauri::command]
pub async fn load_settings_json() -> Result<String, String> {
    let mut path = db::get_lakemind_dir()?;
    path.push("settings.json");
    if !path.exists() {
        return Ok("{}".to_string());
    }
    std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {e}"))
}

/// Write configurations to ~/.lakemind/settings.json
#[tauri::command]
pub async fn save_settings_json(json: String) -> Result<(), String> {
    let mut path = db::get_lakemind_dir()?;
    path.push("settings.json");
    std::fs::write(path, json).map_err(|e| format!("保存配置文件失败: {e}"))
}

// ===========================================================================
// Filesystem commands
// ===========================================================================

/// Open a native platform directory picker and return the selected path.
#[tauri::command]
pub async fn select_directory() -> Result<Option<String>, String> {
    Ok(select_directory_native())
}

#[derive(serde::Serialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Read the direct children of a workspace folder.
#[tauri::command]
pub async fn read_directory(path: String) -> Result<Vec<FileItem>, String> {
    let resolved_path = resolve_workspace_dir(&path)?;
    if !resolved_path.exists() {
        return Err(format!("目录不存在: {}", resolved_path.display()));
    }

    let mut items = Vec::new();
    let entries = std::fs::read_dir(&resolved_path).map_err(|e| format!("读取目录失败: {e}"))?;
    for entry in entries {
        if let Ok(entry) = entry {
            let name = entry.file_name().to_string_lossy().to_string();
            // Hide dotfiles (the `.lake/` store) and any stray DuckDB/DuckLake
            // artifacts so the Files tree shows only the user's data files.
            if name.starts_with('.')
                || name == "lake.duckdb"
                || name == "lake.ducklake"
                || name == "lake_data"
                || name.ends_with(".ducklake.wal")
            {
                continue;
            }
            let p = entry.path();
            let is_dir = p.is_dir();
            items.push(FileItem {
                name,
                path: p.to_string_lossy().to_string(),
                is_dir,
            });
        }
    }
    items.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });
    Ok(items)
}

// ===========================================================================
// Workspace registry
// ===========================================================================

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    pub name: String,
    pub path: String,
}

#[tauri::command]
pub async fn load_workspaces() -> Result<Vec<Workspace>, String> {
    let conn = db::get_db_conn()?;
    let mut stmt = conn
        .prepare("SELECT name, path FROM workspaces ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok(Workspace { name: row.get(0)?, path: row.get(1)? }))
        .map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    for r in rows {
        if let Ok(w) = r {
            list.push(w);
        }
    }
    Ok(list)
}

#[tauri::command]
pub async fn add_workspace(name: String, path: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO workspaces (path, name, created_at) VALUES (?, ?, ?)",
        rusqlite::params![path, name, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn remove_workspace(path: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);

    // Clean up content files for all tasks under this workspace.
    let mut stmt = conn
        .prepare("SELECT id, kind FROM tasks WHERE workspace_path = ?")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&path], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;

    let lakemind_dir = db::get_lakemind_dir()?;
    for r in rows {
        if let Ok((id, kind)) = r {
            delete_task_content_files(&lakemind_dir, &id, &kind);
        }
    }

    // Deleting the workspace cascades to its tasks and sources (FK ON DELETE CASCADE).
    conn.execute("DELETE FROM workspaces WHERE path = ?", [&path])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ===========================================================================
// Task persistence
// ===========================================================================

#[derive(serde::Serialize, serde::Deserialize)]
pub struct QueryTask {
    pub id: String,
    pub name: String,
    pub sql: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    pub kind: String,
    pub messages: Option<serde_json::Value>,
    pub saved: bool,
    #[serde(rename = "modelId")]
    pub model_id: Option<String>,
}

#[tauri::command]
pub async fn load_workspace_tasks(workspace_path: String) -> Result<Vec<QueryTask>, String> {
    let conn = db::get_db_conn()?;
    let mut stmt = conn
        .prepare("SELECT id, name, kind, created_at, saved, model_id FROM tasks WHERE workspace_path = ? ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&workspace_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i32>(4)? != 0,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let lakemind_dir = db::get_lakemind_dir()?;
    let mut tasks = Vec::new();
    for r in rows {
        if let Ok((id, name, kind, created_at, saved, model_id)) = r {
            let mut sql = String::new();
            let mut messages = None;
            if kind == "sql" {
                let filepath = lakemind_dir.join("sqls").join(format!("{id}.sql"));
                if filepath.exists() {
                    sql = std::fs::read_to_string(filepath).unwrap_or_default();
                }
            } else if kind == "chat" {
                let filepath = lakemind_dir.join("chats").join(format!("{id}.json"));
                if filepath.exists() {
                    let json_str = std::fs::read_to_string(filepath).unwrap_or_default();
                    messages = serde_json::from_str(&json_str).ok();
                }
            }
            tasks.push(QueryTask { id, name, sql, created_at, kind, messages, saved, model_id });
        }
    }
    Ok(tasks)
}

#[tauri::command]
pub async fn save_sql_task(workspace_path: String, task_id: String, name: String, sql: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, workspace_path, name, kind, created_at, saved)
         VALUES (?, ?, ?, 'sql', COALESCE((SELECT created_at FROM tasks WHERE id = ?), ?), 1)",
        rusqlite::params![task_id, workspace_path, name, task_id, now],
    )
    .map_err(|e| e.to_string())?;

    let lakemind_dir = db::get_lakemind_dir()?;
    let filepath = lakemind_dir.join("sqls").join(format!("{task_id}.sql"));
    std::fs::write(filepath, sql).map_err(|e| format!("Failed to write SQL file: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn save_chat_task(
    workspace_path: String,
    task_id: String,
    name: String,
    messages: serde_json::Value,
    model_id: Option<String>,
) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, workspace_path, name, kind, created_at, saved, model_id)
         VALUES (?, ?, ?, 'chat', COALESCE((SELECT created_at FROM tasks WHERE id = ?), ?), 1, ?)",
        rusqlite::params![task_id, workspace_path, name, task_id, now, model_id],
    )
    .map_err(|e| e.to_string())?;

    let lakemind_dir = db::get_lakemind_dir()?;
    let filepath = lakemind_dir.join("chats").join(format!("{task_id}.json"));
    let json_str = serde_json::to_string(&messages).map_err(|e| e.to_string())?;
    std::fs::write(filepath, json_str).map_err(|e| format!("Failed to write chat JSON file: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn delete_task(task_id: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let kind: Option<String> = conn
        .query_row("SELECT kind FROM tasks WHERE id = ?", [&task_id], |row| row.get(0))
        .ok();
    if let Some(k) = &kind {
        let lakemind_dir = db::get_lakemind_dir()?;
        delete_task_content_files(&lakemind_dir, &task_id, k);
    }
    conn.execute("DELETE FROM tasks WHERE id = ?", [&task_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn start_agent_chat(
    window: tauri::Window,
    task_id: String,
    model_id: String,
    prompt: String,
    history_json: String,
    priority: Option<String>,
    confirm_mode: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    let priority = priority.unwrap_or_else(|| "均衡".to_string());
    let confirm_mode = confirm_mode.unwrap_or_else(|| "变更前确认".to_string());
    tokio::spawn(async move {
        if let Err(e) = crate::agent::run_agent_chat_stream(
            window.clone(),
            task_id.clone(),
            model_id,
            prompt,
            history_json,
            priority,
            confirm_mode,
            app_state,
        )
        .await
        {
            eprintln!("Agent execution error: {e}");
            let _ = window.emit(
                "agent-event",
                crate::agent::AgentStreamEvent {
                    task_id,
                    kind: "error".to_string(),
                    text: Some(e),
                    segment: None,
                },
            );
        }
    });
    Ok(())
}

/// Resolve a DDL tool call parked in "变更前确认" mode. Called from the UI when
/// the user clicks 确认执行 (`approved = true`) or 取消 (`approved = false`).
/// The matching tool `call()` resumes via the oneshot channel.
#[tauri::command]
pub async fn resolve_tool_confirmation(
    task_id: String,
    tool_call_id: String,
    approved: bool,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    let key = format!("{}:{}", task_id, tool_call_id);
    let pending = {
        let mut map = state.pending_confirmations.lock().await;
        map.remove(&key)
    };
    match pending {
        Some(p) => {
            let _ = p.tx.send(crate::state::ConfirmDecision { approved });
            Ok(approved)
        }
        None => Err("未找到待确认的操作（可能已超时或已处理）".to_string()),
    }
}

// ===========================================================================
// Internals — connection switching, import strategy, helpers
// ===========================================================================

/// Lock the connection on a blocking thread, then run `f`.
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

/// Rebuild the session connection and attach `ws_dir`'s DuckLake as the default
/// catalog. Also records the active workspace key on the state.
async fn switch_workspace_lake(state: &AppState, workspace_path: String, ws_dir: PathBuf) -> Result<(), String> {
    let dir_for_conn = ws_dir.clone();
    let new_conn = tauri::async_runtime::spawn_blocking(move || AppState::open_workspace(&dir_for_conn))
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| e.to_string())?;

    {
        let mut c = state.conn.lock().await;
        *c = new_conn;
    }
    *state.workspace_dir.lock().await = ws_dir;
    *state.workspace_path.lock().await = workspace_path;
    state.sources.lock().await.clear();
    Ok(())
}

/// Decide how to store a scan entry.
///
/// **Zero-copy VIEWs are not yet wired up** — every import is materialized into
/// a DuckLake table so the source file lands in the workspace dir (visible in
/// the Files tree) and the project stays self-contained. When zero-copy for very
/// large files lands later, this will branch on `e.file_size` vs the configured
/// threshold (`db::get_zero_copy_threshold`) and return `View` for large sources
/// (and for Delta, which is already an external table format).
fn decide_storage(_e: &scan::ScanEntry) -> StorageKind {
    StorageKind::Table
}

/// Copy a scan entry's source file/dir into the workspace (if not already
/// inside it) and remap its path/scan_path to the copy. For TABLE materialization.
fn materialize_into_workspace(e: &scan::ScanEntry, ws_dir: &Path) -> AppResult<scan::ScanEntry> {
    let src = PathBuf::from(&e.path);
    let canonical_src = src.canonicalize().unwrap_or_else(|_| src.clone());
    let canonical_ws = ws_dir.canonicalize().unwrap_or_else(|_| ws_dir.to_path_buf());
    if canonical_src.starts_with(&canonical_ws) {
        return Ok(e.clone()); // already inside the workspace
    }
    let name = src
        .file_name()
        .ok_or_else(|| AppError::new("无效文件名"))?;
    let dst = ws_dir.join(name);
    copy_recursive(&src, &dst)?;

    let mut work = e.clone();
    let old_dir = e.path.clone();
    let new_dir = crate::duckdb::pathutil::forward_slashes(&dst);
    work.path = new_dir.clone();
    // scan_path is `<old_dir>/<glob_tail>`; swap the directory prefix.
    work.scan_path = if !old_dir.is_empty() {
        e.scan_path.replacen(&old_dir, &new_dir, 1)
    } else {
        e.scan_path.clone()
    };
    Ok(work)
}

/// Copy `src` into the workspace dir if it isn't already inside it. Returns the
/// path to register from.
fn copy_into_workspace_if_needed(src: &Path, ws_dir: &Path) -> Result<PathBuf, String> {
    let canonical_src = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    let canonical_ws = ws_dir.canonicalize().unwrap_or_else(|_| ws_dir.to_path_buf());
    if canonical_src.starts_with(&canonical_ws) {
        return Ok(src.to_path_buf());
    }
    let name = src
        .file_name()
        .ok_or_else(|| "无效文件名".to_string())?;
    let dst = ws_dir.join(name);
    copy_recursive(src, &dst).map_err(|e| format!("拷贝到工作区失败: {e}"))?;
    Ok(dst)
}

fn drop_lake_object(conn: &duckdb::Connection, name: &str) {
    let _ = conn.execute(&format!("DROP VIEW IF EXISTS \"{}\";", name), []);
    let _ = conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", name), []);
}

/// Rename a lake table/view. `ALTER TABLE` for materialized tables, `ALTER VIEW`
/// for zero-copy views. Used by the sync path when a source's generated name
/// changes (so the file↔table identity — tracked by scan_path — is preserved
/// without re-materializing).
fn rename_lake_object(
    conn: &duckdb::Connection,
    old: &str,
    new: &str,
    storage: StorageKind,
) -> AppResult<()> {
    let sql = match storage {
        StorageKind::View => format!("ALTER VIEW \"{}\" RENAME TO \"{}\";", old, new),
        _ => format!("ALTER TABLE \"{}\" RENAME TO \"{}\";", old, new),
    };
    conn.execute(&sql, [])
        .map_err(|e| AppError::new(format!("重命名 {old} → {new} 失败: {e}")))?;
    Ok(())
}

/// True if a table or view named `name` exists in the attached lake. Tables and
/// views are queried separately so a flaky `duckdb_views()` (seen under DuckLake)
/// cannot make an existing table look absent (which would trigger needless
/// rebuilds on every sync).
pub(crate) fn table_exists_in_lake(conn: &duckdb::Connection, name: &str) -> bool {
    let n = name.replace('"', "\"\"");
    let table_sql = format!(
        "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name=\"{n}\""
    );
    if conn.query_row(&table_sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0 {
        return true;
    }
    let view_sql = format!(
        "SELECT count(*) FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=\"{n}\""
    );
    conn.query_row(&view_sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

fn count_rows(conn: &duckdb::Connection, name: &str) -> Option<i64> {
    let n = name.replace('"', "\"\"");
    conn.query_row(&format!("SELECT count(*) FROM \"{n}\""), [], |r| r.get::<_, i64>(0))
        .ok()
}

/// Lazily INSTALL+LOAD the delta/excel extensions only if such a source is present.
fn load_extensions_if_needed(conn: &duckdb::Connection, entries: &[scan::ScanEntry]) {
    let needs_delta = entries.iter().any(|e| e.kind == SourceKind::Delta);
    if needs_delta {
        let _ = conn.execute("INSTALL delta;", []);
        let _ = conn.execute("LOAD delta;", []);
    }
    let needs_excel = entries.iter().any(|e| e.kind == SourceKind::Excel);
    if needs_excel {
        let _ = conn.execute("INSTALL excel;", []);
        let _ = conn.execute("LOAD excel;", []);
    }
}

/// Hydrate a [`SourceTable`] from a mapping record + live DuckLake metadata.
///
/// Intentionally does NOT count rows — `SELECT count(*)` on materialized
/// DuckLake tables scans parquet, which is slow for large lakes. Row counts are
/// filled in lazily by `list_duckdb_tables` (called async after the fast
/// initial render) so the table list appears instantly with `row_count = None`.
fn build_source_table_from_record(conn: &duckdb::Connection, rec: &SourceRecord) -> AppResult<SourceTable> {
    let columns = schema::describe_view(conn, &rec.table_name).unwrap_or_default();
    Ok(SourceTable {
        name: rec.table_name.clone(),
        label: rec.label.clone(),
        kind: str_to_kind(&rec.kind),
        storage: StorageKind::from_db_str(&rec.storage),
        path: rec.file_path.clone(),
        scan_path: rec.scan_path.clone(),
        partition_keys: rec.partition_keys.clone(),
        row_count_estimate: None,
        columns,
    })
}

/// Build a mapping record from a freshly-registered source. The file
/// fingerprint (mtime/size) is taken from the originating `ScanEntry` so the
/// next sync can detect content changes and trigger a rebuild.
fn source_record_from(t: &SourceTable, e: &scan::ScanEntry, created_at: i64, name_source: &str) -> SourceRecord {
    SourceRecord {
        table_name: t.name.clone(),
        label: t.label.clone(),
        kind: kind_to_str(&t.kind).to_string(),
        storage: t.storage.to_db_str().to_string(),
        file_path: t.path.clone(),
        scan_path: t.scan_path.clone(),
        partition_keys: t.partition_keys.clone(),
        created_at,
        name_source: name_source.to_string(),
        file_mtime: e.mtime,
        file_size: e.file_size as i64,
    }
}

fn kind_to_str(k: &SourceKind) -> &'static str {
    match k {
        SourceKind::Parquet => "parquet",
        SourceKind::Csv => "csv",
        SourceKind::Json => "json",
        SourceKind::Delta => "delta",
        SourceKind::Excel => "excel",
        SourceKind::Table => "table",
        SourceKind::View => "view",
    }
}

fn str_to_kind(s: &str) -> SourceKind {
    match s {
        "parquet" => SourceKind::Parquet,
        "csv" => SourceKind::Csv,
        "json" => SourceKind::Json,
        "delta" => SourceKind::Delta,
        "excel" => SourceKind::Excel,
        "view" => SourceKind::View,
        _ => SourceKind::Table,
    }
}

fn delete_task_content_files(lakemind_dir: &Path, task_id: &str, kind: &str) {
    if kind == "sql" {
        let _ = std::fs::remove_file(lakemind_dir.join("sqls").join(format!("{task_id}.sql")));
    } else if kind == "chat" {
        let _ = std::fs::remove_file(lakemind_dir.join("chats").join(format!("{task_id}.json")));
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Reject anything that would let a caller break out of a quoted identifier.
fn sanitize_ident(name: &str) -> Result<String, AppError> {
    if name.is_empty() || name.contains('"') || name.contains('\0') {
        return Err(AppError::new("invalid table name"));
    }
    Ok(name.to_string())
}

// --- path helpers ---------------------------------------------------------

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

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
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
#[allow(dead_code)]
fn path_total_size(p: &Path) -> u64 {
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
