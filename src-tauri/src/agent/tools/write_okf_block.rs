use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct WriteOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
    content: String,
}

pub(crate) struct WriteOkfBlockTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for WriteOkfBlockTool {
    const NAME: &'static str = "write_okf_block";
    type Error = ToolError;
    type Args = WriteOkfBlockArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_okf_block".to_string(),
            description: "向本地 OKF 知识库中某概念文件里的指定二级标题下更新/写入内容（如更新表关联关系、记录业务定义或者添加异常排障记录）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "OKF 目录，例如 tables, views, sources, concepts, pipelines/specific" },
                    "name": { "type": "string", "description": "概念名，例如 t_sales" },
                    "heading": { "type": "string", "description": "二级标题名，例如 关联关系, 探索备注, 异常排障记录" },
                    "content": { "type": "string", "description": "要写入的纯文本或 Markdown 段落" }
                },
                "required": ["category", "name", "heading", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okfw");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "write_okf_block",
            json!({ "category": args.category, "name": args.name, "heading": args.heading, "content": args.content }),
        );
        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let res = crate::okf::write_okf_block(&ws_dir, &args.category, &args.name, &args.heading, &args.content);
        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(_) => {
                let summary = format!("更新板块 {} 成功", args.heading);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary.clone(), None, None, Some(elapsed), Some(args.content.clone()));
                Ok(summary)
            }
            Err(e) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", e.clone(), None, None, Some(elapsed), None);
                Err(ToolError(e))
            }
        }
    }
}
