use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

#[derive(Deserialize, Serialize)]
pub(crate) struct CheckSourceFingerprintArgs {
    file_path: String,
}

pub(crate) struct CheckSourceFingerprintTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for CheckSourceFingerprintTool {
    const NAME: &'static str = "check_source_fingerprint";
    type Error = ToolError;
    type Args = CheckSourceFingerprintArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "check_source_fingerprint".to_string(),
            description: "计算物理文件的指纹（mtime + size）并检索 OKF 知识库中是否已有对应的注册源文件。如果有，返回其 table_name，可以直接重用而不需要重新探索表结构。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "文件的绝对路径，例如 /workspace/data/sales.csv" }
                },
                "required": ["file_path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = next_tool_id("okff");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "check_source_fingerprint",
            json!({ "file_path": args.file_path }),
        );
        let start = std::time::Instant::now();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let file_path_str = args.file_path.clone();

        let match_res = tokio::task::spawn_blocking(move || {
            let path = Path::new(&file_path_str);
            if !path.exists() {
                return Err(format!("文件不存在: {}", file_path_str));
            }
            let meta = fs::metadata(path).map_err(|e| format!("读取元数据失败: {}", e))?;
            let size = meta.len() as i64;
            let mtime = match meta.modified() {
                Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_millis() as i64,
                    Err(_) => 0,
                },
                Err(_) => 0,
            };

            let target_fp = format!("{}:{}", mtime, size);

            // Search in OKF/sources
            let okf_dir = crate::okf::get_okf_dir(&ws_dir);
            let sources_dir = okf_dir.join("sources");
            if sources_dir.exists() {
                for entry in walkdir::WalkDir::new(&sources_dir) {
                    let Ok(entry) = entry else { continue };
                    if entry.path().is_file() && entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            let content_str: String = content;
                            let mut fp_line = String::new();
                            for line in content_str.lines() {
                                if line.starts_with("fingerprint:") {
                                    fp_line = line.trim_start_matches("fingerprint:").trim().to_string();
                                    break;
                                }
                            }
                            if fp_line == target_fp {
                                let name = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                                return Ok(Some(name));
                            }
                        }
                    }
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ToolError(format!("线程池故障: {}", e)))?
        .map_err(ToolError)?;

        let elapsed = start.elapsed().as_millis() as u64;
        match match_res {
            Some(name) => {
                let msg = format!("找到完全匹配的文件指纹！已有注册表名：`{}`。可直接通过查询操作它，跳过重新探索。", name);
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", msg.clone(), None, None, Some(elapsed), Some(name.clone()));
                Ok(msg)
            }
            None => {
                let msg = "未找到匹配的数据指纹，这是一个全新的数据源文件，请按常规方式导入并探索。".to_string();
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok", msg.clone(), None, None, Some(elapsed), None);
                Ok(msg)
            }
        }
    }
}
