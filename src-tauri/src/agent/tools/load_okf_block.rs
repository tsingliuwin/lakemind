use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct LoadOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
}

pub(crate) struct LoadOkfBlockTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for LoadOkfBlockTool {
    const NAME: &'static str = "load_okf_block";
    type Error = ToolError;
    type Args = LoadOkfBlockArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "load_okf_block".to_string(),
            description: "读取本地 OKF 知识库中某概念文件下的特定二级标题板块（如：读取 tables 分类下订单表的 '关联关系' 或是 concepts 分类下的 '公司背景'，或是 pipelines 分类下的 '异常排障记录'）。这样可以避免加载全文，节省 token。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "OKF 的目录类别，例如 tables, views, sources, concepts, pipelines/specific" },
                    "name": { "type": "string", "description": "文件概念名，不带 .md 扩展名，例如 t_sales" },
                    "heading": { "type": "string", "description": "要读取的二级标题，例如 关联关系, 物理画像, 异常排障记录" }
                },
                "required": ["category", "name", "heading"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okfr");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "load_okf_block",
            json!({ "category": args.category, "name": args.name, "heading": args.heading }),
        );
        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let res = crate::okf::read_okf_block(&ws_dir, &args.category, &args.name, &args.heading);
        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(content) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", format!("读取板块 {} 成功", args.heading), None, None, Some(elapsed), Some(content.clone()));
                Ok(content)
            }
            Err(e) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", e.clone(), None, None, Some(elapsed), None);
                Err(ToolError(e))
            }
        }
    }
}
