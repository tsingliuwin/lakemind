use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::config::get_query_hard_timeout;
use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::duckdb::execute;
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct DescribeTableArgs {
    table_name: String,
}

pub(crate) struct DescribeTableTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for DescribeTableTool {
    const NAME: &'static str = "describe_table";
    type Error = ToolError;
    type Args = DescribeTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "describe_table".to_string(),
            description: "获取指定数据表或视图的结构信息（列名、数据类型等）。在对表编写 SQL 前，必须调用此工具了解其结构。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要查询结构的表名或视图名" }
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

        let call_id = next_tool_id("desc");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "describe_table",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_string = table_name.to_string();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("DESCRIBE \"{}\"", table_name_string.replace('"', "\"\""));
            let query_res = execute::run_query(&guard, &sql, None).map_err(|e| e.to_string())?;
            let (okf_title, col_comments, relations) = crate::okf::parse_column_semantics(&ws_dir, &table_name_string);

            // Read SQLite cache for details
            let sqlite = crate::db::get_db_conn().map_err(|e| e.to_string())?;
            let source_record = crate::db::get_source_by_table(&sqlite, &ws_path, &table_name_string).map_err(|e| e.to_string())?;

            Ok::<_, String>((query_res, okf_title, col_comments, relations, source_record))
        });
        let desc_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}")))),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}"))))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match desc_res {
            Ok((res, okf_title, col_comments, relations, source_record)) => {
                let col_lines: Vec<String> = res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v| v.to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let null = r.get(2).map(|v| v.to_string()).unwrap_or_default();
                    let comment = col_comments.get(&name).map(|c| format!(", 释义: {}", c)).unwrap_or_default();
                    format!("{} (类型: {}, 允许空: {}){}", name, ty, null, comment)
                }).collect();
                let n = res.rows.len();
                let mut title_part = String::new();
                if let Some(t) = okf_title {
                    title_part = format!(" (业务名称: {})", t);
                }
                let mut rels_part = String::new();
                if !relations.is_empty() {
                    rels_part = format!("\n\n关联关系:\n{}", relations.iter().map(|r| format!("- {}", r)).collect::<Vec<_>>().join("\n"));
                }

                let mut header_info = String::new();
                if let Some(ref rec) = source_record {
                    let is_partial = matches!(rec.materialize_status.as_deref(), Some("partial"));
                    if is_partial {
                        let full_rows_str = rec.full_row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        let db_alias = rec.scan_path.split('.').next().unwrap_or("db_conn");
                        header_info = format!(
                            " [注意: 该表当前仅部分物化（已落盘 {} 行 / 远程全量约 {} 行），数据不完整，聚合会失真。如需全量汇总，请再次调用 materialize_remote_table 续传至完成（自动跳过已物化部分），或用原生下推 \"SELECT * FROM {}_query('{}', '...')\"]",
                            rec.row_count.unwrap_or(0),
                            full_rows_str,
                            rec.kind,
                            db_alias
                        );
                    } else if rec.aggregation_misleads() {
                        let full_rows_str = rec.full_row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        let db_alias = rec.scan_path.split('.').next().unwrap_or("db_conn");
                        header_info = format!(
                            " [注意: 该表当前是本地物化采样缓存，包含 {} 行数据，连接数据库类型为 \"{}\"。外部生产数据库的全量总行数大约为 {} 行。如果你需要进行全量汇总或分析完整数据，请优先使用原生下推函数，直接在 SQL 中查询 \"SELECT * FROM {}_query('{}', '...')\"]",
                            rec.row_count.unwrap_or(0),
                            rec.kind,
                            full_rows_str,
                            rec.kind,
                            db_alias
                        );
                    } else {
                        let rows_str = rec.row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        header_info = format!(" [全量数据表，连接数据库类型为 \"{}\"，行数: {}]", rec.kind, rows_str);
                    }
                }

                let summary = format!("结构分析完成，{}{}{}, 共 {} 个字段", table_name, title_part, header_info, n);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, Some(res), Some(elapsed), None,
                );
                Ok(format!("表 {}{} 的列结构如下:\n{}{}", table_name, title_part, col_lines.join("\n"), rels_part))
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
