use std::fs;
use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use super::super::llm::complete_one_shot;
use super::super::okf_io::{backup_and_rewrite_okf, parse_okf_files};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct TidyOkfKnowledgeArgs {}

pub(crate) struct TidyOkfKnowledgeTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
    pub(crate) model_id: String,
    pub(crate) provider_id: Option<String>,
}

impl Tool for TidyOkfKnowledgeTool {
    const NAME: &'static str = "tidy_okf_knowledge";
    type Error = ToolError;
    type Args = TidyOkfKnowledgeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "tidy_okf_knowledge".to_string(),
            description: "对本地 OKF 知识库下的所有 Markdown 文件执行全局自动整理、重构、归类提炼、去重和移动（例如：将多张表/视图的业务描述中冗余/重复的公司介绍或通用概念剥离出来，统一移动并合并到 concepts/company.md 等全局业务概念文件中，以保持整个知识库的精简与清晰）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okft");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "tidy_okf_knowledge",
            json!({}),
        );
        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let model_id_clone = self.model_id.clone();
        let provider_id_clone = self.provider_id.clone();

        let result_str = tokio::task::spawn_blocking(move || {
            let okf_dir = crate::okf::get_okf_dir(&ws_dir);
            if !okf_dir.exists() {
                return Ok("本地 OKF 知识库尚不存在，无需进行整理。".to_string());
            }

            // 1. Gather all md files and build a dump
            let mut dump_text = String::new();
            let mut file_count = 0;

            for entry in walkdir::WalkDir::new(&okf_dir) {
                let Ok(entry) = entry else { continue };
                if entry.path().is_file() && entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        let rel_path = entry.path()
                            .strip_prefix(&okf_dir)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| entry.path().to_string_lossy().to_string());

                        dump_text.push_str(&format!("## FILE: {}\n```markdown\n{}\n```\n\n", rel_path, content));
                        file_count += 1;
                    }
                }
            }

            if file_count == 0 {
                return Ok("本地 OKF 知识库尚无任何知识文件，无需进行整理。".to_string());
            }

            // 2. Prepare the prompt for LLM call
            let cleanup_prompt = format!(
                "# 角色\n\
                 你是一个专职的 OKF 知识库自动重构与精炼专家。你的任务是分析当前本地 OKF 知识库中的所有文件，并根据知识类型重新归并、去重、移动和结构化提炼。\n\n\
                 # 知识库分类规则\n\
                 - `concepts/`：存放全局业务概念、通用业务描述、名词解释、公司背景、通用名词简称或通用计算规则（例如：concepts/company.md 用于公司背景，concepts/terms.md 用于通用名词解释）。任何跨表共享或不局限于单张表/视图的知识，必须整理到此处，并从原来的表/视图中彻底剥离删除。\n\
                 - `tables/` / `views/` / `sources/`：存放单张物理表、视图或物理数据源特有的私有信息（例如列的物理类型说明、仅限该表/视图的私有字段释义、或它们之间的特定关联关系），不能写全局通用背景。\n\
                 - `pipelines/specific/`：存放特定的物理数据导入/清洗配方或特定报错的异常排障记录。\n\n\
                 # 重构要求\n\
                 1. 归一化与去重：如果发现不同表/视图的描述里写了相似的通用业务背景或名词解释，请精炼到 concepts 对应的文件里，在原表/视图中删除以避免冗余。\n\
                 2. 正确移动：把误放在 tables/views 里的公司整体介绍、业务体系描述等全局知识彻底移入 concepts 目录下。\n\
                 3. 整理与理顺：整理每个文件，确保其具备正确的 YAML 头部（带有 title 和 type，且 type 符合规范：duckdb table, duckdb view, business concept, data source 等）和清晰的二级标题架构（如 ## 业务描述、## 关联关系、## 字段描述）。\n\n\
                 # 输出要求\n\
                 你必须且仅输出整理后所有仍然有效的文件的最新内容，用严格的 ```okf-file 包裹。请勿输出任何其他多余文本、致谢或解释说明。如果有某个文件因为内容整个被移空而需要删除，请不要输出该文件，系统会自动从磁盘删除它。\n\n\
                 格式示例：\n\
                 ```okf-file\n\
                 FILE: concepts/company.md\n\
                 ---\n\
                 title: 公司背景\n\
                 type: Business Concept\n\
                 ---\n\
                 # 公司背景\n\n\
                 ## 业务描述\n\
                 公司名为苏州研途教育...\n\
                 ```\n\n\
                 ```okf-file\n\
                 FILE: views/v_jingying_zonglan_2026.md\n\
                 ---\n\
                 title: 经营总览视图\n\
                 type: DuckDB View\n\
                 ---\n\
                 # 经营总览视图\n\n\
                 ## 业务描述\n\
                 本视图展示 2026 年度...\n\
                 ```\n\n\
                 # 当前待整理的文件列表\n\n\
                 {}",
                dump_text
            );

            // 3. Call complete_one_shot asynchronously using block_on inside spawn_blocking
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("构建 Tokio 运行时失败: {}", e))?;

            let llm_res = rt.block_on(async {
                complete_one_shot(&cleanup_prompt, &model_id_clone, provider_id_clone.as_deref()).await
            }).map_err(|e| format!("调用大模型整理失败: {}", e))?;

            // 4. Parse the okf-file blocks
            let new_files = parse_okf_files(&llm_res);
            if new_files.is_empty() {
                return Err("大模型返回结果未包含任何有效的 okf-file 块，放弃整理操作以防数据丢失。".to_string());
            }

            // 5. Rewrite OKF directory with safety backup/rollback
            backup_and_rewrite_okf(&okf_dir, &new_files)
        })
        .await
        .map_err(|e| ToolError(format!("线程池执行异常: {}", e)))?
        .map_err(ToolError)?;

        let elapsed = start.elapsed().as_millis() as u64;
        emit_tool_result(&self.window, &self.task_id, &call_id, "ok", result_str.clone(), None, None, Some(elapsed), Some(result_str.clone()));
        Ok(result_str)
    }
}
