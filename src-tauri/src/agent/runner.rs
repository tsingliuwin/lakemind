//! Core streaming runner: drive the rig multi-turn stream, map its items to
//! frontend events, and assemble the full agent (all tools + provider branches).

use serde_json::json;
use rig_core::{
    client::CompletionClient,
    completion::Message,
    streaming::{StreamingChat, StreamedAssistantContent},
    tool::Tool,
    agent::{MultiTurnStreamItem, StreamingError},
};

use super::config::{get_provider_for_model, sanitize_endpoint};
use super::events::{
    emit_delta, emit_event, emit_usage_estimate, emit_usage_real, emit_usage_run_summary,
};
use super::tools::*;
use super::wire::{ChatMessageDto, Segment};
use crate::state::AppState;
use crate::usage::{self, PREAMBLE};

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

pub(crate) async fn run_agent_chat_stream(
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
