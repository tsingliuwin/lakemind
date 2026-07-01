use std::fs;
use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct SearchOkfRecipesArgs {
    query: String,
}

pub(crate) struct SearchOkfRecipesTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for SearchOkfRecipesTool {
    const NAME: &'static str = "search_okf_recipes";
    type Error = ToolError;
    type Args = SearchOkfRecipesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_okf_recipes".to_string(),
            description: "在本地 OKF 知识库中（特别是 pipelines 目录下）搜索匹配的导入/清洗配方或排障经历。当面临导入报错或数据异常时，可以用此工具查找历史上相似的解决方案。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "要检索的关键字或错误信息，如 'UTF-16', 'date format', 'delimiter'" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okfs");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "search_okf_recipes",
            json!({ "query": args.query }),
        );
        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();

        let search_res = tokio::task::spawn_blocking(move || {
            let okf_dir = crate::okf::get_okf_dir(&ws_dir);
            let mut matches = Vec::new();
            let query_lower = args.query.to_lowercase();

            // Walk only okf/pipelines
            let pipelines_dir = okf_dir.join("pipelines");
            if pipelines_dir.exists() {
                for entry in walkdir::WalkDir::new(&pipelines_dir) {
                    let Ok(entry) = entry else { continue };
                    if entry.path().is_file() && entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            let content_str: String = content.to_lowercase();
                            if content_str.contains(query_lower.as_str()) {
                                let rel_path = entry.path()
                                    .strip_prefix(&okf_dir)
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|_| entry.path().to_string_lossy().to_string());
                                // Just grab first 200 chars as preview
                                let preview: String = content.lines().take(6).collect::<Vec<_>>().join("\n");
                                matches.push(format!("文件: {}\n预览:\n{}\n---", rel_path, preview));
                            }
                        }
                    }
                }
            }
            matches
        })
        .await
        .map_err(|e| ToolError(format!("线程池故障: {}", e)))?;

        let elapsed = start.elapsed().as_millis() as u64;
        if search_res.is_empty() {
            let msg = "未找到匹配的配方或故障记录。".to_string();
            emit_tool_result(&self.window, &self.task_id, &call_id, "ok", msg.clone(), None, None, Some(elapsed), None);
            Ok(msg)
        } else {
            emit_tool_result(&self.window, &self.task_id, &call_id, "ok", format!("检索出 {} 条记录", search_res.len()), None, None, Some(elapsed), Some(search_res.join("\n\n")));
            Ok(format!("检索出以下相关经验配方:\n\n{}", search_res.join("\n\n")))
        }
    }
}
