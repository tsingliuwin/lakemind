use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::config::get_query_hard_timeout;
use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::sample_guard::check_sampled_aggregation;
use crate::duckdb::execute;
use crate::model::SqlResult;
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct ExecuteQueryArgs {
    sql: String,
}

pub(crate) struct ExecuteQueryTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for ExecuteQueryTool {
    const NAME: &'static str = "execute_query";
    type Error = ToolError;
    type Args = ExecuteQueryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "execute_query".to_string(),
            description: "执行只读的 SQL 查询，并返回结果。只允许 SELECT，禁止 DROP/ALTER/UPDATE/DELETE/INSERT 等。注意：对本地采样缓存表（s_ 开头且为采样态）执行聚合（SUM/COUNT/AVG/GROUP BY 等）会被拦截——采样表仅含部分行，聚合会失真；全量聚合请改用 postgres_query/mysql_query 下推，或先 materialize_remote_table 落盘。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "要执行的 SQL 查询语句" }
                },
                "required": ["sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let sql = args.sql.trim();
        let sql_upper = sql.to_uppercase();
        let forbidden_keywords = ["DROP", "DELETE", "UPDATE", "INSERT", "ALTER", "TRUNCATE", "ATTACH", "DETACH"];
        for keyword in &forbidden_keywords {
            if sql_upper.contains(keyword) {
                return Err(ToolError(format!("出于安全考虑，禁止执行包含 {} 操作的 SQL 语句。", keyword)));
            }
        }

        let call_id = next_tool_id("exec");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "execute_query",
            json!({ "sql": sql }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let sql_string = sql.to_string();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, String> {
            // Guard: refuse aggregations over local *sampled* cache tables. A
            // sampled table holds only a tiny subset (e.g. 1000 rows) of a remote
            // table that may have millions of rows; aggregating it silently
            // produces badly wrong metrics. Force the agent onto a full-data path
            // (native pushdown or materialize_remote_table) instead.
            check_sampled_aggregation(&sql_string, &ws_path)?;
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50)).map_err(|e| format!("SQL 执行出错: {e}"))
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(ToolError)),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("SQL 执行已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(ToolError))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match query_res {
            Ok(res) => {
                let n = res.rows.len();
                let summary = format!("查询成功，返回 {} 行（{} 列）", n, res.columns.len());
                // Compact string for the LLM context (avoid flooding it with 50 rows);
                // the full structured SqlResult goes only to the UI via the segment.
                let mut out = String::new();
                out.push_str(&format!("查询成功，返回 {} 行。列: {}\n", n, res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                if res.truncated {
                    out.push_str("(结果已截断，仅返回前 50 行)\n");
                }
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, Some(sql.to_string()), Some(res), Some(elapsed), None,
                );
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}
