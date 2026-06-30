use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
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
use crate::usage::{self, PREAMBLE};

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
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
    },
    /// Visible answer text (Markdown). Accumulated from text deltas.
    Text {
        id: String,
        text: String,
    },
    /// ECharts visualization — emitted by the `render_chart` tool. Carries the
    /// chart config (type + axis mapping) plus the raw `SqlResult` so the
    /// frontend can render and let the user switch chart types.
    #[serde(rename_all = "camelCase")]
    Chart {
        id: String,
        chart_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        x_field: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        y_fields: Option<Vec<String>>,
        table: SqlResult,
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
    #[serde(default)]
    query_timeout: Option<u64>,
    /// Hard timeout: the maximum wall-clock seconds the caller will wait before
    /// returning an error, regardless of whether the DuckDB interrupt has taken
    /// effect. `None` means "derive from soft timeout".
    #[serde(default)]
    query_hard_timeout: Option<u64>,
}

/// Read the settings.json file once and return the parsed struct.
/// Centralises the file-read so both getters share the same snapshot.
fn read_saved_settings() -> SavedSettings {
    let fallback = SavedSettings {
        providers: Vec::new(),
        query_timeout: Some(60),
        query_hard_timeout: None,
    };
    let mut path = match crate::db::get_lakemind_dir() {
        Ok(p) => p,
        Err(_) => return fallback,
    };
    path.push("settings.json");
    if !path.exists() {
        return fallback;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return fallback,
    };
    serde_json::from_str(&content).unwrap_or(fallback)
}

pub(crate) fn get_query_timeout() -> Option<u64> {
    read_saved_settings().query_timeout.or(Some(60))
}

/// Hard timeout: the absolute wall-clock cap for any query execution.
/// If not explicitly configured, defaults to `soft_timeout × 2`.
/// Returns 0 when both soft and hard are disabled ("no limit").
pub(crate) fn get_query_hard_timeout() -> u64 {
    let settings = read_saved_settings();
    let soft = settings.query_timeout.unwrap_or(60);
    if soft == 0 {
        // Soft timeout disabled — honour explicit hard timeout or disable too.
        return settings.query_hard_timeout.unwrap_or(0);
    }
    settings.query_hard_timeout.unwrap_or(soft.saturating_mul(2))
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
        result: None,
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
    result: Option<String>,
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
        result,
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
        result: None,
    }));
}

/// Emit a `chart` segment — a complete chart config + data, rendered inline in
/// the conversation by the frontend's ChartSegment component.
fn emit_chart(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    chart_type: &str,
    title: Option<&str>,
    x_field: Option<&str>,
    y_fields: Option<&[String]>,
    table: SqlResult,
) {
    emit_event(window, task_id, "chart", None, Some(Segment::Chart {
        id: id.to_string(),
        chart_type: chart_type.to_string(),
        title: title.map(|s| s.to_string()),
        x_field: x_field.map(|s| s.to_string()),
        y_fields: y_fields.map(|v| v.to_vec()),
        table,
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
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let tables_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let mut stmt = guard.prepare(sql).map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(|e| e.to_string())?);
            }
            
            // Read sampling metadata from SQLite
            let sqlite = crate::db::get_db_conn().map_err(|e| e.to_string())?;
            let records = crate::db::list_sources(&sqlite, &ws_path).map_err(|e| e.to_string())?;
            
            let mut descriptions = Vec::new();
            for tname in list {
                if let Some(rec) = records.iter().find(|r| r.table_name == tname) {
                    if rec.is_sampled {
                        let full_rows_str = rec.full_row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        descriptions.push(format!(
                            "{} (已本地物化采样 {} 行，外部全量大约 {} 行，类型是 \"{}\"，全量直连路径为 \"{}\")",
                            tname,
                            rec.row_count.unwrap_or(0),
                            full_rows_str,
                            rec.kind,
                            rec.scan_path
                        ));
                    } else {
                        descriptions.push(format!(
                            "{} (全量，类型是 \"{}\"，行数: {})",
                            tname,
                            rec.kind,
                            rec.row_count.map(|c: i64| c.to_string()).unwrap_or_else(|| "未知".to_string())
                        ));
                    }
                } else {
                    descriptions.push(tname);
                }
            }
            
            Ok::<_, String>(descriptions)
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
                    summary, None, None, Some(elapsed), None,
                );
                Ok(format!("当前可用的数据库表列表为: {}", tables.join("; ")))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
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
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_string = table_name.to_string();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("DESCRIBE \"{}\"", table_name_string.replace('"', "\"\""));
            let query_res = execute::run_query(&guard, &sql, None).map_err(|e| e.to_string())?;
            let (okf_title, col_comments, relations) = crate::okf::parse_column_semantics(&ws_dir, &table_name_string);
            
            // Read SQLite cache for details
            let sqlite = crate::db::get_db_conn().map_err(|e| e.to_string())?;
            let source_record = crate::db::get_source_by_table(&sqlite, &ws_path, &table_name_string).map_err(|e| e.to_string())?;
            
            Ok::<_, String>((query_res, okf_title, col_comments, relations, source_record))
        });
        let desc_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}")))),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}"))))
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match desc_res {
            Ok((res, okf_title, col_comments, relations, source_record)) => {
                let col_lines: Vec<String> = res.rows.iter().map(|r| {
                    let name = r.get(0).map(|v| v.to_string()).unwrap_or_default();
                    let ty = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let null = r.get(2).map(|v| v.to_string()).unwrap_or_default();
                    let comment = col_comments.get(&name).map(|c| format!(", 释义: {}", c)).unwrap_or_default();
                    format!("{} (类型: {}, 允许空: {}){}", name, ty, null, comment)
                }).collect();
                let n = res.rows.len();
                let mut title_part = String::new();
                if let Some(t) = okf_title {
                    title_part = format!(" (业务名称: {})", t);
                }
                let mut rels_part = String::new();
                if !relations.is_empty() {
                    rels_part = format!("\n\n关联关系:\n{}", relations.iter().map(|r| format!("- {}", r)).collect::<Vec<_>>().join("\n"));
                }
                
                let mut header_info = String::new();
                if let Some(ref rec) = source_record {
                    if rec.is_sampled {
                        let full_rows_str = rec.full_row_count.map(|c| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        let db_alias = rec.scan_path.split('.').next().unwrap_or("db_conn");
                        header_info = format!(
                            " [注意: 该表当前是本地物化采样缓存，包含 {} 行数据，连接数据库类型为 \"{}\"。外部生产数据库的全量总行数大约为 {} 行。如果你需要进行全量汇总或分析完整数据，请优先使用原生下推函数，直接在 SQL 中查询 \"SELECT * FROM {}_query('{}', '...')\"]",
                            rec.row_count.unwrap_or(0),
                            rec.kind,
                            full_rows_str,
                            rec.kind,
                            db_alias
                        );
                    } else {
                        let rows_str = rec.row_count.map(|c| c.to_string()).unwrap_or_else(|| "未知".to_string());
                        header_info = format!(" [全量数据表，连接数据库类型为 \"{}\"，行数: {}]", rec.kind, rows_str);
                    }
                }
                
                let summary = format!("结构分析完成，{}{}{}, 共 {} 个字段", table_name, title_part, header_info, n);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary, None, Some(res), Some(elapsed), None,
                );
                Ok(format!("表 {}{} 的列结构如下:\n{}{}", table_name, title_part, col_lines.join("\n"), rels_part))
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}

// ===========================================================================
// OKF Knowledge System Rig Tools
// ===========================================================================

#[derive(Deserialize, Serialize)]
struct LoadOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
}

struct LoadOkfBlockTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
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

#[derive(Deserialize, Serialize)]
struct WriteOkfBlockArgs {
    category: String,
    name: String,
    heading: String,
    content: String,
}

struct WriteOkfBlockTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
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

#[derive(Deserialize, Serialize)]
struct SearchOkfRecipesArgs {
    query: String,
}

struct SearchOkfRecipesTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
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

#[derive(Deserialize, Serialize)]
struct CheckSourceFingerprintArgs {
    file_path: String,
}

struct CheckSourceFingerprintTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
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

#[derive(Deserialize, Serialize)]
struct TidyOkfKnowledgeArgs {}

struct TidyOkfKnowledgeTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
    model_id: String,
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
                crate::agent::complete_one_shot(&cleanup_prompt, &model_id_clone).await
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

fn parse_okf_files(output: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let marker = "```okf-file";
    let end_marker = "```";
    
    let mut cur = output;
    while let Some(start_idx) = cur.find(marker) {
        let content_start = start_idx + marker.len();
        let after_start = &cur[content_start..];
        if let Some(end_idx) = after_start.find(end_marker) {
            let block = &after_start[..end_idx];
            let block_trimmed = block.trim();
            if block_trimmed.starts_with("FILE:") {
                if let Some(line_end) = block_trimmed.find('\n') {
                    let file_line = &block_trimmed[..line_end];
                    let file_path = file_line.trim_start_matches("FILE:").trim().to_string();
                    let file_content = block_trimmed[line_end..].trim().to_string();
                    if !file_path.is_empty() && !file_content.is_empty() {
                        files.push((file_path, file_content));
                    }
                }
            }
            cur = &after_start[end_idx + end_marker.len()..];
        } else {
            break;
        }
    }
    files
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn delete_md_files(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            delete_md_files(&entry.path())?;
            if fs::read_dir(entry.path())?.next().is_none() {
                let _ = fs::remove_dir(entry.path());
            }
        } else if entry.path().extension().and_then(|s| s.to_str()) == Some("md") {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn backup_and_rewrite_okf(okf_dir: &Path, new_files: &[(String, String)]) -> Result<String, String> {
    let backup_dir = okf_dir.parent().unwrap_or(okf_dir).join("okf_backup_temp");
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(&backup_dir);
    }
    if okf_dir.exists() {
        copy_dir_all(okf_dir, &backup_dir).map_err(|e| format!("备份失败: {}", e))?;
    }
    let res: Result<usize, String> = (|| {
        if okf_dir.exists() {
            delete_md_files(okf_dir).map_err(|e| e.to_string())?;
        }
        let mut written_count = 0;
        for (rel_path, content) in new_files {
            if rel_path.contains("..") || rel_path.starts_with('/') {
                return Err(format!("非法的文件路径: {}", rel_path));
            }
            let full_path = okf_dir.join(rel_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&full_path, content).map_err(|e| e.to_string())?;
            written_count += 1;
        }
        Ok(written_count)
    })();
    match res {
        Ok(written_count) => {
            if backup_dir.exists() {
                let _ = fs::remove_dir_all(&backup_dir);
            }
            Ok(format!("成功自动整理和提炼了本地知识库。重构写入了 {} 个干净的 OKF 知识文件。", written_count))
        }
        Err(e) => {
            if backup_dir.exists() {
                if okf_dir.exists() {
                    let _ = fs::remove_dir_all(okf_dir);
                }
                let _ = copy_dir_all(&backup_dir, okf_dir);
                let _ = fs::remove_dir_all(&backup_dir);
            }
            Err(format!("自动整理知识库失败，已安全回滚到整理前的状态。错误: {}", e))
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
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let sql_string = sql.to_string();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50))
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("SQL 执行出错: {e}")))),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("SQL 执行已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("SQL 执行出错: {e}"))))
        };

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
                    summary, Some(sql.to_string()), Some(res), Some(elapsed), None,
                );
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), Some(sql.to_string()), None, Some(elapsed), None,
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
            description: "获取指定数据表或视图的前 5 行样例数据。用于直观了解数据的具体内容 and 字段格式。".to_string(),
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
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_string = table_name.to_string();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("SELECT * FROM {table_name_string} LIMIT 5");
            execute::run_query(&guard, &sql, Some(5))
        });
        let query_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                    .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}")))),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!("采样查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程生成失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("采样查询失败: {e}"))))
        };

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
                    summary, None, Some(res), Some(elapsed), None,
                );
                Ok(out)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
struct MaterializeRemoteTableArgs {
    table_name: String,
}

struct MaterializeRemoteTableTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for MaterializeRemoteTableTool {
    const NAME: &'static str = "materialize_remote_table";
    type Error = ToolError;
    type Args = MaterializeRemoteTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "materialize_remote_table".to_string(),
            description: "将指定的外部数据库表/采样表（如 s_db_cdp_message_sending_notification）完整导入为本地 DuckDB 物理表，以实现全量数据的高速本地分析 and 聚合。此操作在数据表极大时可能会消耗较多的网络 and 存储空间。当需要对一个大表进行多次复杂的 OLAP 聚合分析（如频繁 GROUP BY / ORDER BY）时，你应该自主判断是否建议 or 执行此操作，从而避免跨网络全表扫描带来的极大延迟。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要进行全量本地物化的采样表或外部表名，例如 s_postgres_users" }
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

        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let conn = self.app_state.conn.clone();
        let table_name_str = table_name.to_string();

        let call_id = next_tool_id("mat");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "materialize_remote_table",
            json!({ "table_name": table_name }),
        );

        let start = std::time::Instant::now();

        // 1. Get SourceRecord from SQLite with fuzzy/fallback matching
        let source_record_opt = tokio::task::spawn_blocking(move || -> Result<Option<crate::db::SourceRecord>, String> {
            let sqlite = crate::db::get_db_conn()?;
            // Try exact match first
            if let Ok(Some(rec)) = crate::db::get_source_by_table(&sqlite, &ws_path, &table_name_str) {
                return Ok(Some(rec));
            }
            // Fetch all sources to perform fallback fuzzy matching (e.g. user passes message_sending_notification, we match s_cdp_message_sending_notification)
            let all_sources = crate::db::list_sources(&sqlite, &ws_path)?;
            for src in all_sources {
                if src.table_name.starts_with("s_") && (src.table_name.ends_with(&format!("_{}", table_name_str)) || src.table_name == table_name_str) {
                    return Ok(Some(src));
                }
                if src.table_name == table_name_str || src.scan_path.ends_with(&format!(".{}", table_name_str)) {
                    return Ok(Some(src));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))))?;

        let source_record = match source_record_opt {
            Some(r) => r,
            None => {
                let err_msg = format!("未找到该表 '{}' 的注册元数据。确保它是已挂载的外部表。", table_name);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err_msg.clone(), None, None, Some(start.elapsed().as_millis() as u64), None,
                );
                return Err(ToolError(err_msg));
            }
        };

        let actual_table_name = source_record.table_name.clone();
        let window_for_progress = self.window.clone();
        let task_id_for_progress = self.task_id.clone();
        let call_id_for_progress = call_id.clone();
        let approx_total = source_record.full_row_count.unwrap_or(0);

        // 2. Perform table drop & drop view, then CREATE TABLE AS SELECT * in chunks with progress reporting
        let full_path = source_record.scan_path.clone();
        let table_name_clone = actual_table_name.clone();
        let conn_clone = conn.clone();

        let exec_res = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let guard = conn_clone.blocking_lock();
            // Drop view/table to clean up any existing sample view/table
            let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", table_name_clone), []);
            let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", table_name_clone), []);
            
            // Instantly create the empty table schema locally
            let create_schema_sql = format!("CREATE TABLE \"{}\" AS SELECT * FROM {} LIMIT 0;", table_name_clone, full_path);
            guard.execute(&create_schema_sql, []).map_err(|e| e.to_string())?;

            // Retrieve data from remote
            let select_sql = format!("SELECT * FROM {};", full_path);
            let mut stmt = guard.prepare(&select_sql).map_err(|e| e.to_string())?;
            let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

            let mut appender = guard.appender(&table_name_clone).map_err(|e| e.to_string())?;
            
            let mut count = 0;
            let mut last_emit = std::time::Instant::now();

            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let mut row_vals = Vec::new();
                let mut idx = 0;
                while let Ok(val) = row.get::<usize, duckdb::types::Value>(idx) {
                    row_vals.push(val);
                    idx += 1;
                }

                let to_sql_row: Vec<&dyn duckdb::ToSql> = row_vals.iter().map(|v| v as &dyn duckdb::ToSql).collect();
                appender.append_row(duckdb::appender_params_from_iter(to_sql_row)).map_err(|e| e.to_string())?;

                count += 1;

                if count % 20000 == 0 || last_emit.elapsed() >= std::time::Duration::from_millis(500) {
                    let pct_str = if approx_total > 0 {
                        format!(" ({:.1}%)", (count as f64 / approx_total as f64) * 100.0)
                    } else {
                        String::new()
                    };
                    let prog_summary = format!("正在导入数据: 已物化 {} 行{}...", count, pct_str);
                    emit_tool_result(
                        &window_for_progress,
                        &task_id_for_progress,
                        &call_id_for_progress,
                        "running",
                        prog_summary,
                        None,
                        None,
                        None,
                        None,
                    );
                    last_emit = std::time::Instant::now();
                }
            }

            std::mem::drop(appender);
            Ok(count)
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("物化数据失败: {e}"))));

        let elapsed = start.elapsed().as_millis() as u64;

        match exec_res {
            Ok(imported_rows) => {
                // 3. Update metadata in SQLite
                let ws_path = self.app_state.workspace_path.lock().await.clone();
                let table_name_clone = actual_table_name.clone();
                let conn_clone = conn.clone();
                let mut updated_record = source_record;
                updated_record.storage = "table".to_string();
                updated_record.is_sampled = false;
                updated_record.row_count = Some(imported_rows);
                updated_record.full_row_count = Some(imported_rows);

                let update_db_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let sqlite = crate::db::get_db_conn()?;
                    // Describe the new columns schema
                    let guard = conn_clone.blocking_lock();
                    if let Ok(new_cols) = crate::duckdb::schema::describe_view(&guard, &table_name_clone) {
                        updated_record.columns = new_cols;
                    }
                    crate::db::upsert_source(&sqlite, &ws_path, &updated_record)?;
                    Ok(())
                })
                .await
                .map_err(|e| ToolError(format!("线程执行失败: {e}")))
                .and_then(|res| res.map_err(|e| ToolError(format!("更新元数据失败: {e}"))));

                if let Err(err) = update_db_res {
                    emit_tool_result(
                        &self.window, &self.task_id, &call_id, "error",
                        err.0.clone(), None, None, Some(elapsed), None,
                    );
                    return Err(err);
                }

                let summary = format!("成功将外部表 {} (本地表名: {}) 完整物化到本地 DuckDB，共导入 {} 行数据。", table_name, actual_table_name, imported_rows);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Err(err) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err.0.clone(), None, None, Some(elapsed), None,
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
                    summary.clone(), Some(ddl), None, None, None,
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
                        msg.clone(), Some(ddl), None, None, None,
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
                // drop_object: remove persisted definitions for ALL dropped names
                // (cascade delete may drop multiple objects in one batch).
                if tool_name == "drop_object" {
                    for name in ddl_extract_all_names(&ddl) {
                        self.delete_object_def(&name).await;
                    }
                }
                let summary = format!("{}成功", summary_pending);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), Some(ddl), None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Ok(Err(e)) => {
                let msg = format!("执行失败: {e}");
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), Some(ddl), None, Some(elapsed), None,
                );
                Err(ToolError(msg))
            }
            Err(e) => {
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    e.0.clone(), Some(ddl), None, Some(elapsed), None,
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
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
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
            let obj = crate::db::ObjectDef {
                table_name: name.clone(),
                kind: kind.clone(),
                select_sql: select_sql.clone(),
                input_hash,
                created_at,
                columns: columns.clone(),
                row_count,
            };
            crate::db::upsert_object_def(&sqlite, &ws_path, &obj)?;
            if kind == "table" {
                let _ = crate::okf::write_table_okf(&ws_dir, &name, &columns, row_count);
            } else {
                let _ = crate::okf::write_view_okf(&ws_dir, &name, &select_sql, &columns);
            }
            Ok(())
        })
        .await;
    }

    /// Remove the `object_defs` and `sources` rows for `name` (called after a
    /// successful drop). Both tables are checked so s_ source tables (in sources)
    /// and t_/v_ views (in object_defs) are both cleaned up.
    async fn delete_object_def(&self, name: &str) {
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let ws_dir = self.app_state.workspace_dir.lock().await.to_string_lossy().to_string();
        let name = name.to_string();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let sqlite = crate::db::get_db_conn()?;
            let _ = crate::db::delete_object_def(&sqlite, &ws_path, &name);
            let _ = crate::db::delete_source_by_table(&sqlite, &ws_path, &name);
            let _ = crate::okf::delete_okf_files(&ws_dir, &name);
            Ok(())
        })
        .await;
    }
}

/// Best-effort extraction of the target object name from a DROP DDL string, for
/// cleaning up the persisted definition after `drop_object`.
/// Extract ALL quoted object names from a DROP DDL batch (cascade delete may
/// contain multiple `DROP VIEW/TABLE IF EXISTS "name";` statements).
fn ddl_extract_all_names(ddl: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = ddl;
    while let Some(q) = rest.find('"') {
        let after = &rest[q + 1..];
        if let Some(end) = after.find('"') {
            let name = &after[..end];
            if !name.is_empty() {
                names.push(name.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    names
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
            description: "删除指定的表或视图。如果该对象有下游依赖（其他表/视图引用了它），会自动级联删除所有下游依赖（先删下游再删目标）。删除后数据不可恢复，请确认后再使用。".to_string(),
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

        // 1. Compute the cascade deletion order: all transitive downstreams
        //    (leaves first) + the target last. This way one batch of DROP
        //    statements removes everything without "still referenced" errors.
        let ws_path = self.shared.app_state.workspace_path.lock().await.clone();
        let name_for_cascade = name.clone();
        let cascade = tokio::task::spawn_blocking(move || -> Vec<String> {
            let Ok(sqlite) = crate::db::get_db_conn() else { return vec![name_for_cascade]; };
            crate::fingerprint::cascade_delete_order(&sqlite, &ws_path, &name_for_cascade)
        })
        .await
        .unwrap_or_else(|_| vec![name.clone()]);

        // 2. Determine each object's type (table vs view) so we emit precise
        //    DROP statements instead of blindly trying both.
        let conn = self.shared.app_state.conn.clone();
        let cascade_for_type = cascade.clone();
        let type_map: Vec<(String, bool, bool)> = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            cascade_for_type.iter().map(|n| {
                let is_tbl = guard.query_row(
                    "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name=?",
                    [n], |r| r.get::<_, i64>(0),
                ).unwrap_or(0) > 0;
                let is_vw = guard.query_row(
                    "SELECT count(*) FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?",
                    [n], |r| r.get::<_, i64>(0),
                ).unwrap_or(0) > 0;
                (n.clone(), is_tbl, is_vw)
            }).collect()
        })
        .await
        .unwrap_or_default();

        let has_downstream = cascade.len() > 1;
        let summary_pending = if has_downstream {
            format!("删除 {} 及其 {} 个下游依赖", name, cascade.len() - 1)
        } else {
            format!("删除对象 {}", name)
        };

        self.shared.run(
            "drop_object", "drop",
            json!({ "name": name.clone() }),
            summary_pending,
            None,
            move || {
                let mut parts = Vec::new();
                for (n, is_tbl, is_vw) in &type_map {
                    if *is_vw { parts.push(format!("DROP VIEW IF EXISTS \"{}\";", n)); }
                    if *is_tbl { parts.push(format!("DROP TABLE IF EXISTS \"{}\";", n)); }
                }
                if parts.is_empty() {
                    format!("-- 「{}」 not found in lake catalog", name)
                } else {
                    parts.join("\n")
                }
            },
        )
        .await
    }
}

// --- render_chart ---------------------------------------------------------

#[derive(Deserialize, Serialize)]
struct RenderChartArgs {
    sql: String,
    chart_type: String,
    #[serde(default)]
    x_field: Option<String>,
    #[serde(default)]
    y_fields: Option<Vec<String>>,
    #[serde(default)]
    title: Option<String>,
}

struct RenderChartTool {
    app_state: AppState,
    task_id: String,
    window: tauri::Window,
}

impl Tool for RenderChartTool {
    const NAME: &'static str = "render_chart";
    type Error = ToolError;
    type Args = RenderChartArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "render_chart".to_string(),
            description: "用图表可视化查询结果。先写好 SELECT 语句（和 execute_query 一样），指定图表类型和轴映射。适合趋势（折线 line）、对比（柱状 bar）、占比（饼图 pie）、相关性（散点 scatter）。图表会展示在对话中，用户可切换图表类型。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "用于获取图表数据的 SELECT 查询语句" },
                    "chart_type": { "type": "string", "enum": ["bar", "line", "pie", "scatter", "funnel", "gauge"], "description": "图表类型：bar(柱状对比)、line(趋势)、pie(占比)、scatter(相关性)、funnel(转化漏斗)、gauge(单值指标)" },
                    "x_field": { "type": "string", "description": "X 轴/分类列名（饼图时为名称列）" },
                    "y_fields": { "type": "array", "items": { "type": "string" }, "description": "Y 轴/数值列名，支持多列（多系列）。饼图时取第一个" },
                    "title": { "type": "string", "description": "图表标题（可选）" }
                },
                "required": ["sql", "chart_type"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let sql = args.sql.trim();
        let sql_upper = sql.to_uppercase();
        let forbidden = ["DROP", "DELETE", "UPDATE", "INSERT", "ALTER", "TRUNCATE", "ATTACH", "DETACH"];
        for kw in &forbidden {
            if sql_upper.contains(kw) {
                return Err(ToolError(format!("出于安全考虑，禁止执行包含 {} 操作的 SQL 语句。", kw)));
            }
        }

        let valid_types = ["bar", "line", "pie", "scatter", "funnel", "gauge"];
        if !valid_types.contains(&args.chart_type.as_str()) {
            return Err(ToolError(format!(
                "不支持的图表类型「{}」，可选：bar / line / pie / scatter / funnel / gauge", args.chart_type
            )));
        }

        let call_id = next_tool_id("chart");
        emit_tool_call(&self.window, &self.task_id, &call_id, "render_chart", json!({
            "sql": sql,
            "chart_type": args.chart_type,
            "x_field": args.x_field,
            "y_fields": args.y_fields,
        }));

        let start = std::time::Instant::now();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let sql_string = sql.to_string();
        let hard_secs = get_query_hard_timeout();
        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<SqlResult, ToolError> {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(200)).map_err(|e| ToolError(e.to_string()))
        });
        let res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r.map_err(|e| ToolError(format!("线程生成失败: {e}")))?,
                Err(_) => {
                    ih.interrupt();
                    return Err(ToolError(format!("图表查询已达到最大等待时间（{} 秒）被强制终止", hard_secs)));
                }
            }
        } else {
            blocking_fut.await.map_err(|e| ToolError(format!("线程生成失败: {e}")))?
        };

        let elapsed = start.elapsed().as_millis() as u64;
        match res {
            Ok(table) => {
                let row_count = table.row_count;
                // Emit the chart segment — frontend renders it inline.
                emit_chart(
                    &self.window, &self.task_id, &call_id,
                    &args.chart_type,
                    args.title.as_deref(),
                    args.x_field.as_deref(),
                    args.y_fields.as_deref(),
                    table,
                );
                let summary = format!("已生成{}图，共 {} 个数据点", chart_type_cn(&args.chart_type), row_count);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "ok",
                    summary.clone(), None, None, Some(elapsed), None,
                );
                Ok(summary)
            }
            Err(err) => {
                let msg = format!("查询失败: {}", err.0);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    msg.clone(), None, None, Some(elapsed), None,
                );
                Err(err)
            }
        }
    }
}

/// Chinese label for a chart type (used in the tool result summary).
fn chart_type_cn(t: &str) -> &str {
    match t {
        "bar" => "柱状",
        "line" => "折线",
        "pie" => "饼",
        "scatter" => "散点",
        "funnel" => "漏斗",
        "gauge" => "仪表盘",
        _ => "图表",
    }
}

/// Emit a usage *estimate* event — sent before/during streaming, before the
/// API's exact FinalResponse usage arrives. `prompt_tokens_est` is our
/// `k = 1` estimate of the upcoming call's prompt; `output_tokens_est` grows
/// as text streams in (0 on the initial pre-stream estimate).
///
/// The raw (uncalibrated) preamble/tools estimates are always attached so the
/// frontend can render the composition breakdown even mid-stream. Cache fields
/// are intentionally omitted: they are unknown until FinalResponse, and the
/// frontend freezes the last real cache values instead of showing a fake 0.
fn emit_usage_estimate(
    window: &tauri::Window,
    task_id: &str,
    prompt_tokens_est: u64,
    output_tokens_est: u64,
    preamble_raw: u64,
    tools_raw: u64,
) {
    let _ = window.emit("agent-event", AgentStreamEvent {
        task_id: task_id.to_string(),
        kind: "usage".to_string(),
        text: Some(serde_json::to_string(&serde_json::json!({
            "isEstimate": true,
            "promptTokens": prompt_tokens_est,
            "completionTokens": output_tokens_est,
            "estPreambleRaw": preamble_raw,
            "estToolsRaw": tools_raw,
        })).unwrap_or_default()),
        segment: None,
    });
}

/// Emit a *real* usage event from a FinalResponse (one per LLM call within the
/// multi-turn run). [`usage::normalize`] collapses provider-specific fields
/// into one honest shape; `k_sample` is attached only for the **first** call of
/// each run (the only call whose prompt we can locally estimate — subsequent
/// calls include rig-internal tool results we don't tokenize) so the frontend
/// can refit its per-model calibration factor.
///
/// `run_completion_tokens` is the cumulative real output across all calls in
/// this run so far (prior + this one); the frontend displays it as "本轮输出"
/// so the value never drops between calls of a multi-turn (tool-using) run.
fn emit_usage_real(
    window: &tauri::Window,
    task_id: &str,
    n: usage::NormalizedUsage,
    k_sample: Option<f64>,
    run_completion_tokens: u64,
    preamble_raw: u64,
    tools_raw: u64,
) {
    let mut payload = serde_json::json!({
        "isEstimate": false,
        "promptTokens": n.prompt_tokens,
        "completionTokens": n.completion_tokens,
        "runCompletionTokens": run_completion_tokens,
        "cacheReadTokens": n.cache_read_tokens,
        "cacheCreationTokens": n.cache_creation_tokens,
        "freshInputTokens": n.fresh_input_tokens,
        "estPreambleRaw": preamble_raw,
        "estToolsRaw": tools_raw,
    });
    if let Some(k) = k_sample {
        payload["kSample"] = serde_json::json!(k);
    }
    let _ = window.emit("agent-event", AgentStreamEvent {
        task_id: task_id.to_string(),
        kind: "usage".to_string(),
        text: Some(serde_json::to_string(&payload).unwrap_or_default()),
        segment: None,
    });
}

/// Emit a run *summary* at the end of one agent run (one user turn, possibly
/// many LLM calls). The frontend uses this to increment the turn counter and
/// record the final generation speed (tok/s = total output / wall-clock).
fn emit_usage_run_summary(
    window: &tauri::Window,
    task_id: &str,
    run_output_tokens: u64,
    run_elapsed_ms: u64,
) {
    let tok_per_sec = if run_elapsed_ms > 0 {
        let secs = (run_elapsed_ms as f64) / 1000.0;
        (run_output_tokens as f64 / secs.max(0.001)).round() as u64
    } else {
        0
    };
    let _ = window.emit("agent-event", AgentStreamEvent {
        task_id: task_id.to_string(),
        kind: "usage".to_string(),
        text: Some(serde_json::to_string(&serde_json::json!({
            "isEstimate": false,
            "turnComplete": true,
            "runOutputTokens": run_output_tokens,
            "runElapsedMs": run_elapsed_ms,
            "tokPerSec": tok_per_sec,
        })).unwrap_or_default()),
        segment: None,
    });
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

/// The system prompt lives in `usage::PREAMBLE` so the token estimator can
/// tokenize the exact text the model receives (kept faithful to the real call).
/// Rebuild the LLM chat history from persisted messages.
///
/// Legacy messages carry a flat `content` string; new messages carry `segments`.
/// Only visible text reaches the model — reasoning and tool steps are managed
/// by rig within the turn and are not replayed as history (matches prior
/// behavior, which only ever sent `content`).
fn get_message_text(msg: &ChatMessageDto) -> String {
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
    state: &AppState,
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, StreamingError>> + Unpin,
    input_tokens_est: u64,
    api_format: &str,
    preamble_raw: u64,
    tools_raw: u64,
) {
    use futures_util::StreamExt;
    // Wall-clock start of this run (one user turn, possibly many LLM calls) —
    // used at run end to compute the generation speed (tok/s).
    let run_start = std::time::Instant::now();
    // Accumulated completion tokens across every FinalResponse in this run, for
    // the final tok/s = total_output / run_elapsed.
    let mut run_output_tokens: u64 = 0;
    // The first FinalResponse of a run is the only call whose prompt we can
    // locally estimate (subsequent calls include rig-internal tool results we
    // don't tokenize), so only it contributes a calibration sample.
    let mut first_final = true;
    // Accumulated model output (reasoning + visible text) — fed to the
    // char-aware estimator for a live output-token estimate during streaming.
    // Reasoning is included because the API's `output_tokens` counts it too,
    // so the live tok/s and "本轮输出" stay consistent with the final real
    // usage that arrives at FinalResponse.
    let mut output_buf = String::new();
    // Check the abort flag before processing each chunk. If set, stop early and
    // emit a "done" so the frontend unlocks the input.
    {
        let aborted = state.aborted_tasks.lock().await;
        if aborted.contains(&task_id) {
            drop(aborted);
            state.aborted_tasks.lock().await.remove(&task_id);
            emit_event(&window, &task_id, "done", None, None);
            return;
        }
    }
    while let Some(chunk) = stream.next().await {
        // Check abort mid-stream too.
        {
            let aborted = state.aborted_tasks.lock().await;
            if aborted.contains(&task_id) {
                drop(aborted);
                state.aborted_tasks.lock().await.remove(&task_id);
                emit_event(&window, &task_id, "done", None, None);
                return;
            }
        }
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                output_buf.push_str(&text_struct.text);
                emit_delta(&window, &task_id, "text", &text_struct.text);
                // Live output estimate = prior calls' real completion + this
                // call's streaming estimate (reasoning + text). Cumulative for
                // the whole run so the bar never drops between calls.
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                // Reasoning counts toward output tokens (the API bills it as
                // output), so feed it into the same accumulator as text.
                output_buf.push_str(&reasoning);
                emit_delta(&window, &task_id, "reasoning", &reasoning);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                let t = reasoning_struct.display_text();
                output_buf.push_str(&t);
                emit_delta(&window, &task_id, "reasoning", &t);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            // FinalResponse: carries the API's exact per-call token usage.
            Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
                let rig_usage = final_resp.usage();
                // Collapse provider-specific fields into one honest shape
                // (Anthropic's input_tokens excludes cache, OpenAI's includes
                // it). This makes the cache-hit rate ≤ 100 % across providers.
                let n = usage::normalize(
                    rig_usage.input_tokens,
                    rig_usage.output_tokens,
                    rig_usage.cached_input_tokens,
                    rig_usage.cache_creation_input_tokens,
                    api_format,
                );
                run_output_tokens += n.completion_tokens;
                // Only the first call of the run has a locally-estimable
                // prompt (= preamble + tools + prompt + history, computed as
                // `input_tokens_est` before the stream). Its real/estimated
                // ratio refits the per-model calibration factor `k` in the
                // frontend so future estimates converge toward reality.
                let k_sample = if first_final && input_tokens_est > 0 {
                    first_final = false;
                    Some(n.prompt_tokens as f64 / input_tokens_est as f64)
                } else {
                    None
                };
                // `run_completion_tokens` is the cumulative real output for
                // the whole run so far (prior calls + this one); the frontend
                // shows it as "本轮输出" so it never drops between calls.
                emit_usage_real(&window, &task_id, n, k_sample, run_output_tokens, preamble_raw, tools_raw);
                // Reset the streaming accumulator so the next call's live
                // estimate starts from this call's text only (added on top of
                // the real `run_output_tokens`).
                output_buf.clear();
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

    // Normal run completion (no abort/error): emit a run summary so the
    // frontend can count this as a finished turn and show the generation
    // speed (tok/s = total output / wall-clock). Aborted/errored runs return
    // early above and intentionally do not count as a completed turn.
    emit_usage_run_summary(
        &window,
        &task_id,
        run_output_tokens,
        run_start.elapsed().as_millis() as u64,
    );
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
        let text = get_message_text(&msg);
        if !text.is_empty() {
            if msg.role == "user" {
                rig_history.push(Message::user(text));
            } else if msg.role == "assistant" {
                rig_history.push(Message::assistant(text));
            }
        }
    }

    // The pre-stream usage estimate is emitted AFTER the tool instances are
    // created below, so the tool-definition token cost uses rig's real
    // `ToolDefinition`s (not a hardcoded approximation).

    let list_tool = ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let desc_tool = DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let exec_tool = ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let sample_tool = SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    
    // OKF Tools (Instantiated for each format branch due to rig's ownership rules)
    let load_okf_1 = LoadOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let load_okf_2 = LoadOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let load_okf_3 = LoadOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    
    let write_okf_1 = WriteOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let write_okf_2 = WriteOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let write_okf_3 = WriteOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    
    let search_okf_1 = SearchOkfRecipesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let search_okf_2 = SearchOkfRecipesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let search_okf_3 = SearchOkfRecipesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    
    let check_okf_1 = CheckSourceFingerprintTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let check_okf_2 = CheckSourceFingerprintTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let check_okf_3 = CheckSourceFingerprintTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };

    let tidy_okf_1 = TidyOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), model_id: model_id.clone() };
    let tidy_okf_2 = TidyOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), model_id: model_id.clone() };
    let tidy_okf_3 = TidyOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), model_id: model_id.clone() };

    let materialize_tool_1 = MaterializeRemoteTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let materialize_tool_2 = MaterializeRemoteTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let materialize_tool_3 = MaterializeRemoteTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };

    let ddl_shared = DdlToolShared {
        app_state: app_state.clone(),
        task_id: task_id.clone(),
        window: window.clone(),
        confirm_mode: confirm_mode.clone(),
    };
    let create_table_tool = CreateTableTool { shared: ddl_shared.clone() };
    let create_view_tool = CreateViewTool { shared: ddl_shared.clone() };
    let drop_object_tool = DropObjectTool { shared: ddl_shared };
    let render_chart_tool = RenderChartTool {
        app_state: app_state.clone(),
        task_id: task_id.clone(),
        window: window.clone(),
    };
    let render_chart_tool_2 = RenderChartTool {
        app_state: app_state.clone(),
        task_id: task_id.clone(),
        window: window.clone(),
    };
    let render_chart_tool_3 = RenderChartTool {
        app_state: app_state.clone(),
        task_id: task_id.clone(),
        window: window.clone(),
    };

    // Estimate the input token cost before the stream starts so the UI panel
    // shows data immediately (not only after the first response). This is a
    // rough `k = 1` estimate (preamble + tools + prompt + history); the exact
    // value from the API replaces it when FinalResponse arrives, and the
    // first-call real/estimated ratio refits the calibration factor `k`.
    //
    // The tool cost is estimated from rig's *actual* `ToolDefinition`s
    // (name + full description + JSON-Schema parameters), serialized to JSON —
    // not a minimal hardcoded approximation — so the "系统工具" slice reflects
    // what the model really receives. (`render_chart_tool_2`/`_3` are identical
    // duplicates for the other provider branches, so one definition suffices.)
    let tool_defs = vec![
        list_tool.definition(String::new()).await,
        desc_tool.definition(String::new()).await,
        exec_tool.definition(String::new()).await,
        sample_tool.definition(String::new()).await,
        create_table_tool.definition(String::new()).await,
        create_view_tool.definition(String::new()).await,
        drop_object_tool.definition(String::new()).await,
        render_chart_tool.definition(String::new()).await,
        load_okf_1.definition(String::new()).await,
        write_okf_1.definition(String::new()).await,
        search_okf_1.definition(String::new()).await,
        check_okf_1.definition(String::new()).await,
        tidy_okf_1.definition(String::new()).await,
        materialize_tool_1.definition(String::new()).await,
    ];
    let tools_json = serde_json::to_string(&tool_defs).unwrap_or_default();
    let ws_dir = app_state.workspace_dir.lock().await.to_string_lossy().to_string();
    let memory_summary = crate::okf::get_okf_memory_summary(&ws_dir);
    let combined_preamble = if memory_summary.is_empty() {
        PREAMBLE.to_string()
    } else {
        format!("{}\n\n# 你的湖仓数据及业务“记忆”\n根据你之前与用户的对话和本地 OKF 知识库的积累，你已拥有以下数据与业务概念记忆。你在进行数据关联分析、回答提问时应**直接继承并使用**这些知识（包括业务释义与表关系），无需重复向用户澄清：\n\n{}", PREAMBLE, memory_summary)
    };
    let preamble_raw = usage::estimate_tokens(&combined_preamble);
    let tools_raw = usage::estimate_tokens(&tools_json);
    let prompt_t = usage::estimate_tokens(&prompt);
    let history_t: u64 = rig_history.iter()
        .map(|m| usage::estimate_tokens(&format!("{:?}", m)))
        .sum();
    let input_est = preamble_raw + tools_raw + prompt_t + history_t;
    emit_usage_estimate(&window, &task_id, input_est, 0, preamble_raw, tools_raw);

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
            .preamble(&combined_preamble)
            .max_tokens(max_tokens_limit)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool)
            .tool(render_chart_tool)
            .tool(load_okf_1)
            .tool(write_okf_1)
            .tool(search_okf_1)
            .tool(check_okf_1)
            .tool(tidy_okf_1)
            .tool(materialize_tool_1);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
            .await;
        run_stream_loop(
            window.clone(),
            task_id.clone(),
            &app_state,
            stream,
            input_est,
            &provider.api_format,
            preamble_raw,
            tools_raw,
        )
        .await;
    } else if format == "responses" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;

        let mut agent_builder = client
            .agent(&model_id)
            .preamble(&combined_preamble)
            .max_tokens(max_tokens_limit)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool)
            .tool(render_chart_tool_2)
            .tool(load_okf_2)
            .tool(write_okf_2)
            .tool(search_okf_2)
            .tool(check_okf_2)
            .tool(tidy_okf_2)
            .tool(materialize_tool_2);

        if model_id.starts_with("o1") || model_id.starts_with("o3") {
            agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
        }

        let agent = agent_builder.build();
        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
            .await;
        run_stream_loop(
            window.clone(),
            task_id.clone(),
            &app_state,
            stream,
            input_est,
            &provider.api_format,
            preamble_raw,
            tools_raw,
        )
        .await;
    } else if format == "anthropic" {
        let base_url = sanitize_endpoint(&provider.endpoint);
        let client: rig_core::providers::anthropic::Client = rig_core::providers::anthropic::Client::builder()
            .api_key(provider.api_key.clone())
            .base_url(&base_url)
            .build()
            .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;

        let agent = client
            .agent(&model_id)
            .preamble(&combined_preamble)
            .max_tokens(4096)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .tool(create_table_tool)
            .tool(create_view_tool)
            .tool(drop_object_tool)
            .tool(render_chart_tool_3)
            .tool(load_okf_3)
            .tool(write_okf_3)
            .tool(search_okf_3)
            .tool(check_okf_3)
            .tool(tidy_okf_3)
            .tool(materialize_tool_3)
            .build();

        let stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(100)
            .await;
        run_stream_loop(
            window.clone(),
            task_id.clone(),
            &app_state,
            stream,
            input_est,
            &provider.api_format,
            preamble_raw,
            tools_raw,
        )
        .await;
    } else {
        return Err(format!("不支持的 API 格式: {}", provider.api_format));
    }

    // Emit done event
    emit_event(&window, &task_id, "done", None, None);

    Ok(())
}
