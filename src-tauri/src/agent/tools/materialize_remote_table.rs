use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct MaterializeRemoteTableArgs {
    table_name: String,
}

pub(crate) struct MaterializeRemoteTableTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for MaterializeRemoteTableTool {
    const NAME: &'static str = "materialize_remote_table";
    type Error = ToolError;
    type Args = MaterializeRemoteTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "materialize_remote_table".to_string(),
            description: "将指定的外部数据库表/采样表（如 s_db_cdp_message_sending_notification）完整导入为本地 DuckDB 物理表，以实现全量数据的高速本地分析 and 聚合。此操作在数据表极大时可能会消耗较多的网络 and 存储空间。当需要对一个大表进行多次复杂的 OLAP 聚合分析（如频繁 GROUP BY / ORDER BY）时，你应该自主判断是否建议 or 执行此操作，从而避免跨网络全表扫描带来的极大延迟。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要进行全量本地物化的采样表或外部表名，例如 s_postgres_users" }
                },
                "required": ["table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim();
        if !table_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ToolError("表名包含非法字符，仅允许字母、数字和下划线。".to_string()));
        }

        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let conn = self.app_state.conn.clone();
        let table_name_str = table_name.to_string();

        let call_id = next_tool_id("mat");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "materialize_remote_table",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();

        // 1. Get SourceRecord from SQLite with fuzzy/fallback matching
        let source_record_opt = tokio::task::spawn_blocking(move || -> Result<Option<crate::db::SourceRecord>, String> {
            let sqlite = crate::db::get_db_conn()?;
            // Try exact match first
            if let Ok(Some(rec)) = crate::db::get_source_by_table(&sqlite, &ws_path, &table_name_str) {
                return Ok(Some(rec));
            }
            // Fetch all sources to perform fallback fuzzy matching (e.g. user passes message_sending_notification, we match s_cdp_message_sending_notification)
            let all_sources = crate::db::list_sources(&sqlite, &ws_path)?;
            for src in all_sources {
                if src.table_name.starts_with("s_") && (src.table_name.ends_with(&format!("_{}", table_name_str)) || src.table_name == table_name_str) {
                    return Ok(Some(src));
                }
                if src.table_name == table_name_str || src.scan_path.ends_with(&format!(".{}", table_name_str)) {
                    return Ok(Some(src));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))))?;

        let source_record = match source_record_opt {
            Some(r) => r,
            None => {
                let err_msg = format!("未找到该表 '{}' 的注册元数据。确保它是已挂载的外部表。", table_name);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err_msg.clone(), None, None, Some(start.elapsed().as_millis() as u64), None,
                );
                return Err(ToolError(err_msg));
            }
        };

        let actual_table_name = source_record.table_name.clone();

        // 2. Drop any existing view/table, then bulk-materialize the whole remote
        //    table in a single `CREATE TABLE AS SELECT *`. This replaces the former
        //    row-by-row Appender (which did N+1 `row.get` + two Vec allocations per
        //    row) with DuckDB's optimized bulk importer — large tables materialize
        //    orders of magnitude faster. The trade-off is progress granularity: a
        //    single CTAS exposes no intermediate row count, so we report only
        //    start / finish (the user opted into this).
        let full_path = source_record.scan_path.clone();
        let table_name_clone = actual_table_name.clone();
        let conn_clone = conn.clone();

        emit_tool_result(
            &self.window, &self.task_id, &call_id, "running",
            format!("正在将外部表「{}」全量物化到本地，数据量较大时可能需要一些时间...", table_name),
            None, None, None, None,
        );

        let exec_res = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let guard = conn_clone.blocking_lock();
            let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", table_name_clone), []);
            let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", table_name_clone), []);

            let create_sql = format!("CREATE TABLE \"{}\" AS SELECT * FROM {};", table_name_clone, full_path);
            guard.execute(&create_sql, []).map_err(|e| e.to_string())?;

            // Row count from the freshly materialized local table — DuckDB
            // metadata pushdown makes this cheap even for large tables.
            let n: i64 = guard
                .query_row(&format!("SELECT count(*) FROM \"{}\"", table_name_clone), [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            Ok(n)
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("物化数据失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;

        match exec_res {
            Ok(imported_rows) => {
                // 3. Update metadata in SQLite
                let ws_path = self.app_state.workspace_path.lock().await.clone();
                let table_name_clone = actual_table_name.clone();
                let conn_clone = conn.clone();
                let mut updated_record = source_record;
                updated_record.storage = "table".to_string();
                updated_record.is_sampled = false;
                updated_record.row_count = Some(imported_rows);
                updated_record.full_row_count = Some(imported_rows);

                let update_db_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    // Describe columns under the DuckDB lock, then drop the guard
                    // before touching SQLite so the two stores don't serialize.
                    let new_cols = {
                        let guard = conn_clone.blocking_lock();
                        crate::duckdb::schema::describe_view(&guard, &table_name_clone).ok()
                    };
                    if let Some(cols) = new_cols {
                        updated_record.columns = cols;
                    }
                    let sqlite = crate::db::get_db_conn()?;
                    crate::db::upsert_source(&sqlite, &ws_path, &updated_record)?;
                    Ok(())
                })
                .await
                .map_err(|e| ToolError(format!("线程执行失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("更新元数据失败: {e}"))));

                if let Err(err) = update_db_res {
                    emit_tool_result(
                        &self.window, &self.task_id, &call_id, "error",
                        err.0.clone(), None, None, Some(elapsed), None,
                    );
                    return Err(err);
                }

                let summary = format!("成功将外部表 {} (本地表名: {}) 完整物化到本地 DuckDB，共导入 {} 行数据。", table_name, actual_table_name, imported_rows);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}
