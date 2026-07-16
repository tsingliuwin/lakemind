use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

/// No-args marker (rig requires an Args type even for parameter-less tools).
#[derive(Deserialize, Serialize)]
pub(crate) struct GetWorkspaceDialectsArgs {}

pub(crate) struct GetWorkspaceDialectsTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for GetWorkspaceDialectsTool {
    const NAME: &'static str = "get_workspace_dialects";
    type Error = ToolError;
    type Args = GetWorkspaceDialectsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "get_workspace_dialects".to_string(),
            description: "获取当前工作区接入的数据库类型及其方言使用要点（如 MaxCompute 表名需用 `project.table` 全限定名、`ORDER BY` 必须带 `LIMIT` 等）。**对话开始时，或首次需要对外部数据库表写 SQL 前，先调用本工具**对齐工作区数据库方言，写 SQL 时遵循返回的规则以避免反复试错报错。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("dialects");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "get_workspace_dialects",
            json!({}),
        );
        let start = std::time::Instant::now();

        let ws_path = self.app_state.workspace_path.lock().await.clone();
        // `active_dialect_block` touches SQLite (list_workspace_connections +
        // list_sources), so run it on the blocking pool - same pattern as
        // `maxcompute_pushdown_query`.
        let body = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            Ok(crate::db_dialects::active_dialect_block(&ws_path))
        })
        .await
        .map_err(|e| format!("查询工作区数据库方言失败: {e}"))
        .and_then(|r| r)
        .map_err(ToolError)?;
        let out = body.unwrap_or_else(|| {
            "当前工作区未接入需要特殊方言处理的数据库类型（如 MaxCompute），可按标准 SQL 分析。"
                .to_string()
        });
        let elapsed = start.elapsed().as_millis() as u64;
        emit_tool_result(
            &self.window, &self.task_id, &call_id, "ok",
            out.clone(), None, None, Some(elapsed), None,
        );
        Ok(out)
    }
}
