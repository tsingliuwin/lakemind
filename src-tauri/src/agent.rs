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

// Define ToolError
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError(pub String);
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ToolError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStreamEvent {
    pub task_id: String,
    pub kind: String, // "reasoning" | "text" | "card" | "done" | "error"
    pub text: Option<String>,
    pub card: Option<AgentChatCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatCard {
    pub id: String,
    pub kind: String, // "step" | "sql" | "table" | "conclusion"
    pub title: String,
    pub detail: Option<String>,
    pub sql: Option<String>,
    pub rows: Option<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ChatMessageDto {
    pub id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
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

fn emit_card(window: &tauri::Window, task_id: &str, card: AgentChatCard) {
    let _ = window.emit(
        "agent-event",
        AgentStreamEvent {
            task_id: task_id.to_string(),
            kind: "card".to_string(),
            text: None,
            card: Some(card),
        },
    );
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
        let card_id = format!("step-list-tables-{}", now_ms());
        emit_card(&self.window, &self.task_id, AgentChatCard {
            id: card_id.clone(),
            kind: "step".to_string(),
            title: "探索数据库结构".to_string(),
            detail: Some("正在扫描数据表...".to_string()),
            sql: None,
            rows: None,
        });

        let sql = "
            SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
            UNION
            SELECT table_name FROM duckdb_views() WHERE database_name = 'lake' AND schema_name = 'main' AND NOT internal
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

        match tables_res {
            Ok(tables) => {
                let detail = if tables.is_empty() {
                    "数据库中目前没有找到任何表。".to_string()
                } else {
                    format!("探测了 {} 张表: {}", tables.len(), tables.join(", "))
                };

                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: "探索数据库结构".to_string(),
                    detail: Some(detail.clone()),
                    sql: None,
                    rows: None,
                });

                Ok(format!("当前可用的数据库表列表为: {}", tables.join(", ")))
            }
            Err(err) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: "探索数据库结构失败".to_string(),
                    detail: Some(err.0.clone()),
                    sql: None,
                    rows: None,
                });
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

        let card_id = format!("step-desc-{}", now_ms());
        emit_card(&self.window, &self.task_id, AgentChatCard {
            id: card_id.clone(),
            kind: "step".to_string(),
            title: format!("获取数据表 {} 结构", table_name),
            detail: Some("正在分析列信息...".to_string()),
            sql: None,
            rows: None,
        });

        let conn = self.app_state.conn.clone();
        let table_name_string = table_name.to_string();
        let desc_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let sql = format!("DESCRIBE {table_name_string}");
            let mut stmt = guard.prepare(&sql)?;
            let mut columns = Vec::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let name: String = row.get(0)?;
                let ty: String = row.get(1)?;
                let null: String = row.get(2)?;
                columns.push(format!("{} (类型: {}, 允许空: {})", name, ty, null));
            }
            Ok::<_, duckdb::Error>(columns)
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("执行 DESCRIBE 失败: {e}"))));

        match desc_res {
            Ok(cols) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: format!("获取数据表 {} 结构", table_name),
                    detail: Some(format!("结构分析完成，共 {} 个字段", cols.len())),
                    sql: None,
                    rows: None,
                });
                Ok(format!("表 {} 的列结构如下:\n{}", table_name, cols.join("\n")))
            }
            Err(err) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: format!("获取数据表 {} 结构失败", table_name),
                    detail: Some(err.0.clone()),
                    sql: None,
                    rows: None,
                });
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

        let card_id = format!("sql-query-{}", now_ms());
        
        let conn = self.app_state.conn.clone();
        let sql_string = sql.to_string();
        
        let query_res = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            execute::run_query(&guard, &sql_string, Some(50))
        })
        .await
        .map_err(|e| ToolError(format!("线程生成失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("SQL 执行出错: {e}"))));

        match query_res {
            Ok(res) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "sql".to_string(),
                    title: "执行 SQL 查询".to_string(),
                    detail: Some(sql.to_string()),
                    sql: Some(sql.to_string()),
                    rows: Some(res.rows.len()),
                });

                let mut out = String::new();
                out.push_str(&format!("查询成功，返回 {} 行。列: {}\n", res.rows.len(), res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                if res.truncated {
                    out.push_str("(结果已截断，仅返回前 50 行)\n");
                }
                Ok(out)
            }
            Err(err) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: "执行 SQL 查询失败".to_string(),
                    detail: Some(format!("SQL: {}\n错误: {}", sql, err.0)),
                    sql: None,
                    rows: None,
                });
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

        let card_id = format!("step-sample-{}", now_ms());
        emit_card(&self.window, &self.task_id, AgentChatCard {
            id: card_id.clone(),
            kind: "step".to_string(),
            title: format!("采集数据表 {} 样例", table_name),
            detail: Some("正在获取前 5 行样例数据...".to_string()),
            sql: None,
            rows: None,
        });

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

        match query_res {
            Ok(res) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: format!("采集数据表 {} 样例", table_name),
                    detail: Some(format!("完成采样，获取到 {} 行样例数据", res.rows.len())),
                    sql: None,
                    rows: None,
                });

                let mut out = String::new();
                out.push_str(&format!("表 {} 的前 {} 行样例数据如下. 列: {}\n", table_name, res.rows.len(), res.columns.join(", ")));
                for (i, row) in res.rows.iter().enumerate() {
                    let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    out.push_str(&format!("行 #{}: {}\n", i + 1, row_str.join(" | ")));
                }
                Ok(out)
            }
            Err(err) => {
                emit_card(&self.window, &self.task_id, AgentChatCard {
                    id: card_id,
                    kind: "step".to_string(),
                    title: format!("采集数据表 {} 样例失败", table_name),
                    detail: Some(err.0.clone()),
                    sql: None,
                    rows: None,
                });
                Err(err)
            }
        }
    }
}

// ===========================================================================
// Core Streaming Runner
// ===========================================================================

pub async fn run_agent_chat_stream(
    window: tauri::Window,
    task_id: String,
    model_id: String,
    prompt: String,
    history_json: String,
    app_state: AppState,
) -> Result<(), String> {
    // 1. Get model provider config
    let provider = get_provider_for_model(&model_id)?;

    // Get max_tokens limit for the chosen model, defaulting to 4096 if not set
    let max_tokens_limit = provider.models.iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.max_tokens)
        .unwrap_or(4096) as u64;

    // 2. Parse chat history
    let history: Vec<ChatMessageDto> = serde_json::from_str(&history_json)
        .map_err(|e| format!("解析聊天历史失败: {e}"))?;

    let mut rig_history: Vec<Message> = Vec::new();
    for msg in history {
        if msg.role == "user" {
            rig_history.push(Message::user(msg.content));
        } else if msg.role == "assistant" {
            rig_history.push(Message::assistant(msg.content));
        }
    }

    let list_tool = ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let desc_tool = DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let exec_tool = ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };
    let sample_tool = SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() };

    let preamble = "你是一个本地智能数据湖分析助手。你拥有访问本地数据库表的工具。在开始回答问题前，你应该先调用 list_tables 工具了解有哪些可用的表，然后再通过 describe_table 或 sample_data 工具了解表字段详情，最后编写 SELECT 语句执行 execute_query 获取数据结论。禁止执行任何写操作（DELETE, DROP, UPDATE 等）。所有数据都必须在本地的 SQL 结果中获取。若需要，可以自行关联表查询。你的结论必须以中文展示，且务必严谨地基于数据。";

    let format = provider.api_format.to_lowercase();
    if format == "openai" || format == "responses" {
        let client: rig_core::providers::openai::Client = rig_core::providers::openai::Client::builder()
            .api_key(&provider.api_key)
            .base_url(&provider.endpoint)
            .build()
            .map_err(|e| format!("构建 OpenAI 客户端失败: {e}"))?;

        let agent = client
            .agent(&model_id)
            .preamble(preamble)
            .max_tokens(max_tokens_limit)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .build();

        let mut stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(8)
            .await;

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "text".to_string(),
                        text: Some(text_struct.text),
                        card: None,
                    });
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "reasoning".to_string(),
                        text: Some(reasoning),
                        card: None,
                    });
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "reasoning".to_string(),
                        text: Some(reasoning_struct.display_text().to_string()),
                        card: None,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    let err_val: StreamingError = e;
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "error".to_string(),
                        text: Some(err_val.to_string()),
                        card: None,
                    });
                    return Err(err_val.to_string());
                }
            }
        }
    } else if format == "anthropic" {
        let client: rig_core::providers::anthropic::Client = rig_core::providers::anthropic::Client::builder()
            .api_key(provider.api_key.clone())
            .base_url(&provider.endpoint)
            .build()
            .map_err(|e| format!("构建 Anthropic 客户端失败: {e}"))?;

        let agent = client
            .agent(&model_id)
            .preamble(preamble)
            .max_tokens(4096)
            .tool(list_tool)
            .tool(desc_tool)
            .tool(exec_tool)
            .tool(sample_tool)
            .build();

        let mut stream = agent.stream_chat(prompt.clone(), rig_history)
            .multi_turn(8)
            .await;

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "text".to_string(),
                        text: Some(text_struct.text),
                        card: None,
                    });
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "reasoning".to_string(),
                        text: Some(reasoning),
                        card: None,
                    });
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "reasoning".to_string(),
                        text: Some(reasoning_struct.display_text().to_string()),
                        card: None,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    let err_val: StreamingError = e;
                    let _ = window.emit("agent-event", AgentStreamEvent {
                        task_id: task_id.clone(),
                        kind: "error".to_string(),
                        text: Some(err_val.to_string()),
                        card: None,
                    });
                    return Err(err_val.to_string());
                }
            }
        }
    } else {
        return Err(format!("不支持的 API 格式: {}", provider.api_format));
    }

    // Emit done event
    let _ = window.emit("agent-event", AgentStreamEvent {
        task_id: task_id.clone(),
        kind: "done".to_string(),
        text: None,
        card: None,
    });

    Ok(())
}
