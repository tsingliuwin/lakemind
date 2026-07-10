use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct LoadTenetsArgs {
    /// A single concept ID (e.g. `industry/education`) to load in full. If
    /// supplied, `tags` is ignored.
    #[serde(default)]
    concept_id: Option<String>,
    /// Tag list to batch-load every matching concept (OR semantics). Ignored
    /// when `concept_id` is set. Use the full namespaced form, e.g.
    /// `["industry:education", "topic:conversion"]`.
    #[serde(default)]
    tags: Option<Vec<String>>,
}

pub(crate) struct LoadTenetsTool {
    #[allow(dead_code)]
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for LoadTenetsTool {
    const NAME: &'static str = "load_tenets";
    type Error = ToolError;
    type Args = LoadTenetsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "load_tenets".to_string(),
            description: "加载全局分析准则库（OKF bundle）内容。三种用法：(1) 传 concept_id 加载单个准则全文（如 industry/education）；(2) 传 tags 按标签批量加载（如 [\"industry:education\", \"topic:conversion\"]，OR 语义）；(3) 都不传 → 返回根目录大纲 index.md，先浏览再精读（渐进式披露，省 token）。准则记录了前人踩过的坑和验证过的方法，动手分析前先查这里。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "concept_id": { "type": "string", "description": "单个准则的 concept ID（如 core/data-discipline、industry/education）。传此参数时忽略 tags。" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "标签数组，批量加载匹配的准则（OR 语义）。用全名空间形式，如 [\"industry:education\", \"topic:conversion\"]。" }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("tnt_l");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "load_tenets",
            json!({ "concept_id": args.concept_id, "tags": args.tags }),
        );
        let start = std::time::Instant::now();

        // (1) Single concept by ID.
        if let Some(cid) = args.concept_id.as_deref() {
            let cid = cid.trim();
            if !cid.is_empty() {
                let res = crate::tenets::load_tenet(cid);
                let elapsed = start.elapsed().as_millis() as u64;
                return match res {
                    Ok(content) => {
                        let summary = format!("已加载准则: {cid}");
                        emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(content.clone()));
                        Ok(content)
                    }
                    Err(e) => {
                        emit_tool_result(&self.window, &self.task_id, &call_id, "error", e.clone(), None, None, Some(elapsed), None);
                        Err(ToolError(e))
                    }
                };
            }
        }

        // (2) Batch by tags.
        if let Some(tags) = args.tags.as_ref().filter(|t| !t.is_empty()) {
            let tags_owned = tags.clone();
            let hits = tokio::task::spawn_blocking(move || crate::tenets::load_tenets_by_tags(&tags_owned))
                .await
                .map_err(|e| ToolError(format!("线程池故障: {}", e)))?;
            let elapsed = start.elapsed().as_millis() as u64;
            if hits.is_empty() {
                let msg = format!("没有匹配标签 [{}] 的准则。", tags.join(", "));
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", msg.clone(), None, None, Some(elapsed), None);
                return Ok(msg);
            }
            // Load full bodies for each hit.
            let mut bodies = Vec::new();
            for h in &hits {
                if let Ok(content) = crate::tenets::load_tenet(&h.concept_id) {
                    bodies.push(format!("===== {} ({}) =====\n{}", h.concept_id, h.title, content));
                }
            }
            let summary = format!("按标签加载 {} 条准则", hits.len());
            let out = bodies.join("\n\n");
            emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(out.clone()));
            return Ok(out);
        }

        // (3) Default: progressive-disclosure index.
        let res = crate::tenets::load_tenets_index();
        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(content) => {
                let summary = "已加载准则库目录大纲（index.md）".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", summary, None, None, Some(elapsed), Some(content.clone()));
                Ok(content)
            }
            Err(e) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error", e.clone(), None, None, Some(elapsed), None);
                Err(ToolError(e))
            }
        }
    }
}
