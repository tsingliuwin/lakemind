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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{State, Emitter};

/// Progress event for file import, emitted at each stage so the UI can show
/// what's happening instead of a silent busy spinner.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    /// Source file name being imported.
    file: String,
    /// Current stage: "copying" | "scanning" | "registering" | "done" | "error".
    stage: String,
    /// Target table name (when known).
    table: Option<String>,
    /// Column count of the resulting table (on "done").
    columns: Option<i64>,
    /// Row count of the resulting table (on "done").
    rows: Option<i64>,
    /// Error message (on "error").
    error: Option<String>,
}

/// Dependency info for a table/view: what it depends on (upstream) and what
/// depends on it (downstream). Used by the right-inspector panel and the
/// delete-protection check.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepInfo {
    /// Objects this table/view references in its SELECT (empty for s_ sources).
    upstreams: Vec<String>,
    /// Objects that reference this table/view in their SELECT.
    downstreams: Vec<String>,
}
use tokio::sync::Mutex;

use crate::db::{self, SourceRecord};
use crate::duckdb::{execute, naming, register, scan, schema};
use crate::fingerprint;
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
    let hard_secs = crate::agent::get_query_hard_timeout();
    if hard_secs == 0 {
        // No hard limit — just run normally.
        return run_blocking(state, move |conn| execute::run_query(conn, sql.trim(), row_cap)).await;
    }

    // Read the interrupt handle from AppState — no conn lock needed.
    let ih = state.interrupt_handle.lock().unwrap().clone();

    // The entire run_blocking (including its internal conn lock acquisition)
    // is inside the timeout, so even lock contention counts against the budget.
    let fut = run_blocking(state, move |conn| execute::run_query(conn, sql.trim(), row_cap));
    match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), fut).await {
        Ok(inner) => inner,
        Err(_elapsed) => {
            // Hard timeout reached — fire interrupt and return immediately.
            ih.interrupt();
            Err(format!(
                "查询已达到最大等待时间（{} 秒）被强制终止。可在\u{201c}设置\u{201d}->\u{201c}常规\u{201d}中调整该限制。",
                hard_secs
            ))
        }
    }
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
/// Millisecond-fast startup table list: a **pure SQLite read**, zero DuckDB.
///
/// Both `sources` (s_ tables) and `object_defs` (agent-created t_/v_ views)
/// cache columns + row counts keyed by fingerprint, so we can render the full
/// list — sources AND views, with columns + row counts — without touching
/// DuckDB at all. Views appear just as fast as source tables.
#[tauri::command]
pub async fn list_tables_fast(
    workspace_path: String,
    _state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    // SQLite + per-def stat() calls: run on the blocking pool so the tokio
    // worker thread stays free (this runs on startup / table-list refresh).
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<SourceTable>, String> {
        let start = std::time::Instant::now();
        let ws_dir = resolve_workspace_dir(&workspace_path)?;
        let okf_views_dir = ws_dir.join("okf").join("views");
        let okf_tables_dir = ws_dir.join("okf").join("tables");

        // No lake attach, no DuckDB connection — just the SQLite caches.
        let sqlite = db::get_db_conn()?;
        let records = db::list_sources(&sqlite, &workspace_path)?;
        let mut result: Vec<SourceTable> = records.iter().map(build_source_table_from_record).collect();

        // Agent-created views/tables from object_defs (cached columns + row count,
        // valid as long as input_hash matches — verified later by warmup/sync).
        let defs = db::list_object_defs(&sqlite, &workspace_path)?;
        for d in &defs {
            let view_okf = okf_views_dir.join(format!("{}.md", d.table_name));
            let table_okf = okf_tables_dir.join(format!("{}.md", d.table_name));
            if !view_okf.exists() && !table_okf.exists() {
                continue;
            }
            result.push(SourceTable {
                name: d.table_name.clone(),
                label: d.table_name.clone(),
                kind: SourceKind::View,
                storage: StorageKind::Custom,
                path: String::new(),
                scan_path: String::new(),
                partition_keys: Vec::new(),
                row_count_estimate: d.row_count,
                columns: d.columns.clone(),
                is_sampled: false,
                full_row_count: None,
                sheet: None,
            });
        }

        tracing::info!(
            category = "sync",
            "list_tables_fast: {} objects from SQLite in {}ms",
            result.len(),
            start.elapsed().as_millis()
        );
        Ok(result)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

/// All tables/views for the current workspace: registered sources (from the
/// SQLite `sources` mapping, enriched with live column metadata) plus any custom
/// tables the user created via SQL (`storage = custom`). Best-effort throughout
/// — a missing lake object or a flaky catalog query never drops the set of
/// registered sources we already know about.
#[tauri::command]
pub async fn list_duckdb_tables(state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    let ws_path = state.workspace_path.lock().await.clone();
    let active_ws_path = state.workspace_path.clone();
    let ws_path_clone = ws_path.clone();
    // Registered sources come straight from the SQLite cache (columns + row
    // count stored), so only custom tables/views need a live DuckLake catalog
    // query here. Used to refresh after DDL/import.
    run_blocking(state, move |conn| {
        {
            let active = active_ws_path.blocking_lock().clone();
            if active != ws_path_clone {
                return Err(AppError::new("Workspace changed, list_duckdb_tables aborted"));
            }
        }
        let sqlite = db::get_db_conn()?;
        let records = db::list_sources(&sqlite, &ws_path_clone)?;

        // 1. Registered sources (authoritative). Pure SQLite cache.
        let mut result = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for rec in &records {
            known.insert(rec.table_name.clone());
            result.push(build_source_table_from_record(rec));
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
                if name.starts_with("s_") {
                    continue;
                }
                if known.contains(&name) {
                    continue;
                }
                known.insert(name.clone());
                result.push(hydrate_custom_object(conn, &sqlite, &ws_path, &name));
            }
        }
        Ok(result)
    })
    .await
}

/// Background liveness check, run after the fast startup list renders.
///
/// For each registered source we run `SELECT * FROM "name" LIMIT 0`. This is
/// intentionally a **metadata-only** probe (cheap, ~280ms/table) whose purpose
/// is to **verify the lake object is usable**. The fast sync path trusts the
/// fingerprint and skips `table_exists_in_lake`, so a lake object that went
/// missing without a fingerprint change would otherwise only surface as a query
/// error on click. Here we detect that and rebuild from the persisted scan_path.
///
/// We do NOT warm data pages (e.g. LIMIT 50) — pre-reading rows for every table
/// is wasted work when the user typically clicks only a few. The first click on
/// a table pays the column-store cold-start (~1s); the second hits the page
/// cache (~300ms). That's an inherent DuckDB trade-off we accept rather than
/// speculatively reading data the user may never look at.
///
/// Returns the refreshed source list (custom tables/views included) so the
/// frontend can update the data tree if any rebuild happened.
#[tauri::command]
pub async fn warmup_sources(state: State<'_, AppState>) -> Result<Vec<SourceTable>, String> {
    let ws_path = state.workspace_path.lock().await.clone();
    let ws_dir = state.workspace_dir.lock().await.to_string_lossy().to_string();
    let warmup_start = std::time::Instant::now();

    // Read source records from SQLite (no DuckDB lock needed).
    let sqlite = db::get_db_conn()?;
    let records = db::list_sources(&sqlite, &ws_path)?;
    drop(sqlite);

    // Warm each source with a SHORT connection lock, one at a time, yielding the
    // lock between tables. This lets a user's click-triggered query slip in
    // immediately (a single LIMIT 0 holds the lock for ~300ms, vs. the whole
    // batch for 5s+ in a single run_blocking — which blocked all clicks until it
    // finished). If a table turns out missing, rebuild it right here.
    let mut rebuilt = false;
    for rec in &records {
        let conn = state.conn.clone();
        let rec_clone = rec.clone();
        let ws_path_clone = ws_path.clone();
        let ws_dir_clone = ws_dir.clone();
        let active_ws_path = state.workspace_path.clone();
        let did_rebuild = tauri::async_runtime::spawn_blocking(move || -> bool {
            {
                let active = active_ws_path.blocking_lock().clone();
                if active != ws_path_clone {
                    return false;
                }
            }
            let guard = conn.blocking_lock();
            {
                let active = active_ws_path.blocking_lock().clone();
                if active != ws_path_clone {
                    return false;
                }
            }
            // LIMIT 0: metadata-only probe (cheap). Validates the lake object is
            // usable without reading data pages the user may never look at.
            let probe = format!("SELECT * FROM \"{}\" LIMIT 0", rec_clone.table_name.replace('"', "\"\""));
            if guard.execute(&probe, []).is_ok() {
                return false;
            }
            // Lake object missing/corrupt despite a matching fingerprint → rebuild.
            tracing::info!(category = "sync", "warmup: {} not usable, rebuilding from scan_path", rec_clone.table_name);
            let kind = str_to_kind(&rec_clone.kind);
            let entry = scan::ScanEntry {
                label: rec_clone.label.clone(),
                view_name: rec_clone.table_name.clone(),
                kind,
                path: rec_clone.file_path.clone(),
                scan_path: rec_clone.scan_path.clone(),
                partition_keys: rec_clone.partition_keys.clone(),
                file_size: rec_clone.file_size as u64,
                mtime: rec_clone.file_mtime,
                sheet: rec_clone.sheet.clone(),
            };
            let storage = StorageKind::from_db_str(&rec_clone.storage);
            drop_lake_object(&guard, &rec_clone.table_name);
            match register::register(&guard, &entry, storage, None) {
                Ok(t) => {
                    let new_rec = source_record_from(&t, &entry, rec_clone.created_at, &rec_clone.name_source);
                    if let Ok(sqlite) = db::get_db_conn() {
                        let _ = db::upsert_source(&sqlite, &ws_path_clone, &new_rec);
                        let _ = crate::okf::write_source_okf(&ws_dir_clone, &rec_clone.table_name, &rec_clone.label, &rec_clone.file_path, rec_clone.file_size as i64, rec_clone.file_mtime, &t.columns, t.row_count_estimate);
                        let _ = crate::okf::write_pipeline_okf(&ws_dir_clone, &rec_clone.table_name, &rec_clone.label, &rec_clone.file_path, &new_rec.storage);
                    }
                    true
                }
                Err(e) => {
                    tracing::warn!(category = "sync", "warmup rebuild {} failed: {e}", rec_clone.table_name);
                    false
                }
            }
        })
        .await
        .unwrap_or(false);
        rebuilt = rebuilt || did_rebuild;
    }

    // Checkpoint if any rebuild happened.
    if rebuilt {
        let active = state.workspace_path.lock().await.clone();
        if active == ws_path {
            let conn = state.conn.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let guard = conn.blocking_lock();
                let _ = guard.execute("CHECKPOINT;", []);
            })
            .await;
        }
    }

    // Warm custom tables (short lock each) and return the refreshed list.
    let conn = state.conn.clone();
    let ws_path2 = ws_path.clone();
    let active_ws_path2 = state.workspace_path.clone();
    let result = run_blocking(state, move |conn| {
        {
            let active = active_ws_path2.blocking_lock().clone();
            if active != ws_path2 {
                return Err(AppError::new("Workspace changed, warmup aborted"));
            }
        }
        let sqlite = db::get_db_conn()?;
        let records = db::list_sources(&sqlite, &ws_path2)?;
        let mut result = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for rec in &records {
            known.insert(rec.table_name.clone());
            result.push(build_source_table_from_record(rec));
        }
        for sql in CUSTOM_TABLE_SQLS {
            let Ok(mut stmt) = conn.prepare(sql) else { continue };
            let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else { continue };
            for name in rows.flatten() {
                if name.starts_with("s_") { continue; }
                if known.contains(&name) || name.starts_with("_tmp_") { continue; }
                known.insert(name.clone());
                let _ = conn.execute(&format!("SELECT * FROM \"{}\" LIMIT 0", name.replace('"', "\"\"")), []);
                let t = hydrate_custom_object(conn, &sqlite, &ws_path2, &name);
                // Backfill object_defs for views/tables that exist in the catalog
                // but aren't tracked yet. Fetch the view's defining SQL from the
                // DuckLake catalog so dependency extraction (extract_upstreams)
                // works — without it, downstream maps would be empty.
                if db::get_object_def(&sqlite, &ws_path2, &name).ok().flatten().is_none() {
                    let view_sql = conn.query_row(
                        "SELECT sql FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?",
                        [&name],
                        |r| r.get::<_, String>(0),
                    ).unwrap_or_default();
                    let cols_json = serde_json::to_string(&t.columns).unwrap_or_else(|_| "[]".into());
                    let _ = sqlite.execute(
                        "INSERT OR IGNORE INTO object_defs (workspace_path, table_name, kind, select_sql, input_hash, created_at, columns, row_count)
                         VALUES (?, ?, 'view', ?, '', ?, ?, ?)",
                        rusqlite::params![ws_path2, name, view_sql, crate::commands::now_ms(), cols_json, t.row_count_estimate],
                    );
                }
                result.push(t);
            }
        }
        Ok(result)
    })
    .await;

    tracing::info!(category = "sync", "warmup done: {} sources in {}ms", records.len(), warmup_start.elapsed().as_millis());
    // Silence unused warning while keeping conn available for future use.
    let _ = conn;
    result
}

/// Get the upstream and downstream dependencies of a table/view.
///
/// - **upstreams**: for t_/v_ objects, the lake tables referenced in their
///   `select_sql` (via `extract_upstreams`). For s_ source tables, the backing
///   **file name** (from `sources.file_path`) — the file is the true upstream
///   of a source table, making the dependency chain complete: file → s_ → t_/v_.
/// - **downstreams**: objects whose `select_sql` references this table.
#[tauri::command]
pub async fn get_dependencies(
    table_name: String,
    state: State<'_, AppState>,
) -> Result<DepInfo, String> {
    let ws_path = state.workspace_path.lock().await.clone();
    // SQLite queries (fingerprint graph): run on the blocking pool.
    tauri::async_runtime::spawn_blocking(move || -> Result<DepInfo, String> {
        let sqlite = db::get_db_conn()?;

        // Try object_defs first (t_/v_ have select_sql → real upstreams).
        let mut upstreams = fingerprint::get_upstreams(&sqlite, &ws_path, &table_name);

        // s_ source tables have no select_sql — their upstream is the source file.
        if upstreams.is_empty() {
            if let Ok(Some(rec)) = db::get_source_by_table(&sqlite, &ws_path, &table_name) {
                if !rec.file_path.is_empty() {
                    let file_name = std::path::Path::new(&rec.file_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&rec.file_path);
                    upstreams.push(file_name.to_string());
                }
            }
        }

        let downstreams = fingerprint::get_downstreams(&sqlite, &ws_path, &table_name);
        Ok(DepInfo { upstreams, downstreams })
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

/// Delete a table/view safely. If the object has downstream dependencies, they
/// are cascaded (deleted first, leaves → target) so everything is removed in one
/// pass. Drops lake objects + cleans up SQLite mappings for all affected objects.
#[tauri::command]
pub async fn drop_table_safe(
    table_name: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let ws_path = state.workspace_path.lock().await.clone();

    // Compute the cascade deletion order (downstreams first, target last).
    let sqlite = db::get_db_conn()?;
    let cascade = fingerprint::cascade_delete_order(&sqlite, &ws_path, &table_name);

    // Drop all objects in order + clean up SQLite mappings.
    let conn = state.conn.clone();
    let ws_path2 = ws_path.clone();
    let count = cascade.len();
    let _ = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let guard = conn.blocking_lock();
        for name in &cascade {
            drop_lake_object(&guard, name);
            let sqlite = db::get_db_conn()?;
            let _ = db::delete_object_def(&sqlite, &ws_path2, name);
            let _ = db::delete_source_by_table(&sqlite, &ws_path2, name);
        }
        let _ = guard.execute("CHECKPOINT;", []);
        Ok(())
    })
    .await
    .map_err(|e| format!("删除任务失败: {e}"))?;
    Ok(if count > 1 {
        format!("已删除 {} 及其 {} 个下游依赖", table_name, count - 1)
    } else {
        format!("已删除 {}", table_name)
    })
}

/// Delete a workspace file and cascade-clean its s_ table + downstream views.
///
/// 1. Find the source record whose `file_path` matches (the s_ table mapping).
/// 2. Cascade-delete that s_ table and all its downstream t_/v_ (same logic as
///    `drop_table_safe` — leaves first, target last).
/// 3. Remove the file from disk.
/// 4. Returns a summary of what was deleted.
#[tauri::command]
pub async fn delete_file(
    file_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let ws_path = state.workspace_path.lock().await.clone();

    // 1. Find every s_ table mapped to this file. A multi-sheet `.xlsx` backs
    //    several tables (one per sheet) sharing the same file_path, so collect
    //    them all — otherwise deleting the file would orphan the other sheets.
    let sqlite = db::get_db_conn()?;
    let sources = db::list_sources(&sqlite, &ws_path)?;
    let table_names: Vec<String> = sources
        .iter()
        .filter(|s| s.file_path == file_path || s.scan_path == file_path)
        .map(|s| s.table_name.clone())
        .collect();

    // 2. Cascade-delete each s_ table (+ downstreams) if it exists.
    let mut deleted_count = 0usize;
    if !table_names.is_empty() {
        // Build the full cascade set across all matched tables (a downstream
        // t_/v_ derived from one sheet may also appear under another; dedup so
        // we don't double-count or drop twice).
        let mut cascade: Vec<String> = Vec::new();
        for tn in &table_names {
            for name in fingerprint::cascade_delete_order(&sqlite, &ws_path, tn) {
                if !cascade.contains(&name) {
                    cascade.push(name);
                }
            }
        }
        deleted_count = cascade.len();
        let conn = state.conn.clone();
        let ws_path2 = ws_path.clone();
        let cascade2 = cascade.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let guard = conn.blocking_lock();
            for name in &cascade2 {
                drop_lake_object(&guard, name);
                let sqlite = db::get_db_conn()?;
                let _ = db::delete_object_def(&sqlite, &ws_path2, name);
                let _ = db::delete_source_by_table(&sqlite, &ws_path2, name);
            }
            let _ = guard.execute("CHECKPOINT;", []);
            Ok(())
        })
        .await
        .map_err(|e| format!("删除任务失败: {e}"))?;
    }

    // 3. Remove the file from disk.
    let p = std::path::Path::new(&file_path);
    if p.exists() {
        std::fs::remove_file(p).map_err(|e| format!("删除文件失败: {e}"))?;
    }

    Ok(if deleted_count > 0 {
        format!("已删除文件及 {} 个数据对象", deleted_count)
    } else {
        "已删除文件".to_string()
    })
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
    window: tauri::Window,
    workspace: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    let src_path = PathBuf::from(&path);
    let file_name = src_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    if !src_path.exists() {
        let _ = window.emit("import-progress", ImportProgress {
            file: file_name, stage: "error".into(), table: None, columns: None, rows: None,
            error: Some(format!("源路径不存在: {path}")),
        });
        return Err(format!("源路径不存在: {path}"));
    }

    let _ = window.emit("import-progress", ImportProgress {
        file: file_name.clone(), stage: "copying".into(), table: None, columns: None, rows: None, error: None,
    });

    let ws_dir = resolve_workspace_dir(&workspace)?;
    std::fs::create_dir_all(&ws_dir).map_err(|e| format!("无法创建工作区目录: {e}"))?;

    // Always copy the source into the workspace so it shows up in the Files tree
    // and the project stays self-contained.
    let target_path = match copy_into_workspace_if_needed(&src_path, &ws_dir) {
        Ok(p) => p,
        Err(e) => {
            let _ = window.emit("import-progress", ImportProgress {
                file: file_name.clone(), stage: "error".into(), table: None, columns: None, rows: None,
                error: Some(format!("复制文件失败: {e}")),
            });
            return Err(e);
        }
    };

    let _ = window.emit("import-progress", ImportProgress {
        file: file_name.clone(), stage: "scanning".into(), table: None, columns: None, rows: None, error: None,
    });

    // Scan the imported target (blocking), then run the shared sync (naming +
    // register/rename). The lake for this workspace is assumed already attached.
    let target_for_scan = target_path.clone();
    let entries = match tauri::async_runtime::spawn_blocking(move || scan::scan_path(&target_for_scan, false)).await {
        Ok(v) => v,
        Err(e) => {
            let _ = window.emit("import-progress", ImportProgress {
                file: file_name.clone(), stage: "error".into(), table: None, columns: None, rows: None,
                error: Some(format!("扫描失败: {e}")),
            });
            return Err(format!("scan task join error: {e}"));
        }
    };

    // GUARD: Check if workspace changed during scan
    {
        let current = state.workspace_path.lock().await.clone();
        if current != workspace {
            let _ = window.emit("import-progress", ImportProgress {
                file: file_name.clone(), stage: "error".into(), table: None, columns: None, rows: None,
                error: Some("Workspace changed during scan".to_string()),
            });
            return Err("Workspace changed during scan".to_string());
        }
    }

    let _ = window.emit("import-progress", ImportProgress {
        file: file_name.clone(), stage: "registering".into(), table: None, columns: None, rows: None, error: None,
    });

    let window_for_sync = window.clone();
    let file_for_sync = file_name.clone();
    let result = sync_entries(state.conn.clone(), state.sources.clone(), state.workspace_path.clone(), workspace, ws_dir, entries, false, Some(move |stage_msg: &str| {
        let _ = window_for_sync.emit("import-progress", ImportProgress {
            file: file_for_sync.clone(), stage: "registering".into(), table: Some(stage_msg.to_string()),
            columns: None, rows: None, error: None,
        });
    }))
    .await
    .map_err(|e| e.to_string());

    match &result {
        Ok(tables) => {
            if !tables.is_empty() {
                // Report every registered table. A multi-sheet xlsx yields one
                // table per sheet; emitting each lets the progress UI reflect
                // them all instead of only the first.
                for t in tables {
                    let _ = window.emit("import-progress", ImportProgress {
                        file: file_name.clone(), stage: "done".into(),
                        table: Some(t.name.clone()),
                        columns: Some(t.columns.len() as i64),
                        rows: t.row_count_estimate,
                        error: None,
                    });
                }
            } else {
                // Nothing recognizable was found under the imported path. This
                // is the common "picked a folder/file with no supported data
                // files" case — surface it as an error so the user understands
                // why no table appeared, instead of a silent misleading "done".
                let _ = window.emit("import-progress", ImportProgress {
                    file: file_name.clone(), stage: "error".into(), table: None, columns: None, rows: None,
                    error: Some("未找到可识别的数据文件（支持 csv/tsv/json/ndjson/parquet/parq/xlsx/xls）".to_string()),
                });
            }
        }
        Err(e) => {
            let _ = window.emit("import-progress", ImportProgress {
                file: file_name.clone(), stage: "error".into(), table: None, columns: None, rows: None,
                error: Some(e.clone()),
            });
        }
    }
    result
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

    // Attach this workspace's DuckLake only when we're not already on it. On a
    // fresh launch the default workspace is already attached (AppState::new /
    // list_tables_fast), so the background sync skips the connection rebuild +
    // source-clear that would otherwise make the table list flicker away and
    // back. An explicit workspace switch still re-attaches here.
    {
        let current = state.workspace_path.lock().await.clone();
        if current != workspace_path {
            switch_workspace_lake(state, workspace_path.clone(), ws_dir.clone()).await?;
        }
    }

    // Scan the workspace dir for the files currently present (blocking).
    let ws_dir_for_scan = ws_dir.clone();
    let entries = match tauri::async_runtime::spawn_blocking(move || scan::scan_path(&ws_dir_for_scan, true)).await {
        Ok(v) => v,
        Err(e) => return Err(format!("scan task join error: {e}")),
    };

    // GUARD: Check if workspace changed during scan
    {
        let current = state.workspace_path.lock().await.clone();
        if current != workspace_path {
            return Err("Workspace changed during file scan".to_string());
        }
    }

    sync_entries(state.conn.clone(), state.sources.clone(), state.workspace_path.clone(), workspace_path, ws_dir, entries, true, None::<fn(&str)>)
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
    active_ws_path: Arc<Mutex<String>>,
    ws_path: String,
    ws_dir: PathBuf,
    entries: Vec<scan::ScanEntry>,
    prune_orphans: bool,
    // Optional progress callback (file import path). Called with a human-
    // readable stage message (e.g. the table name being built). `None` for
    // startup sync / agent paths that don't need UI progress.
    progress: Option<impl Fn(&str) + Send + Sync + 'static>,
) -> AppResult<Vec<SourceTable>> {
    use std::collections::{HashMap, HashSet};
    let sync_start = std::time::Instant::now();

    // 1. Load existing mappings, indexed by (scan_path, sheet) — the stable file
    //    identity. A multi-sheet `.xlsx` backs several rows that share scan_path
    //    but differ by sheet, so sheet must be part of the identity key.
    let sqlite = db::get_db_conn()?;
    let existing = db::list_sources(&sqlite, &ws_path)?;
    let existing_by_scan: HashMap<(String, Option<String>), SourceRecord> = existing
        .iter()
        .map(|r| ((r.scan_path.clone(), r.sheet.clone()), r.clone()))
        .collect();
    let entry_identities: HashSet<(String, Option<String>)> = entries
        .iter()
        .map(|e| (e.scan_path.clone(), e.sheet.clone()))
        .collect();
    drop(sqlite);

    // 2. Decide each entry's target view name + name_source.
    //    - matched record already settled ("llm" OR "fallback") → reuse its name,
    //      no LLM call. The name is stable and already in use; re-evaluating every
    //      launch would re-run LLM/pinyin naming (slow) for no benefit.
    //    - only "legacy" (pre-naming-module) names get re-evaluated.
    let mut decisions: Vec<(String, &'static str)> = vec![(String::new(), "fallback"); entries.len()];
    let need_choose: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let Some(rec) = existing_by_scan.get(&(e.scan_path.clone(), e.sheet.clone())) {
                if rec.name_source == "llm" || rec.name_source == "fallback" {
                    let src: &'static str = if rec.name_source == "llm" { "llm" } else { "fallback" };
                    decisions[i] = (rec.table_name.clone(), src);
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
        
        // GUARD: check if the workspace has changed
        {
            let active = active_ws_path.blocking_lock().clone();
            if active != ws_path {
                return Err(AppError::new("Workspace changed, sync aborted"));
            }
        }

        load_extensions_if_needed(&guard, &entries);

        let mut result: Vec<SourceTable> = Vec::new();
        // Track whether any DuckLake mutation happened (rebuild/register/rename/
        // orphan-drop). If nothing changed, we skip the (2s+) checkpoint.
        let mut lake_mutated = false;
        let ws_dir_str = ws_dir.to_string_lossy().to_string();

        // Present sources: rename to the target name if it changed, else hydrate
        // (rebuild the lake object if it vanished).
        for (i, e) in entries.iter().enumerate() {
            let target = &final_names[i];
            let src = decisions[i].1;
            let matched = existing_by_scan.get(&(e.scan_path.clone(), e.sheet.clone())).cloned();

            if let Some(rec) = matched {
                let storage = StorageKind::from_db_str(&rec.storage);
                let needs_rename = rec.table_name != *target;
                // A source is reused as-is only when its lake object still exists
                // AND its file fingerprint (mtime+size) is unchanged. A changed
                // file falls through to the rebuild branch so downstream objects
                // see fresh data.
                let fingerprint_unchanged =
                    rec.file_mtime == e.mtime && rec.file_size == e.file_size as i64;
                if fingerprint_unchanged {
                    // Fingerprint matches → trust the persisted lake object exists
                    // (the catalog is durable and was attached at startup). Skipping
                    // the `table_exists_in_lake` check here is critical: that DuckLake
                    // metadata query takes ~1s per table, so verifying all-reused
                    // sources added seconds of pointless startup latency. Only when
                    // the fingerprint differs do we query DuckLake (to decide rebuild).
                    tracing::info!(category = "sync", "{}: fingerprint match → reuse (cached)", e.label);
                    if needs_rename {
                        // Rename preserves the data; no re-materialization.
                        if let Err(err) = rename_lake_object(&guard, &rec.table_name, target, storage) {
                            tracing::warn!(category = "sync", "rename {} -> {} failed: {err}", rec.table_name, target);
                            result.push(build_source_table_from_record(&rec));
                            continue;
                        }
                        lake_mutated = true;
                    }
                    // Update the record (name + label + name_source) and hydrate.
                    // Also refresh the file fingerprint from the scan entry so a
                    // previously-uninitialized (migrated) row gets its real
                    // mtime/size recorded without triggering a rebuild.
                    let mut updated = rec.clone();
                    updated.table_name = target.clone();
                    updated.label = e.label.clone();
                    updated.name_source = src.to_string();
                    updated.file_mtime = e.mtime;
                    updated.file_size = e.file_size as i64;
                    let _ = db::upsert_source(&sqlite, &ws_path, &updated);
                    // upsert is keyed by (ws, table_name); if renamed, the old row
                    // under the old name is now stale — drop it.
                    if needs_rename {
                        let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                        let _ = crate::okf::delete_okf_files(&ws_dir_str, &rec.table_name);
                    }
                    let _ = crate::okf::write_source_okf(&ws_dir_str, target, &e.label, &e.path, e.file_size as i64, e.mtime, &rec.columns, rec.row_count);
                    let _ = crate::okf::write_pipeline_okf(&ws_dir_str, target, &e.label, &e.path, &updated.storage);
                    result.push(build_source_table_from_record(&updated));
                } else {
                    // Fingerprint changed → rebuild under the target name.
                    tracing::info!(category = "sync", "{}: fingerprint changed → rebuild", e.label);
                    lake_mutated = true;
                    let mut work = if storage == StorageKind::Table {
                        materialize_into_workspace(e, &ws_dir)?
                    } else {
                        e.clone()
                    };
                    work.view_name = target.clone();
                    drop_lake_object(&guard, &rec.table_name);
                    let _ = crate::okf::delete_okf_files(&ws_dir_str, &rec.table_name);
                    let prog = progress.as_ref().map(|p| p as &dyn Fn(&str));
                    if let Some(p) = prog { p(target); }
                    match register::register(&guard, &work, storage, prog) {
                        Ok(t) => {
                            let new_rec = source_record_from(&t, e, rec.created_at, src);
                            let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                            let _ = db::upsert_source(&sqlite, &ws_path, &new_rec);
                            let _ = crate::okf::write_source_okf(&ws_dir_str, target, &e.label, &e.path, e.file_size as i64, e.mtime, &t.columns, t.row_count_estimate);
                            let _ = crate::okf::write_pipeline_okf(&ws_dir_str, target, &e.label, &e.path, &new_rec.storage);
                            result.push(t);
                        }
                        Err(err) => tracing::warn!(category = "sync", "rebuild {} failed: {err}", e.label),
                    }
                }
            } else {
                // New source. If the lake object already exists under the target
                // name (e.g. the SQLite mapping was cleared but the DuckLake table
                // survived), reuse it and just record the mapping + fingerprint —
                // no re-materialization needed. Only build from scratch when the
                // object is genuinely absent.
                let storage = decide_storage(e);
                if table_exists_in_lake(&guard, target) {
                    tracing::info!(category = "sync", "{}: new mapping, lake object exists → adopt", e.label);
                    let cols = schema::describe_view(&guard, target).unwrap_or_default();
                    let count = count_rows(&guard, target);
                    let t = SourceTable {
                        name: target.clone(),
                        label: e.label.clone(),
                        kind: e.kind.clone(),
                        storage,
                        path: e.path.clone(),
                        scan_path: e.scan_path.clone(),
                        partition_keys: e.partition_keys.clone(),
                        row_count_estimate: count,
                        columns: cols,
                        is_sampled: false,
                        full_row_count: None,
                        sheet: e.sheet.clone(),
                    };
                    let rec = source_record_from(&t, e, now, src);
                    let _ = db::upsert_source(&sqlite, &ws_path, &rec);
                    let _ = crate::okf::write_source_okf(&ws_dir_str, target, &e.label, &e.path, e.file_size as i64, e.mtime, &t.columns, t.row_count_estimate);
                    let _ = crate::okf::write_pipeline_okf(&ws_dir_str, target, &e.label, &e.path, &rec.storage);
                    result.push(t);
                } else {
                    let mut work = if storage == StorageKind::Table {
                        materialize_into_workspace(e, &ws_dir)?
                    } else {
                        e.clone()
                    };
                    work.view_name = target.clone();
                    let prog = progress.as_ref().map(|p| p as &dyn Fn(&str));
                    if let Some(p) = prog { p(target); }
                    match register::register(&guard, &work, storage, prog) {
                        Ok(t) => {
                            tracing::info!(category = "sync", "{}: new source → register", e.label);
                            lake_mutated = true;
                            let rec = source_record_from(&t, e, now, src);
                            let _ = db::upsert_source(&sqlite, &ws_path, &rec);
                            let _ = crate::okf::write_source_okf(&ws_dir_str, target, &e.label, &e.path, e.file_size as i64, e.mtime, &t.columns, t.row_count_estimate);
                            let _ = crate::okf::write_pipeline_okf(&ws_dir_str, target, &e.label, &e.path, &rec.storage);
                            result.push(t);
                        }
                        Err(err) => tracing::warn!(category = "sync", "register {} failed: {err}", e.label),
                    }
                }
            }
        }

        // Orphans: mapped but no longer on disk. Only on a full workspace sync —
        // never on a single-file import, where `entries` is just the imported file
        // and pruning would delete every other table in the workspace.
        if prune_orphans {
            for rec in &existing {
                if rec.kind == "postgres" || rec.kind == "mysql" || rec.kind == "sqlite" {
                    continue;
                }
                if !entry_identities.contains(&(rec.scan_path.clone(), rec.sheet.clone())) {
                    drop_lake_object(&guard, &rec.table_name);
                    let _ = db::delete_source_by_table(&sqlite, &ws_path, &rec.table_name);
                    let _ = crate::okf::delete_okf_files(&ws_dir_str, &rec.table_name);
                    lake_mutated = true;
                }
            }

            // Also clean up any other orphan/polluted tables/views starting with "s_"
            // in the DuckDB database that are not mapped in this workspace's sources
            // (either via filesystem files or external database tables).
            let mut active_s_names: HashSet<String> = final_names.iter().cloned().collect();
            for rec in &existing {
                if rec.kind == "postgres" || rec.kind == "mysql" || rec.kind == "sqlite" {
                    active_s_names.insert(rec.table_name.clone());
                }
            }
            for sql in CUSTOM_TABLE_SQLS {
                if let Ok(mut stmt) = guard.prepare(sql) {
                    if let Ok(mut rows) = stmt.query([]) {
                        while let Ok(Some(row)) = rows.next() {
                            if let Ok(name) = row.get::<_, String>(0) {
                                if name.starts_with("s_") && !active_s_names.contains(&name) {
                                    tracing::info!(category = "sync", "dropping untracked/polluted lake object: {name}");
                                    drop_lake_object(&guard, &name);
                                    let _ = db::delete_source_by_table(&sqlite, &ws_path, &name);
                                    let _ = crate::okf::delete_okf_files(&ws_dir_str, &name);
                                    lake_mutated = true;
                                }
                            }
                        }
                    }
                }
            }

            // Clean up custom tables/views in object_defs that are broken (fail probe)
            // and do not have corresponding OKF metadata files on disk.
            let okf_views_dir = Path::new(&ws_dir_str).join("okf").join("views");
            let okf_tables_dir = Path::new(&ws_dir_str).join("okf").join("tables");
            if let Ok(custom_defs) = db::list_object_defs(&sqlite, &ws_path) {
                for def in custom_defs {
                    let view_okf = okf_views_dir.join(format!("{}.md", def.table_name));
                    let table_okf = okf_tables_dir.join(format!("{}.md", def.table_name));
                    if !view_okf.exists() && !table_okf.exists() {
                        let probe = format!("SELECT * FROM \"{}\" LIMIT 0", def.table_name.replace('"', "\"\""));
                        if guard.execute(&probe, []).is_err() {
                            tracing::info!(category = "sync", "dropping polluted/broken custom object: {}", def.table_name);
                            drop_lake_object(&guard, &def.table_name);
                            let _ = db::delete_object_def(&sqlite, &ws_path, &def.table_name);
                            lake_mutated = true;
                        }
                    }
                }
            }
        }

        let db_sources: Vec<SourceTable> = existing.iter()
            .filter(|rec| rec.kind == "postgres" || rec.kind == "mysql")
            .map(build_source_table_from_record)
            .collect();
        result.extend(db_sources);

        // Checkpoint the DuckLake catalog so newly-created/updated tables are
        // flushed into lake.sqlite before the process exits. Skipped when nothing
        // in the lake changed (the common all-reused startup) — a 2s+ checkpoint
        // with no writes is pure waste.
        if lake_mutated {
            if let Err(e) = guard.execute("CHECKPOINT;", []) {
                tracing::warn!(category = "sync", "checkpoint warning: {e}");
            }
        }

        drop(guard);

        // 4. Refresh the in-memory cache.
        let mut cache = sources_cache.blocking_lock();
        cache.clear();
        cache.extend(result.iter().cloned());
        tracing::info!(category = "sync", "sync done: {} tables in {}ms", result.len(), sync_start.elapsed().as_millis());
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
// System prompt & shared tenets (read-only viewer for the Settings page)
// ===========================================================================

/// Return the fixed system prompt (PREAMBLE) sent to the model on every call.
/// Read-only — the PREAMBLE is a compile-time constant; this just exposes it
/// to the UI so users can inspect what the agent is told.
#[tauri::command]
pub async fn get_system_preamble() -> Result<String, String> {
    Ok(crate::usage::PREAMBLE.to_string())
}

/// List every concept in the shared tenets bundle as a catalog (concept_id,
/// title, description, tags, preview), sorted by concept_id. Read-only.
#[tauri::command]
pub async fn list_tenets() -> Result<Vec<crate::tenets::TenetHit>, String> {
    Ok(crate::tenets::list_all_tenets())
}

/// Load the full Markdown content (frontmatter + body) of one tenet by its
/// concept_id (e.g. `core/data-discipline`). Read-only.
#[tauri::command]
pub async fn get_tenet_content(concept_id: String) -> Result<String, String> {
    crate::tenets::load_tenet(&concept_id)
}

// ===========================================================================
// Filesystem commands
// ===========================================================================

/// Open a native platform directory picker and return the selected path.
/// `prompt` overrides the dialog title; when `None`, a workspace-oriented
/// default is used (this command is also used to add a workspace).
#[tauri::command]
pub async fn select_directory(prompt: Option<String>) -> Result<Option<String>, String> {
    let prompt = prompt.unwrap_or_else(|| "请选择工作区目录".to_string());
    // rfd's picker is a blocking call (it runs the native modal dialog), so run
    // it on a blocking worker to avoid stalling the Tauri async runtime while
    // the user browses.
    tauri::async_runtime::spawn_blocking(move || select_directory_native(&prompt))
        .await
        .map_err(|e| format!("目录选择器任务失败: {e}"))
}

/// Open a native platform file picker and return the selected file path.
#[tauri::command]
pub async fn select_file() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || select_file_native("请选择数据文件"))
        .await
        .map_err(|e| format!("文件选择器任务失败: {e}"))
}

/// Open a native multi-select file picker (files only; folder picking is
/// handled by `select_directory`). Replaces the old single-file picker for the
/// import flow so users can grab several files at once.
#[tauri::command]
pub async fn select_files() -> Result<Option<Vec<String>>, String> {
    tauri::async_runtime::spawn_blocking(move || select_files_native("请选择数据文件"))
        .await
        .map_err(|e| format!("文件选择器任务失败: {e}"))
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
    // read_dir + per-entry stat(): run on the blocking pool so a tokio worker
    // isn't stalled while the user browses the file tree.
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<FileItem>, String> {
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
                    || name == "okf"
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
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
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
    #[serde(rename = "tokenUsage", skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<serde_json::Value>,
}

#[tauri::command]
pub async fn load_workspace_tasks(workspace_path: String) -> Result<Vec<QueryTask>, String> {
    // SQLite + N file reads + N JSON parses: run on the blocking pool so we
    // don't stall a tokio worker thread (this runs on every task-list render).
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<QueryTask>, String> {
        let conn = db::get_db_conn()?;
        let mut stmt = conn
            .prepare("SELECT id, name, kind, created_at, saved, model_id, token_usage FROM tasks WHERE workspace_path = ? ORDER BY created_at ASC")
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
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let lakemind_dir = db::get_lakemind_dir()?;
        let mut tasks = Vec::new();
        for r in rows {
            if let Ok((id, name, kind, created_at, saved, model_id, token_usage_json)) = r {
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
                // Parse token_usage JSON if present.
                let token_usage = token_usage_json
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                tasks.push(QueryTask { id, name, sql, created_at, kind, messages, saved, model_id, token_usage });
            }
        }
        Ok(tasks)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
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
    token_usage: Option<serde_json::Value>,
) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    let now = now_ms();
    let usage_json = token_usage.map(|v| serde_json::to_string(&v).unwrap_or_default());
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, workspace_path, name, kind, created_at, saved, model_id, token_usage)
         VALUES (?, ?, ?, 'chat', COALESCE((SELECT created_at FROM tasks WHERE id = ?), ?), 1, ?, ?)",
        rusqlite::params![task_id, workspace_path, name, task_id, now, model_id, usage_json],
    )
    .map_err(|e| e.to_string())?;

    let lakemind_dir = db::get_lakemind_dir()?;
    let filepath = lakemind_dir.join("chats").join(format!("{task_id}.json"));
    let json_str = serde_json::to_string(&messages).map_err(|e| e.to_string())?;
    std::fs::write(filepath, json_str).map_err(|e| format!("Failed to write chat JSON file: {e}"))?;
    Ok(())
}

// ===========================================================================
// Unified logging commands
// ===========================================================================
//
// The frontend writes logs through `append_log` (one row per log call) so every
// UI-side error/event is persisted alongside backend tracing output. `query_logs`
// backs the console's history view and the future log-analysis module;
// `clear_logs` powers the console's clear button and retention sweeps.

/// Append one log row from the frontend. `ts` is filled server-side so callers
/// can't forge timestamps. The row is also emitted to the `app-log` channel so
/// other open windows (if any) see it live — but NOT echoed back to the sender,
// since the frontend logger already holds it in its in-memory signal.
#[tauri::command]
pub async fn append_log(
    app: tauri::AppHandle,
    mut record: crate::model::LogRecord,
) -> Result<i64, String> {
    use tauri::Emitter;
    record.ts = crate::db::now_ms();
    let conn = db::get_db_conn()?;
    let id = db::insert_log(&conn, &record)?;
    // Broadcast to other windows; the originating window already has it.
    let mut to_emit = record.clone();
    to_emit.id = Some(id);
    let _ = app.emit("app-log", &to_emit);
    Ok(id)
}

/// Query historical logs with optional filters. Returns newest-first.
#[tauri::command]
pub async fn query_logs(filter: crate::model::LogFilter) -> Result<Vec<crate::model::LogRecord>, String> {
    let conn = db::get_db_conn()?;
    db::query_logs(&conn, &filter)
}

/// Clear logs. `before = None` clears everything; `Some(ts)` deletes rows older
/// than `ts` (Unix ms) — used for retention sweeps.
#[tauri::command]
pub async fn clear_logs(before: Option<i64>) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::clear_logs(&conn, before)
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
    provider_id: Option<String>,
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
            provider_id,
            prompt,
            history_json,
            priority,
            confirm_mode,
            app_state,
        )
        .await
        {
            tracing::error!(category = "agent", "agent execution error: {e}");
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

/// Abort a running agent chat stream. Sets the abort flag so `run_stream_loop`
/// stops on the next iteration and emits "done" to unlock the frontend.
#[tauri::command]
pub async fn abort_chat(
    task_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut aborted = state.aborted_tasks.lock().await;
    aborted.insert(task_id);
    Ok(())
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

    // Auto-attach database connections for the switched workspace
    let _ = db::attach_workspace_connections(&new_conn, &workspace_path);

    // Update interrupt handle BEFORE swapping the connection so it's always
    // consistent (and available without locking conn).
    {
        let ih = new_conn.interrupt_handle();
        let mut stored = state.interrupt_handle.lock().unwrap();
        *stored = ih;
    }
    {
        let mut c = state.conn.lock().await;
        *c = new_conn;
    }
    *state.workspace_dir.lock().await = ws_dir;
    *state.workspace_path.lock().await = workspace_path;
    state.sources.lock().await.clear();
    Ok(())
}

/// Turn a remote-table read error into a clear, actionable Chinese message.
///
/// Postgres/MySQL table-level privilege failures (`permission denied for table`)
/// are matched by keyword and explained with the `GRANT SELECT` remedy; any other
/// failure (timeout, network, missing table) keeps the raw engine message so it
/// stays diagnosable. Used both for the pre-registration probe and as a safety
/// net around the actual CREATE statements.
fn friendly_remote_err(e: duckdb::Error, table: &str) -> String {
    let msg = e.to_string();
    if msg.to_lowercase().contains("permission denied") {
        format!(
            "无法读取远程表“{table}”：连接账号对该表没有 SELECT 权限（数据库返回 permission denied）。请用有授权的账号在数据库中执行 GRANT SELECT 后重试。\n\n原始错误：{msg}"
        )
    } else {
        format!("读取远程表“{table}”失败: {msg}")
    }
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
    // Use a bound parameter (single-quoted literal) for the name. Previously this
    // used a double-quoted identifier (`table_name="s_xxx"`) which DuckDB treats
    // as a column reference, not a string value — the comparison silently failed
    // (parse error → unwrap_or(0) → "not found"), forcing a rebuild every launch.
    let table_sql =
        "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name=?";
    if conn.query_row(table_sql, [name], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0 {
        return true;
    }
    let view_sql =
        "SELECT count(*) FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?";
    conn.query_row(view_sql, [name], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
}

fn count_rows(conn: &duckdb::Connection, name: &str) -> Option<i64> {
    // If it's a view (either registered source view or custom view), do NOT count rows
    // as it can trigger slow remote table scans or DuckDB optimizer crashes.
    let is_view = conn.query_row(
        "SELECT count(*) FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?",
        [name],
        |r| r.get::<_, i64>(0)
    ).unwrap_or(0) > 0;

    if is_view {
        return None;
    }

    let n = name.replace('"', "\"\"");
    conn.query_row(&format!("SELECT count(*) FROM \"{n}\""), [], |r| r.get::<_, i64>(0))
        .ok()
}

/// Build a SourceTable for a custom (agent-created) table/view. Prefers the
/// `object_defs` cache when the upstream fingerprint still matches — then
/// columns + row_count come straight from SQLite, no DuckDB query. Falls back to
/// live `describe_view` + `count_rows` when the cache is missing or stale.
fn hydrate_custom_object(
    conn: &duckdb::Connection,
    sqlite: &rusqlite::Connection,
    ws_path: &str,
    name: &str,
) -> SourceTable {
    // Try the cache: valid only if the persisted input_hash still matches.
    if let Ok(Some(def)) = db::get_object_def(sqlite, ws_path, name) {
        let upstreams = fingerprint::extract_upstreams(&def.select_sql);
        let current_hash = fingerprint::compute_input_hash(sqlite, ws_path, &def.select_sql, &upstreams);
        if current_hash == def.input_hash {
            return SourceTable {
                name: name.to_string(),
                label: name.to_string(),
                kind: SourceKind::View,
                storage: StorageKind::Custom,
                path: String::new(),
                scan_path: String::new(),
                partition_keys: Vec::new(),
                row_count_estimate: def.row_count,
                columns: def.columns,
                is_sampled: false,
                full_row_count: None,
                sheet: None,
            };
        }
    }
    // Cache miss or stale → live hydrate.
    let cols = schema::describe_view(conn, name).unwrap_or_default();
    // Do NOT run count_rows for custom views/tables live, as they can be extremely slow
    // or trigger optimizer bugs when querying large external database tables.
    let count = None;
    SourceTable {
        name: name.to_string(),
        label: name.to_string(),
        kind: SourceKind::View,
        storage: StorageKind::Custom,
        path: String::new(),
        scan_path: String::new(),
        partition_keys: Vec::new(),
        row_count_estimate: count,
        columns: cols,
        is_sampled: false,
        full_row_count: None,
        sheet: None,
    }
}

/// Lazily INSTALL+LOAD the delta/excel extensions only if such a source is present.
fn load_extensions_if_needed(conn: &duckdb::Connection, entries: &[scan::ScanEntry]) {
    let needs_delta = entries.iter().any(|e| e.kind == SourceKind::Delta);
    if needs_delta {
        if conn.execute("LOAD delta;", []).is_err() {
            let _ = conn.execute("INSTALL delta;", []);
            let _ = conn.execute("LOAD delta;", []);
        }
    }
    let needs_excel = entries.iter().any(|e| e.kind == SourceKind::Excel);
    if needs_excel {
        if conn.execute("LOAD excel;", []).is_err() {
            let _ = conn.execute("INSTALL excel;", []);
            let _ = conn.execute("LOAD excel;", []);
        }
    }
}

/// Build a [`SourceTable`] purely from the cached mapping record — no DuckDB
/// query at all. Both `columns` and `row_count_estimate` come straight from
/// SQLite; they're valid as long as the source fingerprint matches (refreshed on
/// rebuild). This is what makes the startup list millisecond-fast.
fn build_source_table_from_record(rec: &SourceRecord) -> SourceTable {
    SourceTable {
        name: rec.table_name.clone(),
        label: rec.label.clone(),
        kind: str_to_kind(&rec.kind),
        storage: StorageKind::from_db_str(&rec.storage),
        path: rec.file_path.clone(),
        scan_path: rec.scan_path.clone(),
        partition_keys: rec.partition_keys.clone(),
        row_count_estimate: rec.row_count,
        columns: rec.columns.clone(),
        is_sampled: rec.is_sampled,
        full_row_count: rec.full_row_count,
        sheet: rec.sheet.clone(),
    }
}

/// Build a mapping record from a freshly-registered source. The file
/// fingerprint (mtime/size) is taken from the originating `ScanEntry`; the
/// cached columns + row count come from the `SourceTable` (already computed by
/// `register`) so the startup list can skip DuckDB entirely.
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
        columns: t.columns.clone(),
        row_count: t.row_count_estimate,
        is_sampled: t.is_sampled,
        full_row_count: t.full_row_count,
        materialize_status: if t.is_sampled {
            Some(db::mat_status::SAMPLED.to_string())
        } else {
            Some(db::mat_status::FULL.to_string())
        },
        sheet: e.sheet.clone(),
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
        SourceKind::Postgres => "postgres",
        SourceKind::Mysql => "mysql",
        SourceKind::Sqlite => "sqlite",
        SourceKind::Maxcompute => "maxcompute",
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
        "postgres" => SourceKind::Postgres,
        "mysql" => SourceKind::Mysql,
        "sqlite" => SourceKind::Sqlite,
        "maxcompute" => SourceKind::Maxcompute,
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

fn select_directory_native(prompt: &str) -> Option<String> {
    // rfd talks to the native file-dialog API directly (Win32 IFileOpenDialog,
    // macOS NSOpenPanel, xdg-desktop-portal), so non-ASCII paths round-trip as
    // correct Unicode. The previous osascript/powershell/zenity shelling read
    // the path back through stdout, which on a CJK Windows is CP936(GBK) and was
    // decoded as UTF-8 — silently mangling every Chinese path into mojibake.
    rfd::FileDialog::new()
        .set_title(prompt)
        .pick_folder()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

/// Native single-file picker. Mirrors [`select_directory_native`] but opens an
/// open-file dialog. No extension filter is enforced — the scan layer classifies
/// the file, and unrecognized types surface a clear error upstream.
fn select_file_native(prompt: &str) -> Option<String> {
    // See `select_directory_native` for why rfd is used instead of shelling out
    // to osascript/powershell/zenity (CJK path corruption via GBK stdout).
    rfd::FileDialog::new()
        .set_title(prompt)
        .pick_file()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

/// Native multi-select file picker (files only).
fn select_files_native(prompt: &str) -> Option<Vec<String>> {
    rfd::FileDialog::new()
        .set_title(prompt)
        .pick_files()
        .map(|paths| {
            paths
                .into_iter()
                .filter_map(|p| p.to_str().map(|s| s.to_string()))
                .collect()
        })
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

// ===========================================================================
// Database connection commands
// ===========================================================================

fn test_connection_impl(
    r: &db::DbConnectionRecord,
    paths: &crate::external::paths::SidecarPaths,
) -> Result<(), String> {
    // Sidecar DB types (MaxCompute, future generic JDBC) — route to the Java
    // sidecar instead of DuckDB ATTACH (no DuckDB extension exists for these).
    if db::is_sidecar_db_type(&r.db_type) {
        let opts = r.maxcompute_opts();
        let jars = paths.driver_jars(&opts.driver_coord)?;
        let launcher = paths.dbx_launcher()?;
        let mut sc = crate::external::jdbc_sidecar::JdbcSidecar::spawn(&launcher)?;
        let conn_obj = crate::external::jdbc_sidecar::build_maxcompute_connection(r, &jars)?;
        let res = sc.test_connection(&conn_obj);
        sc.close();
        return res;
    }
    let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    let load_sql = format!("LOAD {};", r.db_type);
    if conn.execute(&load_sql, []).is_err() {
        let install_sql = format!("INSTALL {};", r.db_type);
        let _ = conn.execute(&install_sql, []);
        conn.execute(&load_sql, []).map_err(|e| format!("加载驱动失败: {e}"))?;
    }

    let conn_name = "test_attached_db";
    let attach_sql = db::build_attach_sql(r, conn_name);

    conn.execute(&attach_sql, []).map_err(|e| format!("连接数据库失败: {e}"))?;
    let _ = conn.execute(&format!("DETACH {conn_name};"), []);
    Ok(())
}

fn list_connection_tables_impl(r: &db::DbConnectionRecord) -> Result<Vec<DbTableItem>, String> {
    let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    let load_sql = format!("LOAD {};", r.db_type);
    if conn.execute(&load_sql, []).is_err() {
        let install_sql = format!("INSTALL {};", r.db_type);
        let _ = conn.execute(&install_sql, []);
        conn.execute(&load_sql, []).map_err(|e| format!("加载驱动失败: {e}"))?;
    }

    let conn_name = "list_attached_db";
    let attach_sql = db::build_attach_sql(r, conn_name);

    conn.execute(&attach_sql, []).map_err(|e| format!("连接数据库失败: {e}"))?;

    let out = query_attached_tables(&conn, conn_name)?;
    let _ = conn.execute(&format!("DETACH {conn_name};"), []);
    Ok(out)
}

/// List user tables/views already ATTACHed under `alias` on `conn`.
/// Pulled out of `list_connection_tables_impl` so the fast path (querying the
/// shared session connection, where workspace connections are already attached
/// as `db_{safe_name}`) and the slow fallback path share one implementation.
fn query_attached_tables(conn: &duckdb::Connection, alias: &str) -> Result<Vec<DbTableItem>, String> {
    let mut stmt = conn.prepare(&format!(
        "SELECT schema_name, table_name, 'table' AS type FROM duckdb_tables() WHERE database_name = '{alias}' AND NOT internal
         UNION ALL
         SELECT schema_name, view_name AS table_name, 'view' AS type FROM duckdb_views() WHERE database_name = '{alias}' AND NOT internal
         ORDER BY schema_name, table_name"
    )).map_err(|e| e.to_string())?;

    let rows = stmt.query_map([], |row| {
        Ok(DbTableItem {
            schema: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
        })
    }).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        if let Ok(item) = r {
            out.push(item);
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_db_connections() -> Result<Vec<db::DbConnectionRecord>, String> {
    let conn = db::get_db_conn()?;
    db::list_db_connections(&conn)
}

#[tauri::command]
pub async fn upsert_db_connection(config: db::DbConnectionRecord) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    if db::get_db_connection(&conn, &config.id)?.is_some() {
        db::update_db_connection(&conn, &config)
    } else {
        db::create_db_connection(&conn, &config)
    }
}

#[tauri::command]
pub async fn delete_db_connection(id: String) -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::delete_db_connection(&conn, &id)
}

#[tauri::command]
pub async fn test_db_connection(
    app: tauri::AppHandle,
    config: db::DbConnectionRecord,
) -> Result<(), String> {
    let paths = crate::external::paths::SidecarPaths::resolve(&app)?;
    tokio::task::spawn_blocking(move || test_connection_impl(&config, &paths))
        .await
        .map_err(|e| format!("Task execution error: {e}"))?
}

/// Check whether a Java runtime (JRE 17+) is available for the MaxCompute
/// sidecar. Returns the first `java -version` line on success (e.g.
/// `openjdk version "17.0.19"`) so the frontend can show it.
#[tauri::command]
pub async fn check_java_runtime() -> Result<String, String> {
    tokio::task::spawn_blocking(crate::external::jdbc_sidecar::check_java_runtime)
        .await
        .map_err(|e| format!("Task execution error: {e}"))?
}

/// Test an LLM provider/model against the exact config shown in the settings
/// form (not the saved settings.json), so unsaved edits are reflected. Sends a
/// minimal prompt with an 8s hard timeout and returns a friendly error on
/// failure. Mirrors `test_db_connection`'s "test before you trust" pattern.
#[tauri::command]
pub async fn test_llm_connection(
    endpoint: String,
    api_key: String,
    api_format: String,
    model_id: String,
) -> Result<(), String> {
    const TEST_TIMEOUT_SECS: u64 = 8;
    match tokio::time::timeout(
        std::time::Duration::from_secs(TEST_TIMEOUT_SECS),
        crate::agent::test_connection(&endpoint, &api_key, &api_format, &model_id),
    )
    .await
    {
        Ok(inner) => inner,
        Err(_elapsed) => Err(format!(
            "连接测试超时（{TEST_TIMEOUT_SECS} 秒未收到响应）。请检查网络或 Base URL 是否可访问。"
        )),
    }
}

#[tauri::command]
pub async fn link_connection_to_workspace(
    ws_path: String,
    conn_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Persist the link in SQLite first.
    let sqlite = db::get_db_conn()?;
    db::link_connection_to_workspace(&sqlite, &ws_path, &conn_id)?;

    // Fetch the full record so we can ATTACH it to the live session connection.
    let record = db::get_db_connection(&sqlite, &conn_id)?
        .ok_or_else(|| format!("未找到数据库连接: {}", conn_id))?;

    // Immediately ATTACH on the shared session so "enabled" is truthful. Run it
    // on a blocking thread (attach does a synchronous driver load + server
    // handshake) and take the tokio mutex via blocking_lock — never hold the
    // async lock across a blocking DuckDB call, that stalls the runtime.
    let conn_arc = state.conn.clone();
    let name_for_log = record.name.clone();
    let attach_result = tauri::async_runtime::spawn_blocking(move || {
        let guard = conn_arc.blocking_lock();
        db::attach_one(&guard, &record)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?;

    if let Err(e) = attach_result {
        tracing::warn!(category = "link", "ATTACH failed for {}: {e}", name_for_log);
        // Roll back the persisted link so the UI doesn't show "enabled" while
        // the database isn't actually attached.
        let _ = db::unlink_connection_from_workspace(&sqlite, &ws_path, &conn_id);
        return Err(format!("启用连接失败: {e}"));
    }
    tracing::info!(category = "link", "{} attached to session", name_for_log);
    Ok(())
}

#[tauri::command]
pub async fn unlink_connection_from_workspace(
    ws_path: String,
    conn_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Fetch the record (by id) before removing the link so we know the name to
    // DETACH. If it's already gone, there's nothing to detach.
    let sqlite = db::get_db_conn()?;
    let record = db::get_db_connection(&sqlite, &conn_id)?;

    // Remove the persisted link first.
    db::unlink_connection_from_workspace(&sqlite, &ws_path, &conn_id)?;

    // Drop the cached table list so a future re-enable fetches fresh data
    // instead of showing a stale snapshot.
    let _ = db::clear_db_connection_tables_cache(&sqlite, &conn_id);

    // Immediately DETACH from the live session so "disabled" is truthful.
    if let Some(r) = record {
        if db::is_sidecar_db_type(&r.db_type) {
            // Sidecar connections were never ATTACHed — nothing to DETACH.
            tracing::info!(category = "link", "{} (sidecar) — skipped DETACH", r.name);
        } else {
            let conn_arc = state.conn.clone();
            let name_for_log = r.name.clone();
            let res = tauri::async_runtime::spawn_blocking(move || {
                let guard = conn_arc.blocking_lock();
                db::detach_one(&guard, &r.name)
            })
            .await
            .map_err(|e| format!("task join error: {e}"))?;
            // DETACH may fail if the database was never attached — that's fine.
            match res {
                Ok(()) => tracing::info!(category = "link", "{} detached from session", name_for_log),
                Err(e) => tracing::warn!(category = "link", "detach warning for {}: {e}", name_for_log),
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn list_workspace_connections(ws_path: String) -> Result<Vec<db::DbConnectionRecord>, String> {
    let conn = db::get_db_conn()?;
    db::list_workspace_connections(&conn, &ws_path)
}

#[derive(serde::Serialize)]
pub struct DbTableItem {
    pub schema: String,
    pub name: String,
    pub kind: String, // "table" | "view"
}

/// Sidecar path: list a MaxCompute project's tables via the dbx JDBC sidecar
/// (no DuckDB ATTACH). Returns flat table names (schema empty).
fn list_maxcompute_tables(
    r: &db::DbConnectionRecord,
    paths: &crate::external::paths::SidecarPaths,
) -> Result<Vec<DbTableItem>, String> {
    let opts = r.maxcompute_opts();
    let jars = paths.driver_jars(&opts.driver_coord)?;
    let launcher = paths.dbx_launcher()?;
    let mut sc = crate::external::jdbc_sidecar::JdbcSidecar::spawn(&launcher)?;
    let conn_obj = crate::external::jdbc_sidecar::build_maxcompute_connection(r, &jars)?;
    let names = sc.list_tables(&conn_obj, &opts.project, 5000);
    sc.close();
    let names = names?;
    Ok(names
        .into_iter()
        .map(|n| DbTableItem { schema: String::new(), name: n, kind: "table".to_string() })
        .collect())
}

#[tauri::command]
pub async fn list_db_connection_tables(
    app: tauri::AppHandle,
    config: db::DbConnectionRecord,
    force_refresh: Option<bool>,
    state: State<'_, AppState>,
) -> Result<Vec<DbTableItem>, String> {
    let conn_id = config.id.clone();
    let conn_name = config.name.clone();
    let force = force_refresh == Some(true);

    // ── Cache-first. Listing tables from a remote catalog via DuckDB is slow
    //    (~2s — it scans the server's system tables every time), so serve from
    //    the local SQLite cache unless an explicit refresh is requested. SQLite
    //    reads are blocking, so run on a blocking thread.
    if !force {
        let cid = conn_id.clone();
        let cname = conn_name.clone();
        let cached = tauri::async_runtime::spawn_blocking(move || {
            let sqlite = db::get_db_conn()?;
            db::list_db_connection_tables_cache(&sqlite, &cid)
        })
        .await
        .map_err(|e| format!("task join error: {e}"))?
        .unwrap_or_default();
        if !cached.is_empty() {
            tracing::info!(category = "link", "db_tables cache hit: {} ({} tables)", cname, cached.len());
            return Ok(cached
                .into_iter()
                .map(|c| DbTableItem { schema: c.schema, name: c.name, kind: c.kind })
                .collect());
        }
    }

    // ── Cache miss / forced refresh: query the live database. Sidecar DB types
    //    (MaxCompute) route through the Java sidecar; ATTACH-based types try the
    //    shared session first, then fall back to a throwaway ATTACH connection.
    let tables: Vec<DbTableItem> = if db::is_sidecar_db_type(&config.db_type) {
        let paths = crate::external::paths::SidecarPaths::resolve(&app)?;
        let cfg = config.clone();
        tauri::async_runtime::spawn_blocking(move || list_maxcompute_tables(&cfg, &paths))
            .await
            .map_err(|e| format!("task join error: {e}"))?
            .map_err(|e| e)?
    } else {
        let alias = db::workspace_attach_alias(&conn_name);
        let conn_arc = state.conn.clone();
        let alias_clone = alias.clone();
        let name_clone = conn_name.clone();
        let start = std::time::Instant::now();
        // Use try_lock instead of blocking_lock to avoid a deadlock: if another
        // async task (e.g. an agent stream) is holding the session connection,
        // blocking_lock would stall the spawn_blocking thread forever, causing the
        // frontend spinner to never stop. When the lock is unavailable, we fall
        // through to the fallback path that opens a fresh throwaway connection.
        match tauri::async_runtime::spawn_blocking(move || {
            match conn_arc.try_lock() {
                Ok(guard) => query_attached_tables(&guard, &alias_clone),
                Err(_) => Err("session connection busy, will use fresh connection".to_string()),
            }
        })
        .await
        .map_err(|e| format!("task join error: {e}"))?
        {
            Ok(t) => {
                tracing::info!(
                    category = "link",
                    "db_tables live query (attached) {} -> {} tables in {}ms",
                    name_clone,
                    t.len(),
                    start.elapsed().as_millis()
                );
                t
            }
            Err(e) => {
                tracing::warn!(
                    category = "link",
                    "db_tables attached query failed for {} ({}), falling back to fresh connection",
                    conn_name, e
                );
                let config2 = config.clone();
                tokio::task::spawn_blocking(move || list_connection_tables_impl(&config2))
                    .await
                    .map_err(|e| format!("Task execution error: {e}"))?
                    .map_err(|e| e)?
            }
        }
    };

    // Persist the fresh result so subsequent clicks are instant. Run on a
    // blocking thread (SQLite writes block).
    let cached_items: Vec<db::CachedDbTable> = tables
        .iter()
        .map(|t| db::CachedDbTable { schema: t.schema.clone(), name: t.name.clone(), kind: t.kind.clone() })
        .collect();
    let cid2 = conn_id.clone();
    let cname2 = conn_name.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let mut sqlite = db::get_db_conn()?;
        db::save_db_connection_tables(&mut sqlite, &cid2, &cached_items)?;
        tracing::info!(category = "link", "db_tables cached {} tables for {}", cached_items.len(), cname2);
        Ok(())
    })
    .await;

    tracing::info!(category = "link", "db_tables returning {} tables for {}", tables.len(), conn_name);
    Ok(tables)
}

/// MaxCompute registration path: count via the dbx JDBC sidecar, bulk-materialize
/// the table into the lake via the Arrow sidecar, then register a local source.
/// (No DuckDB ATTACH — there's no MaxCompute extension.)
async fn register_maxcompute_table(
    workspace: String,
    connection_id: String,
    schema_name: String,
    table_name: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<SourceTable>, String> {
    let paths = crate::external::paths::SidecarPaths::resolve(&app)?;
    let ws_dir = resolve_workspace_dir(&workspace)?;
    let ws_dir_str = ws_dir.to_string_lossy().to_string();
    let conn_arc = state.conn.clone();
    let sources_clone = state.sources.clone();
    let ws_path_clone = workspace.clone();

    let res = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let sqlite = db::get_db_conn()?;
        let conn_record = db::get_db_connection(&sqlite, &connection_id)?
            .ok_or_else(|| format!("未找到数据库连接: {connection_id}"))?;
        let opts = conn_record.maxcompute_opts();
        let table_ref = conn_record.maxcompute_table_ref(&table_name);

        let safe_conn_name = conn_record.name.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
            .collect::<String>();
        let local_table_name = format!("s_{}_{}", safe_conn_name, table_name)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
            .collect::<String>();

        // 1) row count via the JDBC sidecar (1-row result; instance-tunnel cap N/A)
        let launcher = paths.dbx_launcher()?;
        let jars = paths.driver_jars(&opts.driver_coord)?;
        let mut sc = crate::external::jdbc_sidecar::JdbcSidecar::spawn(&launcher)?;
        let conn_obj = crate::external::jdbc_sidecar::build_maxcompute_connection(&conn_record, &jars)?;
        let count_sql = format!("SELECT count(*) AS c FROM {table_ref}");
        let (_, count_rows) = sc.execute_query(&conn_obj, &count_sql, 1)?;
        sc.close();
        let full_rows = count_rows.get(0)
            .and_then(|r| r.get(0))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 2) bulk-materialize via the Arrow sidecar into the lake
        let arrow_jar = paths.arrow_jar()?;
        let guard = conn_arc.blocking_lock();
        let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{local_table_name}\";"), []);
        let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{local_table_name}\";"), []);
        let stats = crate::external::arrow_sidecar::pull_table(
            &guard, &conn_record, &opts, &table_ref, &local_table_name,
            &arrow_jar, &jars, 0, 0,
        )?;
        let columns = schema::describe_view(&guard, &local_table_name)?;
        drop(guard);

        // 3) register the local materialized table as a source
        let now = now_ms();
        let record = db::SourceRecord {
            table_name: local_table_name.clone(),
            label: format!("{}.{}.{}", conn_record.name, schema_name, table_name),
            kind: "maxcompute".to_string(),
            storage: "table".to_string(),
            file_path: format!("maxcompute://{}/{}/{}", connection_id, opts.project, table_name),
            scan_path: local_table_name.clone(),
            partition_keys: Vec::new(),
            created_at: now,
            name_source: "llm".to_string(),
            file_mtime: now,
            file_size: full_rows,
            columns: columns.clone(),
            row_count: Some(stats.rows as i64),
            is_sampled: false,
            full_row_count: Some(full_rows),
            materialize_status: Some(db::mat_status::FULL.to_string()),
            sheet: None,
        };
        db::upsert_source(&sqlite, &ws_path_clone, &record)?;
        let _ = crate::okf::write_source_okf(
            &ws_dir_str, &local_table_name, &record.label, &record.file_path,
            full_rows, now, &columns, Some(full_rows));
        let _ = crate::okf::write_pipeline_okf(
            &ws_dir_str, &local_table_name, &record.label, &record.file_path, "table");

        let records = db::list_sources(&sqlite, &ws_path_clone)?;
        let mut result: Vec<SourceTable> = records.iter().map(build_source_table_from_record).collect();
        let defs = db::list_object_defs(&sqlite, &ws_path_clone)?;
        for d in &defs {
            result.push(SourceTable {
                name: d.table_name.clone(), label: d.table_name.clone(),
                kind: SourceKind::View, storage: StorageKind::Custom,
                path: String::new(), scan_path: String::new(), partition_keys: Vec::new(),
                row_count_estimate: d.row_count, columns: d.columns.clone(),
                is_sampled: false, full_row_count: None, sheet: None,
            });
        }
        let mut cache = sources_clone.blocking_lock();
        cache.clear();
        cache.extend(result.iter().cloned()
            .filter(|r| r.kind != SourceKind::View || r.storage != StorageKind::Custom));
        Ok(result)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
    .map_err(|e| e.to_string())?;
    Ok(res)
}

#[tauri::command]
pub async fn register_database_table(
    app: tauri::AppHandle,
    workspace: String,
    connection_id: String,
    schema_name: String,
    table_name: String,
    db_type: String, // "postgres" | "mysql" | "maxcompute"
    state: State<'_, AppState>,
) -> Result<Vec<SourceTable>, String> {
    // MaxCompute (and future sidecar types) don't ATTACH via DuckDB — route to
    // the sidecar-based registration (count via JDBC, bulk pull via Arrow tunnel).
    if db::is_sidecar_db_type(&db_type) {
        return register_maxcompute_table(workspace, connection_id, schema_name, table_name, state, app).await;
    }
    let sqlite = db::get_db_conn()?;
    let conn_record = db::get_db_connection(&sqlite, &connection_id)?
        .ok_or_else(|| format!("未找到数据库连接: {}", connection_id))?;
    
    let safe_conn_name = conn_record.name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>();
    let catalog_name = format!("db_{safe_conn_name}");
    
    let full_path = if db_type == "postgres" {
        format!("{}.{}.{}", catalog_name, schema_name, table_name)
    } else {
        format!("{}.{}", catalog_name, table_name)
    };
    
    let local_table_name = format!("s_{}_{}", safe_conn_name, table_name)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>();
    
    let ws_dir = resolve_workspace_dir(&workspace)?;
    let ws_dir_str = ws_dir.to_string_lossy().to_string();
    
    let conn_clone = state.conn.clone();
    let sources_clone = state.sources.clone();
    let ws_path_clone = workspace.clone();
    
    let res = tauri::async_runtime::spawn_blocking(move || -> AppResult<Vec<SourceTable>> {
        let sqlite = db::get_db_conn()?;

        // Read sampling config before taking the DuckDB lock — these SQLite
        // reads don't need the connection and shouldn't block other queries.
        let sample_enabled = db::get_config(&sqlite, "explore.materialized_sample_enabled")
            .unwrap_or(None)
            .map(|v| v == "true")
            .unwrap_or(true);
        let sample_limit = db::get_config(&sqlite, "explore.materialized_sample_limit")
            .unwrap_or(None)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10000);

        let guard = conn_clone.blocking_lock();
        
        let catalog_exists = guard.prepare(&format!("SELECT * FROM duckdb_databases() WHERE database_name = '{}'", catalog_name))?
            .query_row([], |_| Ok(()))
            .is_ok();
        
        if !catalog_exists {
            let load_sql = format!("LOAD {};", conn_record.db_type);
            if guard.execute(&load_sql, []).is_err() {
                let install_sql = format!("INSTALL {};", conn_record.db_type);
                let _ = guard.execute(&install_sql, []);
                guard.execute(&load_sql, [])?;
            }
            
            let attach_sql = db::build_attach_sql(&conn_record, &catalog_name);
            guard.execute(&attach_sql, [])?;
        }

        // 1. Estimate the row count for the external table. Postgres' planner
        // statistics (pg_class.reltuples / pg_stat_all_tables.n_live_tup) are
        // instant, but return -1/0 for tables never ANALYZE'd.
        // We do NOT fall back to a remote SELECT COUNT(*) because that will cause timeouts on large tables.
        // MySQL's information_schema.table_rows is an estimate that's effectively always present.
        // SQLite has no statistics catalog; for a local file we run an actual
        // COUNT(*) via the attached scanner — cheap, and we want the exact count
        // since the table will be fully materialized next.
        let is_sqlite = conn_record.db_type == "sqlite";
        let (write_count, approx_rows) = if conn_record.db_type == "postgres" {
            let stats_sql = format!(
                "SELECT reltuples::BIGINT,
                       COALESCE((SELECT n_live_tup
                                 FROM {catalog_name}.pg_catalog.pg_stat_all_tables
                                 WHERE schemaname = '{}' AND relname = '{}'), 0)::BIGINT,
                       (SELECT COALESCE(MAX(n_tup_ins + n_tup_upd + n_tup_del), 0)
                        FROM {catalog_name}.pg_catalog.pg_stat_all_tables
                        WHERE schemaname = '{}' AND relname = '{}')
                 FROM {catalog_name}.pg_catalog.pg_class
                 JOIN {catalog_name}.pg_catalog.pg_namespace ON pg_namespace.oid = pg_class.relnamespace
                 WHERE pg_namespace.nspname = '{}' AND pg_class.relname = '{}';",
                schema_name, table_name, schema_name, table_name, schema_name, table_name
            );
            let (reltuples, n_live_tup, writes) = guard
                .query_row(&stats_sql, [], |row| {
                    Ok((
                        row.get::<_, i64>(0).unwrap_or(0),
                        row.get::<_, i64>(1).unwrap_or(0),
                        row.get::<_, i64>(2).unwrap_or(0),
                    ))
                })
                .unwrap_or((0, 0, 0));
            // Prefer the planner estimate, then the live-tuple counter.
            let mut rows = reltuples;
            if rows <= 0 {
                rows = n_live_tup;
            }
            (writes, rows)
        } else if is_sqlite {
            let count_sql = format!("SELECT COUNT(*)::BIGINT FROM {};", full_path);
            let rows = guard
                .query_row(&count_sql, [], |row| row.get::<_, i64>(0))
                .unwrap_or(0);
            (0, rows)
        } else {
            let stats_sql = format!(
                "SELECT COALESCE(table_rows, 0)::BIGINT, 
                        COALESCE(UNIX_TIMESTAMP(update_time), 0)::BIGINT 
                 FROM {catalog_name}.information_schema.tables 
                 WHERE table_schema = '{}' AND table_name = '{}';",
                conn_record.database_name, table_name
            );
            guard.query_row(&stats_sql, [], |row| Ok((row.get::<_, i64>(1).unwrap_or(0), row.get::<_, i64>(0).unwrap_or(0))))
                .unwrap_or((0, 0))
        };

        let row_count_opt = if approx_rows > 0 { Some(approx_rows) } else { None };

        // 2. Sampling config was read above, before the DuckDB lock.

        // 3. Decide between (a) a small local sample, (b) a zero-copy view over
        // the remote table, or (c) a full local materialization.
        // SQLite is always fully materialized: it's a local file so there is no
        // network cost to copy, and a full table avoids sample-guard / pushdown
        // machinery (DuckDB has no `sqlite_query` function). Postgres/MySQL use
        // the sample-vs-view tradeoff driven by the estimated row count.
        //
        // Pre-registration read probe for remote tables: the zero-copy VIEW
        // branch below (`CREATE VIEW ... AS SELECT * FROM <remote>`) is LAZY —
        // it stores only the query definition and does NOT touch table data, so
        // a table the connection account lacks SELECT privilege on would
        // register silently and only fail on the first "view data" click. The
        // row estimate (reltuples) and schema (DESCRIBE) read only catalog
        // metadata, which does not require table-level SELECT either. A real
        // `SELECT ... LIMIT 1` forces DuckDB to push down `COPY (...) TO
        // STDOUT`, which provokes the table-level privilege check NOW, with a
        // clear message instead of a silent success. (LIMIT 0 can be optimized
        // to a metadata-only plan and thus may NOT trigger the check.)
        let is_remote = conn_record.db_type == "postgres" || conn_record.db_type == "mysql";
        if is_remote {
            let probe_sql = format!("SELECT * FROM {} LIMIT 1", full_path);
            guard.execute(&probe_sql, []).map_err(|e| AppError::new(friendly_remote_err(e, &table_name)))?;
        }

        let do_sample = !is_sqlite && sample_enabled && (approx_rows > sample_limit as i64 || approx_rows <= 0);
        let do_full_materialize = is_sqlite;
        let (storage_kind, is_sampled, row_count_to_save) = if do_sample {
            let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_table_name), []);
            let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", local_table_name), []);

            let create_table_sql = format!(
                "CREATE TABLE \"{}\" AS SELECT * FROM {} LIMIT {};",
                local_table_name, full_path, sample_limit
            );
            guard.execute(&create_table_sql, []).map_err(|e| AppError::new(friendly_remote_err(e, &table_name)))?;

            // Get actual number of rows imported locally
            let count_sql = format!("SELECT COUNT(*)::BIGINT FROM \"{}\";", local_table_name);
            let imported_rows = guard.query_row(&count_sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0);

            ("table".to_string(), true, Some(imported_rows))
        } else if do_full_materialize {
            let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_table_name), []);
            let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", local_table_name), []);

            let create_table_sql = format!(
                "CREATE TABLE \"{}\" AS SELECT * FROM {};",
                local_table_name, full_path
            );
            guard.execute(&create_table_sql, [])?;

            let count_sql = format!("SELECT COUNT(*)::BIGINT FROM \"{}\";", local_table_name);
            let imported_rows = guard.query_row(&count_sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0);

            ("table".to_string(), false, Some(imported_rows))
        } else {
            let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_table_name), []);
            let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", local_table_name), []);

            let create_view_sql = format!("CREATE VIEW \"{}\" AS SELECT * FROM {};", local_table_name, full_path);
            // CREATE VIEW is lazy, so a privilege failure surfaces only at query
            // time — wrap it as a safety net (the pre-registration probe above is
            // the primary guard).
            guard.execute(&create_view_sql, []).map_err(|e| AppError::new(friendly_remote_err(e, &table_name)))?;

            ("view".to_string(), false, row_count_opt)
        };
        
        let columns = schema::describe_view(&guard, &local_table_name)?;
        // All DuckDB work is done — release the connection lock before the
        // SQLite/OKF writes and cache refresh so they don't block other queries.
        drop(guard);

        let now = now_ms();
        let record = db::SourceRecord {
            table_name: local_table_name.clone(),
            label: format!("{}.{}.{}", conn_record.name, schema_name, table_name),
            kind: db_type.clone(),
            storage: storage_kind,
            file_path: format!("db://{}/{}/{}", connection_id, schema_name, table_name),
            scan_path: full_path.clone(),
            partition_keys: Vec::new(),
            created_at: now,
            name_source: "llm".to_string(),
            file_mtime: write_count,
            file_size: row_count_opt.unwrap_or(0),
            columns: columns.clone(),
            row_count: row_count_to_save,
            is_sampled,
            full_row_count: row_count_opt,
            materialize_status: if is_sampled {
                Some(db::mat_status::SAMPLED.to_string())
            } else {
                Some(db::mat_status::FULL.to_string())
            },
            sheet: None,
        };
        
        db::upsert_source(&sqlite, &ws_path_clone, &record)?;
        let _ = crate::okf::write_source_okf(&ws_dir_str, &local_table_name, &record.label, &record.file_path, row_count_opt.unwrap_or(0), write_count, &columns, row_count_opt);
        let _ = crate::okf::write_pipeline_okf(&ws_dir_str, &local_table_name, &record.label, &record.file_path, "view");
        
        let records = db::list_sources(&sqlite, &ws_path_clone)?;
        let mut result: Vec<SourceTable> = records.iter().map(build_source_table_from_record).collect();
        
        let defs = db::list_object_defs(&sqlite, &ws_path_clone)?;
        for d in &defs {
            result.push(SourceTable {
                name: d.table_name.clone(),
                label: d.table_name.clone(),
                kind: SourceKind::View,
                storage: StorageKind::Custom,
                path: String::new(),
                scan_path: String::new(),
                partition_keys: Vec::new(),
                row_count_estimate: d.row_count,
                columns: d.columns.clone(),
                is_sampled: false,
                full_row_count: None,
                sheet: None,
            });
        }
        
        let mut cache = sources_clone.blocking_lock();
        cache.clear();
        cache.extend(result.iter().cloned().filter(|r| r.kind != SourceKind::View || r.storage != StorageKind::Custom));
        
        Ok(result)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
    .map_err(|e| e.to_string())?;
    
    Ok(res)
}
#[tauri::command]
pub async fn get_table_ddl(
    table_name: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let ws_path = state.workspace_path.lock().await.clone();
    let sqlite = db::get_db_conn()?;

    // 1. Try to get it from object_defs first (for custom views/tables t_/v_)
    if let Ok(Some(def)) = db::get_object_def(&sqlite, &ws_path, &table_name) {
        if !def.select_sql.is_empty() {
            let sql = def.select_sql.trim();
            if sql.to_uppercase().starts_with("CREATE ") || sql.to_uppercase().starts_with("WITH ") {
                return Ok(sql.to_string());
            } else {
                let kind_str = if def.kind == "table" { "TABLE" } else { "VIEW" };
                return Ok(format!("CREATE OR REPLACE {} \"{}\" AS\n{}", kind_str, table_name, sql));
            }
        }
    }

    // 2. Otherwise query DuckDB for its SQL definition (duckdb_views or duckdb_tables)
    let table_name_clone = table_name.clone();
    run_blocking(state, move |conn| {
        // Query duckdb_views first
        if let Ok(sql) = conn.query_row(
            "SELECT sql FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?",
            [&table_name_clone],
            |r| r.get::<_, String>(0),
        ) {
            if !sql.is_empty() {
                return Ok(sql);
            }
        }

        // If not a view, check if it's a base table in duckdb
        let is_table = conn.query_row(
            "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name=?",
            [&table_name_clone],
            |r| r.get::<_, i64>(0),
        ).unwrap_or(0) > 0;

        if is_table {
            let show_sql = format!("SHOW CREATE TABLE \"{}\"", table_name_clone.replace('"', "\"\""));
            if let Ok(ddl) = conn.query_row(&show_sql, [], |r| r.get::<_, String>(0)) {
                return Ok(ddl);
            }
        }

        // If not found in duckdb tables/views either, check if there's a source record we can describe
        if let Ok(Some(rec)) = db::get_source_by_table(&sqlite, &ws_path, &table_name_clone) {
            // For registered files, we can generate a mock SELECT read_* DDL
            if !rec.scan_path.is_empty() {
                let lower = rec.scan_path.to_lowercase();
                let reader = if lower.contains(".parquet") {
                    "read_parquet"
                } else if lower.contains(".csv") || lower.contains(".txt") {
                    "read_csv_auto"
                } else if lower.contains(".json") {
                    "read_json_auto"
                } else {
                    "read_parquet"
                };
                return Ok(format!(
                    "CREATE OR REPLACE VIEW \"{}\" AS\nSELECT * FROM {}('{}');",
                    table_name_clone, reader, rec.scan_path
                ));
            }
        }

        Err(AppError::new(format!("未找到表或视图 \"{}\" 的 DDL 定义", table_name_clone)))
    })
    .await
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut table = [0u8; 256];
    for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for &b in bytes {
        if b == b'=' { break; }
        let val = table[b as usize];
        if val == 0 && b != b'A' { continue; }
        buffer = (buffer << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[tauri::command]
pub async fn save_image_from_base64(base64_data: String, default_name: String) -> Result<(), String> {
    let base64_str = if let Some(pos) = base64_data.find("base64,") {
        &base64_data[pos + 7..]
    } else {
        &base64_data
    };

    let decoded_bytes = decode_base64(base64_str)
        .ok_or_else(|| "Failed to decode base64 data".to_string())?;

    let file_path = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("PNG Image", &["png"])
        .save_file();

    if let Some(path) = file_path {
        std::fs::write(&path, decoded_bytes)
            .map_err(|e| format!("Failed to write file: {}", e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_connection() {
        let _ = db::init_global_db(); // ensure the global schema (incl. options column) is migrated
        let sqlite = db::get_db_conn().unwrap();
        let list = db::list_db_connections(&sqlite).unwrap();
        println!("Connections found: {:?}", list);
        for conn in list {
            println!("Testing connection: {}", conn.name);
            match list_connection_tables_impl(&conn) {
                Ok(tables) => {
                    println!("SUCCESS: found {} tables", tables.len());
                }
                Err(e) => {
                    println!("ERROR testing connection {}: {}", conn.name, e);
                }
            }
        }
    }

    /// Verify the SQLite table-list cache round-trips and replaces correctly
    /// (save → list → clear), without hitting any remote database.
    #[test]
    fn test_db_tables_cache() {
        let _ = db::init_global_db(); // ensure the global schema (incl. options column) is migrated
        let mut sqlite = db::get_db_conn().unwrap();
        let list = db::list_db_connections(&sqlite).unwrap();
        let conn_id = match list.first() {
            Some(c) => c.id.clone(),
            None => {
                println!("[cache] no connections configured, skipping");
                return;
            }
        };

        // Initially empty.
        let initial = db::list_db_connection_tables_cache(&sqlite, &conn_id).unwrap();
        println!("[cache] initial count: {}", initial.len());

        // Save a fake list.
        let items = vec![
            db::CachedDbTable { schema: "public".into(), name: "t1".into(), kind: "table".into() },
            db::CachedDbTable { schema: "public".into(), name: "v1".into(), kind: "view".into() },
        ];
        db::save_db_connection_tables(&mut sqlite, &conn_id, &items).unwrap();
        let read = db::list_db_connection_tables_cache(&sqlite, &conn_id).unwrap();
        assert_eq!(read.len(), 2, "cache should hold saved items");
        println!("[cache] after save: {} items", read.len());

        // Replace with a different list — old entries must be gone.
        let items2 = vec![
            db::CachedDbTable { schema: "public".into(), name: "t2".into(), kind: "table".into() },
        ];
        db::save_db_connection_tables(&mut sqlite, &conn_id, &items2).unwrap();
        let read2 = db::list_db_connection_tables_cache(&sqlite, &conn_id).unwrap();
        assert_eq!(read2.len(), 1, "save should replace, not append");
        assert_eq!(read2[0].name, "t2");

        // Clear.
        db::clear_db_connection_tables_cache(&sqlite, &conn_id).unwrap();
        let after = db::list_db_connection_tables_cache(&sqlite, &conn_id).unwrap();
        assert!(after.is_empty(), "cache should be empty after clear");
        println!("[cache] after clear: {} items", after.len());
    }
}


