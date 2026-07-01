//! Shared driver for the three DDL tools (create_table / create_view /
//! drop_object) plus the identifier helpers they depend on.

use tokio::sync::oneshot;

use super::super::error::ToolError;
use super::super::events::{emit_tool_awaiting, emit_tool_call, emit_tool_result, next_tool_id, now_ms};
use crate::state::AppState;

/// Validate a user-supplied identifier for use in a quoted DuckDB DDL statement.
/// Mirrors `commands::sanitize_ident`: rejects empty names and characters that
/// could break out of a double-quoted identifier.
pub(super) fn sanitize_ddl_ident(name: &str) -> Result<String, ToolError> {
    if name.is_empty() || name.contains('"') || name.contains('\0') {
        return Err(ToolError("非法的表/视图名（不能为空，不能包含双引号）".to_string()));
    }
    Ok(name.to_string())
}

/// Shared state for the three DDL tools.
#[derive(Clone)]
pub(crate) struct DdlToolShared {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
    pub(crate) confirm_mode: String,
}

impl DdlToolShared {
    /// Drive one DDL operation end-to-end respecting the confirm mode.
    ///
    /// `tool_name`/`tool_prefix` identify the tool in the transcript.
    /// `args` is forwarded to the UI via the tool_call segment.
    /// `summary_pending` describes the not-yet-run action for the awaiting UI.
    /// `build_ddl` returns the final statement(s) to execute.
    /// `object_meta` = `Some((name, select_sql, kind))` for create_table/create_view
    /// (enables incremental caching + persists the definition); `None` for drop.
    /// Returns the string fed back to the LLM.
    pub(super) async fn run<F>(
        &self,
        tool_name: &str,
        tool_prefix: &str,
        args: serde_json::Value,
        summary_pending: String,
        object_meta: Option<(String, String, String)>,
        build_ddl: F,
    ) -> Result<String, ToolError>
    where
        F: FnOnce() -> String + Send + 'static,
    {
        let call_id = next_tool_id(tool_prefix);
        emit_tool_call(&self.window, &self.task_id, &call_id, tool_name, args);

        let ddl = build_ddl();

        // Incremental cache: for create_table/create_view, if the upstream
        // fingerprint is unchanged AND the lake object still exists, skip the
        // expensive DROP+CREATE and reuse the existing object.
        if let Some((name, select_sql, _kind)) = &object_meta {
            if self.can_reuse_object(name, select_sql).await {
                let summary = format!("复用已有的{}（输入未变化）", summary_pending);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), Some(ddl), None, None, None,
                );
                return Ok(summary);
            }
        }

        // "变更前确认": park until the user decides. "自动执行" (or any other
        // value) falls through to immediate execution.
        if self.confirm_mode == "变更前确认" {
            let (tx, rx) = oneshot::channel::<crate::state::ConfirmDecision>();
            {
                let key = format!("{}:{}", self.task_id, call_id);
                let mut pending = self.app_state.pending_confirmations.lock().await;
                pending.insert(key.clone(), crate::state::PendingConfirmation { tx });
            }
            // Notify the UI this step is awaiting the user.
            emit_tool_awaiting(&self.window, &self.task_id, &call_id, summary_pending.clone(), ddl.clone());

            let decision = rx.await;
            match decision {
                Ok(d) if d.approved => {
                    // fall through to execute below
                }
                _ => {
                    let msg = "用户已取消此操作".to_string();
                    emit_tool_result(
                        &self.window, &self.task_id, &call_id, "error",
                        msg.clone(), Some(ddl), None, None, None,
                    );
                    return Err(ToolError(msg));
                }
            }
        }

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ddl_for_exec = ddl.clone();
        let exec_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            // DuckDB `execute` runs a single statement; our DDL strings already
            // contain semicolons separating DROP + CREATE, so use execute_batch.
            guard.execute_batch(&ddl_for_exec)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")));

        let elapsed = start.elapsed().as_millis() as u64;
        match exec_res {
            Ok(Ok(())) => {
                // Persist the definition + fingerprint so future builds can skip
                // re-materialization, and so the object can be rebuilt after a
                // lake crash-recovery.
                if let Some((name, select_sql, kind)) = object_meta {
                    self.persist_object_def(&name, &select_sql, &kind).await;
                }
                // drop_object: remove persisted definitions for ALL dropped names
                // (cascade delete may drop multiple objects in one batch).
                if tool_name == "drop_object" {
                    for name in ddl_extract_all_names(&ddl) {
                        self.delete_object_def(&name).await;
                    }
                }
                let summary = format!("{}成功", summary_pending);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), Some(ddl), None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Ok(Err(e)) => {
                let msg = format!("执行失败: {e}");
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), Some(ddl), None, Some(elapsed), None,
                );
                Err(ToolError(msg))
            }
            Err(e) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    e.0.clone(), Some(ddl), None, Some(elapsed), None,
                );
                Err(e)
            }
        }
    }

    /// True iff an `object_defs` record exists for `name`, its current upstream
    /// fingerprint matches the stored one, and the lake object still exists.
    async fn can_reuse_object(&self, name: &str, select_sql: &str) -> bool {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let name = name.to_string();
        let select_sql = select_sql.to_string();
        let conn_clone = self.app_state.conn.clone();
        tokio::task::spawn_blocking(move || -> bool {
            let Ok(sqlite) = crate::db::get_db_conn() else { return false };
            let Ok(Some(def)) = crate::db::get_object_def(&sqlite, &ws_path, &name) else {
                return false;
            };
            let upstreams = crate::fingerprint::extract_upstreams(&select_sql);
            let current_hash =
                crate::fingerprint::compute_input_hash(&sqlite, &ws_path, &select_sql, &upstreams);
            if current_hash != def.input_hash {
                return false;
            }
            // Object still materialized in the lake?
            let guard = conn_clone.blocking_lock();
            crate::commands::table_exists_in_lake(&guard, &name)
        })
        .await
        .unwrap_or(false)
    }

    /// Persist (or refresh) the `object_defs` row for a freshly-built object.
    async fn persist_object_def(&self, name: &str, select_sql: &str, kind: &str) {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let name = name.to_string();
        let select_sql = select_sql.to_string();
        let kind = kind.to_string();
        let created_at = now_ms();
        let conn = self.app_state.conn.clone();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let sqlite = crate::db::get_db_conn()?;
            let upstreams = crate::fingerprint::extract_upstreams(&select_sql);
            let input_hash =
                crate::fingerprint::compute_input_hash(&sqlite, &ws_path, &select_sql, &upstreams);
            // Capture the freshly-built object's columns + row count so the
            // data-tree list can read them from SQLite (no DuckDB query) as long
            // as the upstream fingerprint is unchanged.
            let (columns, row_count) = {
                let guard = conn.blocking_lock();
                let cols = crate::duckdb::schema::describe_view(&guard, &name).unwrap_or_default();
                let escaped = name.replace('"', "\"\"");
                let cnt = guard
                    .query_row(
                        &format!("SELECT count(*) FROM \"{}\"", escaped),
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .ok();
                (cols, cnt)
            };
            let obj = crate::db::ObjectDef {
                table_name: name.clone(),
                kind: kind.clone(),
                select_sql: select_sql.clone(),
                input_hash,
                created_at,
                columns: columns.clone(),
                row_count,
            };
            crate::db::upsert_object_def(&sqlite, &ws_path, &obj)?;
            if kind == "table" {
                let _ = crate::okf::write_table_okf(&ws_dir, &name, &columns, row_count);
            } else {
                let _ = crate::okf::write_view_okf(&ws_dir, &name, &select_sql, &columns);
            }
            Ok(())
        })
        .await;
    }

    /// Remove the `object_defs` and `sources` rows for `name` (called after a
    /// successful drop). Both tables are checked so s_ source tables (in sources)
    /// and t_/v_ views (in object_defs) are both cleaned up.
    async fn delete_object_def(&self, name: &str) {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let name = name.to_string();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let sqlite = crate::db::get_db_conn()?;
            let _ = crate::db::delete_object_def(&sqlite, &ws_path, &name);
            let _ = crate::db::delete_source_by_table(&sqlite, &ws_path, &name);
            let _ = crate::okf::delete_okf_files(&ws_dir, &name);
            Ok(())
        })
        .await;
    }
}

/// Extract ALL quoted object names from a DROP DDL batch (cascade delete may
/// contain multiple `DROP VIEW/TABLE IF EXISTS "name";` statements).
fn ddl_extract_all_names(ddl: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = ddl;
    while let Some(q) = rest.find('"') {
        let after = &rest[q + 1..];
        if let Some(end) = after.find('"') {
            let name = &after[..end];
            if !name.is_empty() {
                names.push(name.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    names
}
