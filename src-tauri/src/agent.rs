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
use tokio::sync::oneshot;
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
    #[serde(rename_all = "camelCase")]
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
    /// Terminal/agent execution error.
    Error {
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
pub(crate) struct ModelProvider {
    id: String,
    name: String,
    endpoint: String,
    api_key: String,
    api_format: String, // "openai" | "anthropic" | "responses"
    models: Vec<ModelItem>,
    enabled: bool,
}

pub(crate) fn get_provider_for_model(model_id: &str) -> Result<ModelProvider, String> {
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

/// Return the id of the first enabled model in settings.json, or `None` when no
/// provider is configured. Used by the naming module to pick a model for
/// generating concise table names.
pub(crate) fn first_enabled_model() -> Option<String> {
    let mut path = crate::db::get_lakemind_dir().ok()?;
    path.push("settings.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let settings: SavedSettings = serde_json::from_str(&content).ok()?;
    for provider in settings.providers {
        if provider.enabled {
            if let Some(m) = provider.models.first() {
                return Some(m.id.clone());
            }
        }
    }
    None
}

/// One-shot LLM completion: stream the model's reply but gather all text locally
/// (no window events, no tools). Returns the concatenated assistant text. Used by
/// the naming module to ask for a concise table identifier. Mirrors the provider
/// client construction in [`run_agent_chat_stream`] but stripped down.
pub(crate) async fn complete_one_shot(prompt: &str, model_id: &str) -> Result<String, String> {
    let provider = get_provider_for_model(model_id)?;
    let format = provider.api_format.to_lowercase();
    let max_tokens: u64 = 64;

    if format == "openai" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.completions_api().agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "responses" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    if format == "anthropic" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::anthropic::Client =
            rig_core::providers::anthropic::Client::builder()
                .api_key(provider.api_key.clone())
                .base_url(&base_url)
                .build()
                .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;
        let agent = client.agent(model_id).max_tokens(max_tokens).build();
        let stream = agent.stream_chat(prompt.to_string(), Vec::<Message>::new()).multi_turn(0).await;
        return collect_stream_text(stream).await;
    }
    Err(format!("不支持的 API 格式: {}", provider.api_format))
}

/// Collect all assistant text deltas from a rig multi-turn stream into a single
/// string. Mirrors [`run_stream_loop`] but returns text instead of emitting
/// window events. Tool/reasoning items are ignored (no tools are attached for
/// one-shot calls).
async fn collect_stream_text<R>(
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, StreamingError>> + Unpin,
) -> Result<String, String> {
    use futures_util::StreamExt;
    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                out.push_str(&t.text);
            }
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Monotonic counter that guarantees unique tool-call ids. `now_ms()` alone is
/// millisecond-resolution, so two tools of the same kind starting in the same
/// millisecond (concurrent turns, or back-to-back fast metadata calls) would
/// collide — and the frontend merges a `tool_result` into the FIRST segment
/// matching that id, leaving the duplicate spinning forever. The counter makes
/// every id unique regardless of timing.
static TOOL_CALL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_tool_id(prefix: &str) -> String {
    let n = TOOL_CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("tool-{prefix}-{}-{n}", now_ms())
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

/// Emit a `tool_result` carrying `status: "awaiting"` — marks the tool segment
/// as parked pending the user's confirm/cancel decision in "变更前确认" mode.
/// `ddl` is the statement the tool intends to run, shown in the inline confirm UI.
fn emit_tool_awaiting(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    summary: String,
    ddl: String,
) {
    emit_event(window, task_id, "tool_result", None, Some(Segment::Tool {
        id: id.to_string(),
        tool: String::new(),
        args: None,
        status: "awaiting".to_string(),
        summary: Some(summary),
        sql: Some(ddl),
        table: None,
        elapsed_ms: None,
    }));
}

/// Validate a user-supplied identifier for use in a quoted DuckDB DDL statement.
/// Mirrors `commands::sanitize_ident`: rejects empty names and characters that
/// could break out of a double-quoted identifier.
fn sanitize_ddl_ident(name: &str) -> Result<String, ToolError> {
    if name.is_empty() || name.contains('"') || name.contains('\0') {
        return Err(ToolError("非法的表/视图名（不能为空，不能包含双引号）".to_string()));
    }
    Ok(name.to_string())
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
        let call_id = next_tool_id("list");
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

        let call_id = next_tool_id("desc");
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

        let call_id = next_tool_id("exec");
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

        let call_id = next_tool_id("sample");
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

// ===========================================================================
// DDL Tools — create_table / create_view / drop_object
//
// These carry the "变更能力" (write capability). In "变更前确认" mode their
// `call()` parks on a oneshot channel until the user approves/cancels from the
// UI (resolve_tool_confirmation). In "自动执行" mode they run immediately.
// The read-only `execute_query` tool is unchanged and still rejects all DDL.
// ===========================================================================

/// Shared state for the three DDL tools.
#[derive(Clone)]
struct DdlToolShared {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
    confirm_mode: String,
}

impl DdlToolShared {
    /// Drive one DDL operation end-to-end respecting the confirm mode.
    ///
    /// `tool_name`/`tool_prefix` identify the tool in the transcript.
    /// `args` is forwarded to the UI via the tool_call segment.
    /// `summary_pending` describes the not-yet-run action for the awaiting UI.
    /// `build_ddl` returns the final statement(s) to execute.
    /// `object_meta` = `Some((name, select_sql, kind))` for create_table/create_view
    /// (enables incremental caching + persists the definition); `None` for drop.
    /// Returns the string fed back to the LLM.
    async fn run<F>(
        &self,
        tool_name: &str,
        tool_prefix: &str,
        args: serde_json::Value,
        summary_pending: String,
        object_meta: Option<(String, String, String)>,
        build_ddl: F,
    ) -> Result<String, ToolError>
    where
        F: FnOnce() -> String + Send + 'static,
    {
        let call_id = next_tool_id(tool_prefix);
        emit_tool_call(&self.window, &self.task_id, &call_id, tool_name, args);

        let ddl = build_ddl();

        // Incremental cache: for create_table/create_view, if the upstream
        // fingerprint is unchanged AND the lake object still exists, skip the
        // expensive DROP+CREATE and reuse the existing object.
        if let Some((name, select_sql, _kind)) = &object_meta {
            if self.can_reuse_object(name, select_sql).await {
                let summary = format!("复用已有的{}（输入未变化）", summary_pending);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), Some(ddl), None, None,
                );
                return Ok(summary);
            }
        }

        // "变更前确认": park until the user decides. "自动执行" (or any other
        // value) falls through to immediate execution.
        if self.confirm_mode == "变更前确认" {
            let (tx, rx) = oneshot::channel::<crate::state::ConfirmDecision>();
            {
                let key = format!("{}:{}", self.task_id, call_id);
                let mut pending = self.app_state.pending_confirmations.lock().await;
                pending.insert(key.clone(), crate::state::PendingConfirmation { tx });
            }
            // Notify the UI this step is awaiting the user.
            emit_tool_awaiting(&self.window, &self.task_id, &call_id, summary_pending.clone(), ddl.clone());

            let decision = rx.await;
            match decision {
                Ok(d) if d.approved => {
                    // fall through to execute below
                }
                _ => {
                    let msg = "用户已取消此操作".to_string();
                    emit_tool_result(
                        &self.window, &self.task_id, &call_id, "error",
                        msg.clone(), Some(ddl), None, None,
                    );
                    return Err(ToolError(msg));
                }
            }
        }

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ddl_for_exec = ddl.clone();
        let exec_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            // DuckDB `execute` runs a single statement; our DDL strings already
            // contain semicolons separating DROP + CREATE, so use execute_batch.
            guard.execute_batch(&ddl_for_exec)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")));

        let elapsed = start.elapsed().as_millis() as u64;
        match exec_res {
            Ok(Ok(())) => {
                // Persist the definition + fingerprint so future builds can skip
                // re-materialization, and so the object can be rebuilt after a
                // lake crash-recovery.
                if let Some((name, select_sql, kind)) = object_meta {
                    self.persist_object_def(&name, &select_sql, &kind).await;
                }
                // drop_object: remove any persisted definition for this name.
                if tool_name == "drop_object" {
                    if let Some(name) = ddl_extract_name(&ddl) {
                        self.delete_object_def(&name).await;
                    }
                }
                let summary = format!("{}成功", summary_pending);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), Some(ddl), None, Some(elapsed),
                );
                Ok(summary)
            }
            Ok(Err(e)) => {
                let msg = format!("执行失败: {e}");
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), Some(ddl), None, Some(elapsed),
                );
                Err(ToolError(msg))
            }
            Err(e) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    e.0.clone(), Some(ddl), None, Some(elapsed),
                );
                Err(e)
            }
        }
    }

    /// True iff an `object_defs` record exists for `name`, its current upstream
    /// fingerprint matches the stored one, and the lake object still exists.
    async fn can_reuse_object(&self, name: &str, select_sql: &str) -> bool {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let name = name.to_string();
        let select_sql = select_sql.to_string();
        let conn_clone = self.app_state.conn.clone();
        tokio::task::spawn_blocking(move || -> bool {
            let Ok(sqlite) = crate::db::get_db_conn() else { return false };
            let Ok(Some(def)) = crate::db::get_object_def(&sqlite, &ws_path, &name) else {
                return false;
            };
            let upstreams = crate::fingerprint::extract_upstreams(&select_sql);
            let current_hash =
                crate::fingerprint::compute_input_hash(&sqlite, &ws_path, &select_sql, &upstreams);
            if current_hash != def.input_hash {
                return false;
            }
            // Object still materialized in the lake?
            let guard = conn_clone.blocking_lock();
            crate::commands::table_exists_in_lake(&guard, &name)
        })
        .await
        .unwrap_or(false)
    }

    /// Persist (or refresh) the `object_defs` row for a freshly-built object.
    async fn persist_object_def(&self, name: &str, select_sql: &str, kind: &str) {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let name = name.to_string();
        let select_sql = select_sql.to_string();
        let kind = kind.to_string();
        let created_at = now_ms();
        let conn = self.app_state.conn.clone();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let sqlite = crate::db::get_db_conn()?;
            let upstreams = crate::fingerprint::extract_upstreams(&select_sql);
            let input_hash =
                crate::fingerprint::compute_input_hash(&sqlite, &ws_path, &select_sql, &upstreams);
            // Capture the freshly-built object's columns + row count so the
            // data-tree list can read them from SQLite (no DuckDB query) as long
            // as the upstream fingerprint is unchanged.
            let (columns, row_count) = {
                let guard = conn.blocking_lock();
                let cols = crate::duckdb::schema::describe_view(&guard, &name).unwrap_or_default();
                let escaped = name.replace('"', "\"\"");
                let cnt = guard
                    .query_row(
                        &format!("SELECT count(*) FROM \"{}\"", escaped),
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .ok();
                (cols, cnt)
            };
            crate::db::upsert_object_def(
                &sqlite,
                &ws_path,
                &crate::db::ObjectDef {
                    table_name: name,
                    kind,
                    select_sql,
                    input_hash,
                    created_at,
                    columns,
                    row_count,
                },
            )
        })
        .await;
    }

    /// Remove the `object_defs` row for `name` (called after a successful drop).
    async fn delete_object_def(&self, name: &str) {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let name = name.to_string();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let sqlite = crate::db::get_db_conn()?;
            crate::db::delete_object_def(&sqlite, &ws_path, &name)
        })
        .await;
    }
}

/// Best-effort extraction of the target object name from a DROP DDL string, for
/// cleaning up the persisted definition after `drop_object`.
fn ddl_extract_name(ddl: &str) -> Option<String> {
    // Matches: DROP (VIEW|TABLE) IF EXISTS "name";  → returns name
    let upper = ddl.to_uppercase();
    let idx = upper.find("DROP ")?;
    let after = &ddl[idx..];
    // Find the first quoted identifier.
    let q = after.find('"')?;
    let rest = &after[q + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// --- create_table ---------------------------------------------------------

#[derive(Deserialize, Serialize)]
struct CreateTableArgs {
    name: String,
    select_sql: String,
}

struct CreateTableTool {
    shared: DdlToolShared,
}

impl Tool for CreateTableTool {
    const NAME: &'static str = "create_table";
    type Error = ToolError;
    type Args = CreateTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "create_table".to_string(),
            description: "创建一张物理表（物化存储）来持久化加工后的数据。传入新表名与一条 SELECT 语句，结果会被物化写入。若同名表已存在会先删除再创建。命名建议用 t_ 前缀（最终表）或 tmp_ 前缀（中间表）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "新表名（遵循命名规范，如 t_sales、tmp_joined）" },
                    "select_sql": { "type": "string", "description": "用于生成该表的 SELECT 查询语句" }
                },
                "required": ["name", "select_sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(args.name.trim())?;
        let select_sql = args.select_sql.trim().to_string();
        if select_sql.is_empty() {
            return Err(ToolError("select_sql 不能为空".to_string()));
        }
        let summary_pending = format!("创建物理表 {}", name);
        self.shared.run(
            "create_table", "ct",
            json!({ "name": name, "select_sql": select_sql }),
            summary_pending,
            Some((name.clone(), select_sql.clone(), "table".to_string())),
            move || format!(
                "DROP TABLE IF EXISTS \"{name}\";\nCREATE TABLE \"{name}\" AS {select_sql};",
                name = name,
                select_sql = select_sql
            ),
        )
        .await
    }
}

// --- create_view ----------------------------------------------------------

#[derive(Deserialize, Serialize)]
struct CreateViewArgs {
    name: String,
    select_sql: String,
}

struct CreateViewTool {
    shared: DdlToolShared,
}

impl Tool for CreateViewTool {
    const NAME: &'static str = "create_view";
    type Error = ToolError;
    type Args = CreateViewArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "create_view".to_string(),
            description: "创建一个虚拟视图（零拷贝，不物化数据）来封装加工逻辑。传入新视图名与一条 SELECT 语句。若同名对象已存在会先删除再创建。命名建议用 v_ 前缀（最终视图）或 tmp_v_ 前缀（中间视图）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "新视图名（遵循命名规范，如 v_sales、tmp_v_filtered）" },
                    "select_sql": { "type": "string", "description": "定义该视图的 SELECT 查询语句" }
                },
                "required": ["name", "select_sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(args.name.trim())?;
        let select_sql = args.select_sql.trim().to_string();
        if select_sql.is_empty() {
            return Err(ToolError("select_sql 不能为空".to_string()));
        }
        let summary_pending = format!("创建视图 {}", name);
        self.shared.run(
            "create_view", "cv",
            json!({ "name": name, "select_sql": select_sql }),
            summary_pending,
            Some((name.clone(), select_sql.clone(), "view".to_string())),
            move || format!(
                "DROP VIEW IF EXISTS \"{name}\";\nDROP TABLE IF EXISTS \"{name}\";\nCREATE VIEW \"{name}\" AS {select_sql};",
                name = name,
                select_sql = select_sql
            ),
        )
        .await
    }
}

// --- drop_object ----------------------------------------------------------

#[derive(Deserialize, Serialize)]
struct DropObjectArgs {
    name: String,
}

struct DropObjectTool {
    shared: DdlToolShared,
}

impl Tool for DropObjectTool {
    const NAME: &'static str = "drop_object";
    type Error = ToolError;
    type Args = DropObjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "drop_object".to_string(),
            description: "删除指定的表或视图（同时清理同名视图与表两种形态）。删除后数据不可恢复，请确认后再使用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "要删除的表或视图名" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(args.name.trim())?;
        let summary_pending = format!("删除对象 {}", name);
        self.shared.run(
            "drop_object", "drop",
            json!({ "name": name }),
            summary_pending,
            None,
            move || format!(
                "DROP VIEW IF EXISTS \"{name}\";\nDROP TABLE IF EXISTS \"{name}\";",
                name = name
            ),
        )
        .await
    }
}

pub(crate) fn sanitize_endpoint(endpoint: &str) -> String {
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
你是 LakeMind 数据分析助手——一个严谨的数据分析师。你不猜测、不假设，用数据说话。你不只是回答问题，还能主动加工数据、沉淀结果。

# 工作流程（严格按顺序执行）

## 第一步：探索
调用 `list_tables` 了解数据库中有哪些表。

## 第二步：理解
对与问题相关的表，调用 `describe_table` 获取结构，调用 `sample_data` 查看样例数据。

## 第三步：查询或加工
基于理解，判断接下来该做什么——

### A. 只需要查询
如果用一次 SELECT 就能回答，直接调用 `execute_query` 执行即可。

### B. 需要加工数据（主动判断，不要等用户指令）
当出现以下情况时，**你应该立即用 DDL 工具把结果沉淀下来，而不是只在回答里贴一段 SQL**：
- 任务涉及多步清洗（去重、过滤脏数据、类型转换、派生字段），且结果会被后续分析复用。
- 需要把多张表关联（JOIN）成一个清晰的结果集。
- 用户明确要求"建表/落表/保存结果/整理成一张表/做成视图"。
- 数据源是只读的 `s_` 视图，需要产出一份可重复使用的干净数据。

此时按用途选择工具：
- **`create_table`**：结果需要物化存储、或源数据很大需要避免重复扫描 → 用 `t_` 前缀（最终表）或 `tmp_` 前缀（中间表）。
- **`create_view`**：只是封装一段查询逻辑、源数据不大、希望随源更新 → 用 `v_` 前缀（最终视图）或 `tmp_v_` 前缀（中间视图）。
- **`drop_object`**：仅当用户明确要求删除，或你创建的中间 `tmp_` 表已用完且想清理时使用。

操作准则：
- 建表/视图的 `select_sql` 必须先用 `execute_query` 验证能跑通、字段正确，再调用 `create_table`/`create_view`。
- 一次只创建一个对象；创建后用 `describe_table` 确认结构符合预期。
- 如果是多步加工，先用 `tmp_`/`tmp_v_` 搭中间结果，最后产出 `t_`/`v_`。

## 第四步：总结
基于查询或加工的结果，用中文给出清晰的结论。结论必须引用具体数据。若创建了表/视图，说明它叫什么、用途是什么。

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
- 不要只给出 SQL 文本让用户自己跑——需要加工时就主动用工具创建对象

# 数据库命名规范（创建表/视图时必须遵循）
- `s_`：源文件映射的原始只读视图（如 `s_sales`），可能包含头部备注等脏数据。**只读，不要创建 s_ 开头的对象。**
- `tmp_`：中间过渡物理表（如 `tmp_sales_joined`）。
- `tmp_v_`：中间过渡虚拟视图（如 `tmp_v_order_filtered`）。
- `t_`：最终清洗加工后的可用物理表（如 `t_sales`）。
- `v_`：最终清洗加工后的可用虚拟视图（如 `v_sales`）。

# 思考语言
你的思考过程（reasoning）也必须用中文进行。不要用英文思考，即使问题用英文提出。思考内容应保持与中文回复一致的语言风格。

# 安全约束
- `execute_query` 工具仅用于只读查询（SELECT），禁止通过它执行任何写操作（DELETE, DROP, UPDATE, INSERT, ALTER 等）。
- 所有创建/删除表/视图的操作，只能通过专用工具：`create_table`、`create_view`、`drop_object`。
- 删除操作不可恢复，仅当用户明确要求时才调用 `drop_object`。"#;

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
    confirm_mode: String,
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
    let ddl_shared = DdlToolShared {
        app_state: app_state.clone(),
        task_id: task_id.clone(),
        window: window.clone(),
        confirm_mode: confirm_mode.clone(),
    };
    let create_table_tool = CreateTableTool { shared: ddl_shared.clone() };
    let create_view_tool = CreateViewTool { shared: ddl_shared.clone() };
    let drop_object_tool = DropObjectTool { shared: ddl_shared };

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
            .tool(sample_tool)
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
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
            .tool(sample_tool)
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
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
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool)
            .build();

        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
            .await;
        run_stream_loop(window.clone(), task_id.clone(), stream).await;
    } else {
        return Err(format!("不支持的 API 格式: {}", provider.api_format));
    }

    // Emit done event
    emit_event(&window, &task_id, "done", None, None);

    Ok(())
}
