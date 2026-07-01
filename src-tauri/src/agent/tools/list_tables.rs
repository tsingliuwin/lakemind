use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct ListTablesArgs {}

pub(crate) struct ListTablesTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for ListTablesTool {
    const NAME: &'static str = "list_tables";
    type Error = ToolError;
    type Args = ListTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_tables".to_string(),
            description: "列出当前数据库中的所有数据表和视图名。开始探索前应先调用此工具了解有哪些数据。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("list");
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_tables", json!({}));

        let start = std::time::Instant::now();
        let sql = "
            SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
            UNION
            SELECT view_name as table_name FROM duckdb_views() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
        ";
        let conn = self.app_state.conn.clone();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let tables_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let mut stmt = guard.prepare(sql).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(|e| e.to_string())?);
            }

            // Read sampling metadata from SQLite
            let sqlite = crate::db::get_db_conn().map_err(|e| e.to_string())?;
            let records = crate::db::list_sources(&sqlite, &ws_path).map_err(|e| e.to_string())?;

            let mut descriptions = Vec::new();
            for tname in list {
                if let Some(rec) = records.iter().find(|r| r.table_name == tname) {
                    if rec.is_sampled {
                        let full_rows_str = rec.full_row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        descriptions.push(format!(
                            "{} (已本地物化采样 {} 行，外部全量大约 {} 行，类型是 \"{}\"，全量直连路径为 \"{}\")",
                            tname,
                            rec.row_count.unwrap_or(0),
                            full_rows_str,
                            rec.kind,
                            rec.scan_path
                        ));
                    } else {
                        descriptions.push(format!(
                            "{} (全量，类型是 \"{}\"，行数: {})",
                            tname,
                            rec.kind,
                            rec.row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string())
                        ));
                    }
                } else {
                    descriptions.push(tname);
                }
            }

            Ok::<_, String>(descriptions)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let summary = if tables.is_empty() {
                    "数据库中目前没有找到任何表。".to_string()
                } else {
                    format!("探测到 {} 张表: {}", tables.len(), tables.join(", "))
                };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, None, Some(elapsed), None,
                );
                Ok(format!("当前可用的数据库表列表为: {}", tables.join("; ")))
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
