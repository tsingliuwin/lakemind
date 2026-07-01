use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::config::get_query_hard_timeout;
use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::duckdb::execute;
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct SampleDataArgs {
    table_name: String,
}

pub(crate) struct SampleDataTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for SampleDataTool {
    const NAME: &'static str = "sample_data";
    type Error = ToolError;
    type Args = SampleDataArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "sample_data".to_string(),
            description: "获取指定数据表或视图的前 5 行样例数据。用于直观了解数据的具体内容 and 字段格式。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要采样查询的表名或视图名" }
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

        let call_id = next_tool_id("sample");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "sample_data",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_string = table_name.to_string();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("SELECT * FROM {table_name_string} LIMIT 5");
            execute::run_query(&guard, &sql, Some(5))
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}")))),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("采样查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}"))))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match query_res {
            Ok(res) => {
                let n = res.rows.len();
                let summary = format!("完成采样，获取到 {} 行样例数据", n);
                let mut out = String::new();
                out.push_str(&format!("表 {} 的前 {} 行样例数据如下. 列: {}\n", table_name, n, res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, Some(res), Some(elapsed), None,
                );
                Ok(out)
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
