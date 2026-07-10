use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct SearchTenetsArgs {
    query: String,
}

pub(crate) struct SearchTenetsTool {
    #[allow(dead_code)]
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for SearchTenetsTool {
    const NAME: &'static str = "search_tenets";
    type Error = ToolError;
    type Args = SearchTenetsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_tenets".to_string(),
            description: "检索全局分析准则库（OKF bundle，随软件分发、所有工作区共享）。库中沉淀了跨行业分析方法论、各行业（教育/旅游/房地产…）典型案例、分析主题（转化/增长…）陷阱。用关键词检索标题/描述/标签/正文，返回 concept ID + 预览；需要全文时用 load_tenets 按 concept_id 加载。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "检索关键词，如「归因」「体验课」「转化漏斗」「截断」" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("tnt_s");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "search_tenets",
            json!({ "query": args.query }),
        );
        let start = std::time::Instant::now();

        let hits = tokio::task::spawn_blocking(move || crate::tenets::search_tenets(&args.query))
            .await
            .map_err(|e| ToolError(format!("线程池故障: {}", e)))?;

        let elapsed = start.elapsed().as_millis() as u64;
        if hits.is_empty() {
            let msg = "未找到匹配的准则或案例。".to_string();
            emit_tool_result(&self.window, &self.task_id, &call_id, "ok", msg.clone(), None, None, Some(elapsed), None);
            Ok(msg)
        } else {
            let body: Vec<String> = hits
                .iter()
                .map(|h| {
                    let tags = if h.tags.is_empty() {
                        String::new()
                    } else {
                        format!("  标签: {}\n", h.tags.join(", "))
                    };
                    format!(
                        "concept_id: {}\n  标题: {}\n{}  描述: {}\n  预览:\n{}\n---",
                        h.concept_id,
                        h.title,
                        tags,
                        h.description,
                        h.preview
                    )
                })
                .collect();
            let summary = format!("检索出 {} 条准则/案例", hits.len());
            let out = format!("检索出以下相关准则/案例（需要全文请用 load_tenets + concept_id 加载）:\n\n{}", body.join("\n\n"));
            emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(out.clone()));
            Ok(out)
        }
    }
}
