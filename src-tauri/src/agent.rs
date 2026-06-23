use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{
    completion::Message,
    streaming::{StreamingChat, StreamedAssistantContent},
    tool::Tool,
    completion::ToolDefinition,
    client::CompletionClient,
    agent::{MultiTurnStreamItem, StreamingError},
};
use tauri::Emitter;
use crate::state::AppState;
use crate::duckdb::execute;
use crate::model::SqlResult;

// Define ToolError
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError(pub String);
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ToolError {}

// ===========================================================================
// Wire protocol: ordered segment transcript streamed to the frontend.
//
// An assistant message is a list of `Segment`s in arrival order:
//   reasoning → tool → reasoning → tool → text (final answer)
// Each tool is one `Segment::Tool` whose status transitions running → ok|error
// when the matching tool_result event arrives (updated in place by id).
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Segment {
    /// Model thinking. Accumulated from reasoning deltas.
    Reasoning {
        id: String,
        text: String,
    },
    /// One tool call + its result merged into a single logical step.
    /// `status` goes running → ok|error when the tool_result event arrives.
    Tool {
        id: String,
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        status: String, // "running" | "ok" | "error"
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sql: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        table: Option<SqlResult>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    /// Visible answer text (Markdown). Accumulated from text deltas.
    Text {
        id: String,
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStreamEvent {
    pub task_id: String,
    // "reasoning" | "text" | "tool_call" | "tool_result" | "done" | "error"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment: Option<Segment>,
}

/// Minimal view of a stored message for rebuilding the LLM history.
/// Both `content` (legacy) and `segments` (new) are optional + default so the
/// DTO tolerates either persisted shape.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatMessageDto {
    pub role: String, // "user" | "assistant"
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub segments: Option<Vec<Segment>>,
    pub ts: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSettings {
    providers: Vec<ModelProvider>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelItem {
    id: String,
    context_window: usize,
    max_tokens: Option<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelProvider {
    id: String,
    name: String,
    endpoint: String,
    api_key: String,
    api_format: String, // "openai" | "anthropic" | "responses"
    models: Vec<ModelItem>,
    enabled: bool,
}

fn get_provider_for_model(model_id: &str) -> Result<ModelProvider, String> {
    let mut path = crate::db::get_lakemind_dir()?;
    path.push("settings.json");
    if !path.exists() {
        return Err("配置文件 settings.json 不存在，请先在设置中配置模型。".to_string());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("读取配置文件失败: {e}"))?;
    let settings: SavedSettings = serde_json::from_str(&content)
        .map_err(|e| format!("解析配置文件失败: {e}"))?;

    for provider in settings.providers {
        if provider.enabled {
            if provider.models.iter().any(|m| m.id == model_id) {
                return Ok(provider);
            }
        }
    }

    Err(format!("未找到包含模型「{}」且已启用的服务商，请检查设置。", model_id))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn emit_event(window: &tauri::Window, task_id: &str, kind: &str, text: Option<String>, segment: Option<Segment>) {
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            text,
            segment,
        },
    );
}

/// Emit a partial reasoning/text delta to be appended to the current segment of
/// that type on the frontend.
fn emit_delta(window: &tauri::Window, task_id: &str, kind: &str, text: &str) {
    emit_event(window, task_id, kind, Some(text.to_string()), None);
}

/// Emit a `tool_call` segment (status: running) — opens a new tool step in the
/// transcript.
fn emit_tool_call(window: &tauri::Window, task_id: &str, id: &str, tool: &str, args: serde_json::Value) {
    emit_event(window, task_id, "tool_call", None, Some(Segment::Tool {
        id: id.to_string(),
        tool: tool.to_string(),
        args: Some(args),
        status: "running".to_string(),
        summary: None,
        sql: None,
        table: None,
        elapsed_ms: None,
    }));
}

/// Emit a `tool_result` — merged into the matching tool segment by id, flipping
/// its status to ok|error and attaching the result payload.
fn emit_tool_result(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    status: &str, // "ok" | "error"
    summary: String,
    sql: Option<String>,
    table: Option<SqlResult>,
    elapsed_ms: Option<u64>,
) {
    emit_event(window, task_id, "tool_result", None, Some(Segment::Tool {
        id: id.to_string(),
        tool: String::new(), // frontend merges by id; tool name already set
        args: None,
        status: status.to_string(),
        summary: Some(summary),
        sql,
        table,
        elapsed_ms,
    }));
}

// ===========================================================================
// Rig Tools Implementation
// ===========================================================================

#[derive(Deserialize, Serialize)]
struct ListTablesArgs {}

struct ListTablesTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for ListTablesTool {
    const NAME: &'static str = "list_tables";
    type Error = ToolError;
    type Args = ListTablesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_tables".to_string(),
            description: "列出当前数据库中的所有数据表和视图名。开始探索前应先调用此工具了解有哪些数据。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let call_id = format!("tool-list-{}", now_ms());
        emit_tool_call(&self.window, &self.task_id, &call_id, "list_tables", json!({}));

        let start = std::time::Instant::now();
        let sql = "
            SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
            UNION
            SELECT view_name as table_name FROM duckdb_views() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
        ";
        let conn = self.app_state.conn.clone();
        let tables_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let mut stmt = guard.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r?);
            }
            Ok::<_, duckdb::Error>(list)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match tables_res {
            Ok(tables) => {
                let summary = if tables.is_empty() {
                    "数据库中目前没有找到任何表。".to_string()
                } else {
                    format!("探测到 {} 张表: {}", tables.len(), tables.join(", "))
                };
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, None, Some(elapsed),
                );
                Ok(format!("当前可用的数据库表列表为: {}", tables.join(", ")))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed),
                );
                Err(err)
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct DescribeTableArgs {
    table_name: String,
}

struct DescribeTableTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for DescribeTableTool {
    const NAME: &'static str = "describe_table";
    type Error = ToolError;
    type Args = DescribeTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "describe_table".to_string(),
            description: "获取指定数据表或视图的结构信息（列名、数据类型等）。在对表编写 SQL 前，必须调用此工具了解其结构。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要查询结构的表名或视图名" }
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

        let call_id = format!("tool-desc-{}", now_ms());
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "describe_table",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let table_name_string = table_name.to_string();
        let desc_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("DESCRIBE {table_name_string}");
            execute::run_query(&guard, &sql, None)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;
        match desc_res {
            Ok(res) => {
                let col_lines: Vec<String> = res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v| v.to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let null = r.get(2).map(|v| v.to_string()).unwrap_or_default();
                    format!("{} (类型: {}, 允许空: {})", name, ty, null)
                }).collect();
                let n = res.rows.len();
                let summary = format!("结构分析完成，{} 共 {} 个字段", table_name, n);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, Some(res), Some(elapsed),
                );
                Ok(format!("表 {} 的列结构如下:\n{}", table_name, col_lines.join("\n")))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed),
                );
                Err(err)
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct ExecuteQueryArgs {
    sql: String,
}

struct ExecuteQueryTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for ExecuteQueryTool {
    const NAME: &'static str = "execute_query";
    type Error = ToolError;
    type Args = ExecuteQueryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "execute_query".to_string(),
            description: "执行只读的 SQL 查询，并返回结果。所有查询语句必须只能包含 SELECT，禁止执行 DROP, ALTER, UPDATE, DELETE, INSERT 等修改数据库的操作。".to_string(),
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

        let call_id = format!("tool-exec-{}", now_ms());
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "execute_query",
            json!({ "sql": sql }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let sql_string = sql.to_string();
        let query_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50))
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("SQL 执行出错: {e}"))));

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
                    summary, Some(sql.to_string()), Some(res), Some(elapsed),
                );
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed),
                );
                Err(err)
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct SampleDataArgs {
    table_name: String,
}

struct SampleDataTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for SampleDataTool {
    const NAME: &'static str = "sample_data";
    type Error = ToolError;
    type Args = SampleDataArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "sample_data".to_string(),
            description: "获取指定数据表或视图的前 5 行样例数据。用于直观了解数据的具体内容和字段格式。".to_string(),
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

        let call_id = format!("tool-sample-{}", now_ms());
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "sample_data",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let table_name_string = table_name.to_string();
        let query_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("SELECT * FROM {table_name_string} LIMIT 5");
            execute::run_query(&guard, &sql, Some(5))
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}"))));

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
                    summary, None, Some(res), Some(elapsed),
                );
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed),
                );
                Err(err)
            }
        }
    }
}

fn sanitize_endpoint(endpoint: &str) -> String {
    let mut clean = endpoint.trim().to_string();
    while clean.ends_with('/') {
        clean.pop();
    }
    if clean.ends_with("/chat/completions") {
        clean = clean[..clean.len() - "/chat/completions".len()].to_string();
    } else if clean.ends_with("/v1/chat/completions") {
        clean = clean[..clean.len() - "/v1/chat/completions".len()].to_string();
    } else if clean.ends_with("/v1/messages") {
        clean = clean[..clean.len() - "/v1/messages".len()].to_string();
    } else if clean.ends_with("/messages") {
        clean = clean[..clean.len() - "/messages".len()].to_string();
    }
    while clean.ends_with('/') {
        clean.pop();
    }
    clean
}

// ===========================================================================
// Core Streaming Runner
// ===========================================================================

const PREAMBLE: &str = r#"# 角色
你是 LakeMind 数据分析助手——一个严谨的数据分析师。你不猜测、不假设，用数据说话。

# 工作流程（严格按顺序执行）

## 第一步：探索
调用 `list_tables` 了解数据库中有哪些表。

## 第二步：理解
对与问题相关的表，调用 `describe_table` 获取结构，调用 `sample_data` 查看样例数据。

## 第三步：查询
基于理解，编写精确的 SQL 查询。如果一次查询不够，可以分多次查询。

## 第四步：总结
基于查询结果，用中文给出清晰的结论。结论必须引用具体数据。

# 输出格式要求
- 用 Markdown 格式回复
- 用 `##` 标题分隔每个步骤
- 数据结论用表格或列表呈现
- 关键数值用 **粗体** 标注

# 禁止行为
- 不要在没有数据支撑时反复猜测
- 不要写"等等"、"不对"、"让我重新想"这类自我纠正的文字
- 不要推翻自己的结论后又得出相同结论
- 不要在一段话中混杂猜测和结论
- 每个结论都必须基于查询结果
- 如果数据不足以回答问题，直接说明需要什么数据

# 思考语言
你的思考过程（reasoning）也必须用中文进行。不要用英文思考，即使问题用英文提出。思考内容应保持与中文回复一致的语言风格。

# 安全约束
禁止执行任何写操作（DELETE, DROP, UPDATE 等）。所有数据都必须从本地 SQL 查询获取。若需要可自行关联表查询。"#;

/// Rebuild the LLM chat history from persisted messages.
///
/// Legacy messages carry a flat `content` string; new messages carry `segments`.
/// Only visible text reaches the model — reasoning and tool steps are managed
/// by rig within the turn and are not replayed as history (matches prior
/// behavior, which only ever sent `content`).
fn assistant_text(msg: &ChatMessageDto) -> String {
    if let Some(c) = &msg.content {
        return c.clone();
    }
    if let Some(segs) = &msg.segments {
        let mut out = String::new();
        for s in segs {
            if let Segment::Text { text, .. } = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        return out;
    }
    String::new()
}

/// Drive the rig multi-turn stream: map each `MultiTurnStreamItem` to a frontend
/// event. Tool calls/results are NOT taken from rig's stream — each tool emits
/// its own richer `tool_call`/`tool_result` from inside `call()` (real status +
/// structured SqlResult). Rig's tool stream items are therefore ignored.
///
/// Generic over `R` (the provider's streaming-response type) so OpenAI
/// completions, OpenAI responses, and Anthropic streams all share this body.
async fn run_stream_loop<R>(
    window: tauri::Window,
    task_id: String,
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, StreamingError>> + Unpin,
) {
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                emit_delta(&window, &task_id, "text", &text_struct.text);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                emit_delta(&window, &task_id, "reasoning", &reasoning);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                let t = reasoning_struct.display_text();
                emit_delta(&window, &task_id, "reasoning", &t);
            }
            // Tool calls arrive here too, but the tools emit their own events
            // (with structured args/status/SqlResult). Ignore rig's variants.
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                emit_event(&window, &task_id, "error", Some(msg.clone()), None);
                return;
            }
        }
    }
}

pub async fn run_agent_chat_stream(
    window: tauri::Window,
    task_id: String,
    model_id: String,
    prompt: String,
    history_json: String,
    priority: String,
    app_state: AppState,
) -> Result<(), String> {
    // 1. Get model provider config
    let provider = get_provider_for_model(&model_id)?;

    // Map priority (最高/均衡/最快) → OpenAI reasoning_effort (high/medium/low).
    // For models that don't support this param, it's silently ignored by the API.
    let effort = match priority.as_str() {
        "最高" => "high",
        "最快" => "low",
        _ => "medium", // 均衡 or default
    };

    // Get max_tokens limit for the chosen model, defaulting to 4096 if not set
    let max_tokens_limit = provider.models.iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.max_tokens)
        .unwrap_or(4096) as u64;

    // 2. Parse chat history (tolerates legacy flat `content` and new `segments`)
    let history: Vec<ChatMessageDto> = serde_json::from_str(&history_json)
        .map_err(|e| format!("解析聊天历史失败: {e}"))?;

    let mut rig_history: Vec<Message> = Vec::new();
    for msg in history {
        if msg.role == "user" {
            if let Some(c) = &msg.content {
                rig_history.push(Message::user(c.clone()));
            }
        } else if msg.role == "assistant" {
            let text = assistant_text(&msg);
            if !text.is_empty() {
                rig_history.push(Message::assistant(text));
            }
        }
    }

    let list_tool = ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let desc_tool = DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let exec_tool = ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let sample_tool = SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };

    let format = provider.api_format.to_lowercase();
    if format == "openai" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;

        let mut agent_builder = client
            .completions_api()
            .agent(&model_id)
            .preamble(PREAMBLE)
            .max_tokens(max_tokens_limit)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(8)
            .await;
        run_stream_loop(window.clone(), task_id.clone(), stream).await;
    } else if format == "responses" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;

        let mut agent_builder = client
            .agent(&model_id)
            .preamble(PREAMBLE)
            .max_tokens(max_tokens_limit)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(8)
            .await;
        run_stream_loop(window.clone(), task_id.clone(), stream).await;
    } else if format == "anthropic" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::anthropic::Client = rig_core::providers::anthropic::Client::builder()
            .api_key(provider.api_key.clone())
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;

        let agent = client
            .agent(&model_id)
            .preamble(PREAMBLE)
            .max_tokens(4096)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .build();

        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(8)
            .await;
        run_stream_loop(window.clone(), task_id.clone(), stream).await;
    } else {
        return Err(format!("不支持的 API 格式: {}", provider.api_format));
    }

    // Emit done event
    emit_event(&window, &task_id, "done", None, None);

    Ok(())
}
