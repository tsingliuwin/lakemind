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
/// Outcome of one streaming run. `RateLimited` is returned only when a 429 /
/// rate-limit error arrives *before* any content was emitted to the frontend,
/// so the caller can rebuild the stream and retry with backoff. Once content
/// has been emitted we can no longer safely retry (the multi-turn state has
/// advanced), so any later error is terminal (reported via the error event
/// inside the loop and returns `Done`).
enum RunOutcome {
    Done,
    /// Carries the last rate-limit error string so that when retries are
    /// exhausted the caller can surface *why* (the preflight path below
    /// intentionally skips emitting an error so it can retry — without this
    /// payload the final failure would be silent).
    RateLimited(String),
}

/// Classify a provider error string into one of three buckets so the runner
/// knows whether retrying is worthwhile.
///
/// rig's `http_client` formats every non-2xx response uniformly as
/// `"Invalid status code {StatusCode} with message: {body}"`. The status code
/// is a stable number, but providers overload 429 to mean two very different
/// things:
///   - **Transient throttling** (TPM/RPM exceeded, too_many_requests) — wait a
///     few seconds and the request will succeed. RETRY.
///   - **Quota/balance exhausted** (insufficient_quota, credit_balance_too_low,
///     余额不足) — no amount of waiting helps; the account is out of money.
///     DON'T RETRY (just surface the error so the user can top up).
///
/// Both return 429, so the status code alone can't tell them apart. We look at
/// the body wording: quota/balance/credit keywords → `Unretriable` (checked
/// first), otherwise a 429 or throttle keyword → `Retriable`.
#[derive(Debug, PartialEq, Eq)]
enum RateLimitKind {
    /// Transient rate limit — retrying after a backoff is worthwhile.
    Retriable,
    /// Quota/balance exhausted — retrying won't help; surface to the user.
    Unretriable,
    /// Not a rate-limit error at all.
    No,
}

fn classify_rate_limit_error(msg: &str) -> RateLimitKind {
    let m = msg.to_lowercase();

    // --- Unretriable: account out of quota / balance / credit. These may still
    // carry 429 (providers are inconsistent), so check BEFORE the 429 rule.
    // (Matching is conservative: "quota" alone could be a transient RPM quota,
    //  so we require a stronger signal — "insufficient", "exhausted", "balance",
    //  "credit", "billing", "payment", "充值", "余额", "用尽".)
    const UNRETRIABLE: &[&str] = &[
        "insufficient_quota", "insufficient quota", "insufficient balance",
        "quota exhausted", "quota_exhausted", "exceeded your current quota",
        "credit_balance", "credit balance", "balance is too low", "balance too low",
        "billing", "payment", "no credit", "out of credit",
        "额度已用尽", "额度不足", "余额不足", "余额已尽", "余额耗尽",
        "充值", "欠费", "计费",
    ];
    if UNRETRIABLE.iter().any(|k| m.contains(k)) {
        return RateLimitKind::Unretriable;
    }

    // --- Retriable: transient throttling. Primary signal is the HTTP 429
    // status code (rig formats as "Invalid status code 429 ..."), stable across
    // all providers. The body wording ("TPM超过限制" / "rate_limit_exceeded")
    // is provider-specific but we don't need to parse it — 429 + not-unretriable
    // (checked above) is enough to justify a retry.
    if m.contains("status code 429") {
        return RateLimitKind::Retriable;
    }

    // Fallback: non-standard gateways that express throttling via 503 or 200
    // with a rate-limit body (no 429). Match provider throttle wording.
    if m.contains("too many requests")
        || m.contains("rate_limit")
        || m.contains("ratelimit")
        || m.contains("overloaded")
        || m.contains("throttl")
        || m.contains("tpm")
        || m.contains("rpm")
    {
        return RateLimitKind::Retriable;
    }

    RateLimitKind::No
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
) -> RunOutcome {
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
    // Tracks whether any content (text/reasoning/tool/usage) has been emitted
    // to the frontend this run. A 429 that arrives before anything is emitted
    // is safe to retry (the multi-turn state hasn't advanced); one that arrives
    // mid-stream is not (rig's internal tool-result state has moved on).
    let mut emitted_any = false;
    // Check the abort flag before processing each chunk. If set, stop early and
    // emit a "done" so the frontend unlocks the input.
    {
        let aborted = state.aborted_tasks.lock().await;
        if aborted.contains(&task_id) {
            drop(aborted);
            state.aborted_tasks.lock().await.remove(&task_id);
            emit_event(&window, &task_id, "done", None, None);
            return RunOutcome::Done;
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
                return RunOutcome::Done;
            }
        }
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text_struct))) => {
                emitted_any = true;
                output_buf.push_str(&text_struct.text);
                emit_delta(&window, &task_id, "text", &text_struct.text);
                // Live output estimate = prior calls' real completion + this
                // call's streaming estimate (reasoning + text). Cumulative for
                // the whole run so the bar never drops between calls.
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta { reasoning, .. })) => {
                emitted_any = true;
                // Reasoning counts toward output tokens (the API bills it as
                // output), so feed it into the same accumulator as text.
                output_buf.push_str(&reasoning);
                emit_delta(&window, &task_id, "reasoning", &reasoning);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(reasoning_struct))) => {
                emitted_any = true;
                let t = reasoning_struct.display_text();
                output_buf.push_str(&t);
                emit_delta(&window, &task_id, "reasoning", &t);
                let completion_est = run_output_tokens + usage::estimate_tokens(&output_buf);
                emit_usage_estimate(&window, &task_id, input_tokens_est, completion_est, preamble_raw, tools_raw);
            }
            // FinalResponse: carries the API's exact per-call token usage.
            Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
                emitted_any = true;
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
            Ok(_) => { emitted_any = true; }
            Err(e) => {
                let msg = e.to_string();
                // Classify the error: a *transient* rate-limit (TPM/RPM) that
                // arrived BEFORE any content was emitted is retriable — the
                // caller waits and rebuilds the stream. A *quota/balance*
                // exhaustion is not retriable (waiting won't add credits), so
                // surface it as a normal error. If content already streamed out,
                // the multi-turn state has advanced and we can't safely retry
                // regardless.
                if !emitted_any && classify_rate_limit_error(&msg) == RateLimitKind::Retriable {
                    return RunOutcome::RateLimited(msg.clone());
                }
                emit_event(&window, &task_id, "error", Some(msg.clone()), None);
                return RunOutcome::Done;
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
    RunOutcome::Done
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

    // Factory closure that builds a fresh set of the 14 tool instances. Called
    // once per provider branch AND once per 429-retry (rig consumes the tools
    // when building the agent, so each retry needs a fresh set).
    let build_tools = || -> (ListTablesTool, DescribeTableTool, ExecuteQueryTool, SampleDataTool,
        LoadOkfBlockTool, WriteOkfBlockTool, SearchOkfRecipesTool, CheckSourceFingerprintTool,
        TidyOkfKnowledgeTool, MaterializeRemoteTableTool,
        CreateTableTool, CreateViewTool, DropObjectTool, RenderChartTool) {
        let ddl_shared = DdlToolShared {
            app_state: app_state.clone(),
            task_id: task_id.clone(),
            window: window.clone(),
            confirm_mode: confirm_mode.clone(),
        };
        (
            ListTablesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            DescribeTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            ExecuteQueryTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            SampleDataTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            LoadOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            WriteOkfBlockTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            SearchOkfRecipesTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            CheckSourceFingerprintTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            TidyOkfKnowledgeTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone(), model_id: model_id.clone() },
            MaterializeRemoteTableTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
            CreateTableTool { shared: ddl_shared.clone() },
            CreateViewTool { shared: ddl_shared.clone() },
            DropObjectTool { shared: ddl_shared },
            RenderChartTool { app_state: app_state.clone(), task_id: task_id.clone(), window: window.clone() },
        )
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
    // what the model really receives.
    let (list_tool, desc_tool, exec_tool, sample_tool,
        load_okf, write_okf, search_okf, check_okf, tidy_okf, materialize_tool,
        create_table_tool, create_view_tool, drop_object_tool, render_chart_tool) = build_tools();
    let tool_defs = vec![
        list_tool.definition(String::new()).await,
        desc_tool.definition(String::new()).await,
        exec_tool.definition(String::new()).await,
        sample_tool.definition(String::new()).await,
        create_table_tool.definition(String::new()).await,
        create_view_tool.definition(String::new()).await,
        drop_object_tool.definition(String::new()).await,
        render_chart_tool.definition(String::new()).await,
        load_okf.definition(String::new()).await,
        write_okf.definition(String::new()).await,
        search_okf.definition(String::new()).await,
        check_okf.definition(String::new()).await,
        tidy_okf.definition(String::new()).await,
        materialize_tool.definition(String::new()).await,
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

    // Retry loop for rate-limit (429) errors. Up to MAX_RETRIES attempts with
    // exponential backoff. Only retries when the 429 arrives before any content
    // was streamed (preflight rate-limit); mid-stream 429s can't be safely
    // retried because rig's multi-turn state has already advanced.
    const MAX_RETRIES: usize = 4;
    const BASE_DELAY_SECS: u64 = 5;
    let format = provider.api_format.to_lowercase();
    let mut attempt: usize = 0;
    loop {
        attempt += 1;
        // Build a fresh tool set each attempt (rig consumes them on .build()).
        let (list_tool, desc_tool, exec_tool, sample_tool,
            load_okf, write_okf, search_okf, check_okf, tidy_okf, materialize_tool,
            create_table_tool, create_view_tool, drop_object_tool, render_chart_tool) = build_tools();

        let outcome = if format == "openai" {
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
                .tool(load_okf)
                .tool(write_okf)
                .tool(search_okf)
                .tool(check_okf)
                .tool(tidy_okf)
                .tool(materialize_tool);
            if model_id.starts_with("o1") || model_id.starts_with("o3") {
                agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
            }
            let agent = agent_builder.build();
            let stream = agent.stream_chat(prompt.clone(), rig_history.clone())
                .multi_turn(100)
                .await;
            run_stream_loop(
                window.clone(), task_id.clone(), &app_state, stream,
                input_est, &provider.api_format, preamble_raw, tools_raw,
            ).await
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
                .tool(render_chart_tool)
                .tool(load_okf)
                .tool(write_okf)
                .tool(search_okf)
                .tool(check_okf)
                .tool(tidy_okf)
                .tool(materialize_tool);
            if model_id.starts_with("o1") || model_id.starts_with("o3") {
                agent_builder = agent_builder.additional_params(json!({"reasoning_effort": effort}));
            }
            let agent = agent_builder.build();
            let stream = agent.stream_chat(prompt.clone(), rig_history.clone())
                .multi_turn(100)
                .await;
            run_stream_loop(
                window.clone(), task_id.clone(), &app_state, stream,
                input_est, &provider.api_format, preamble_raw, tools_raw,
            ).await
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
                .tool(render_chart_tool)
                .tool(load_okf)
                .tool(write_okf)
                .tool(search_okf)
                .tool(check_okf)
                .tool(tidy_okf)
                .tool(materialize_tool)
                .build();
            let stream = agent.stream_chat(prompt.clone(), rig_history.clone())
                .multi_turn(100)
                .await;
            run_stream_loop(
                window.clone(), task_id.clone(), &app_state, stream,
                input_est, &provider.api_format, preamble_raw, tools_raw,
            ).await
        } else {
            return Err(format!("不支持的 API 格式: {}", provider.api_format));
        };

        match outcome {
            RunOutcome::Done => break,
            RunOutcome::RateLimited(_) if attempt <= MAX_RETRIES => {
                // Exponential backoff: 5s, 10s, 20s, 40s.
                let delay = BASE_DELAY_SECS * (1 << (attempt - 1));
                emit_event(
                    &window, &task_id, "text",
                    Some(format!("（遇到速率限制，{} 秒后自动重试…第 {}/{} 次）", delay, attempt, MAX_RETRIES)),
                    None,
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                continue;
            }
            // Retries exhausted on a preflight rate-limit. The preflight path
            // in run_stream_loop returns RateLimited WITHOUT emitting an error
            // event (so it could retry), so without this the run would end
            // silently after the countdown. Surface the final cause now.
            RunOutcome::RateLimited(last) => {
                emit_event(
                    &window, &task_id, "error",
                    Some(format!(
                        "已自动重试 {MAX_RETRIES} 次仍被速率限制（429），请稍候降低请求频率或更换模型后重试。\n{last}"
                    )),
                    None,
                );
                break;
            }
        }
    }

    // Emit done event
    emit_event(&window, &task_id, "done", None, None);

    Ok(())
}

#[cfg(test)]
mod tests_rate_limit_classify {
    use super::*;

    // ---- Retriable: transient throttling (should retry) ----

    #[test]
    fn longcat_tpm_429_is_retriable() {
        // The exact error the user hit.
        let msg = "CompletionError: ProviderError: Invalid status code 429 Too Many Requests with message: {\"error\":{\"message\":\"服务端模型:LongCat-2.0 总prefill TPM超过限制\",\"type\":\"rate_limit_error\",\"code\":\"too_many_requests\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }

    #[test]
    fn openai_rate_limit_429_is_retriable() {
        let msg = "Invalid status code 429 Too Many Requests with message: {\"error\":{\"type\":\"rate_limit_exceeded\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }

    #[test]
    fn anthropic_overloaded_is_retriable() {
        // Anthropic sometimes returns 503 + "overloaded".
        let msg = "Invalid status code 503 Service Unavailable with message: {\"error\":{\"type\":\"overloaded_error\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }

    // ---- Unretriable: quota / balance exhausted (must NOT retry) ----

    #[test]
    fn openai_insufficient_quota_is_unretriable() {
        // Even though it may carry 429, it's a billing issue — no retry.
        let msg = "Invalid status code 429 with message: {\"error\":{\"code\":\"insufficient_quota\",\"message\":\"You exceeded your current quota\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    #[test]
    fn anthropic_credit_balance_too_low_is_unretriable() {
        let msg = "Invalid status code 400 with message: {\"error\":{\"type\":\"credit_balance_too_low\",\"message\":\"Your credit balance is too low\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    #[test]
    fn deepseek_insufficient_balance_is_unretriable() {
        let msg = "Invalid status code 402 with message: Insufficient Balance";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    #[test]
    fn chinese_quota_exhausted_is_unretriable() {
        let msg = "Invalid status code 429 with message: 额度已用尽，请充值";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Unretriable);
    }

    // ---- Not a rate-limit error at all ----

    #[test]
    fn auth_error_is_not_rate_limit() {
        let msg = "Invalid status code 401 Unauthorized with message: Invalid API key";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::No);
    }

    #[test]
    fn server_error_is_not_rate_limit() {
        let msg = "Invalid status code 500 Internal Server Error";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::No);
    }

    // ---- Edge case: "quota" in a transient context should still be retriable ----
    // (e.g. "RPM quota exceeded" is transient, not a billing exhaustion.)
    #[test]
    fn rpm_quota_429_is_retriable_not_unretriable() {
        let msg = "Invalid status code 429 with message: RPM quota exceeded, too many requests";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }

    // The provider error the user actually hit: message "rpm exhausted" with
    // type "quota_exceeded_error" on a 429. Despite "quota" in the type name,
    // "rpm exhausted" is a per-minute (RPM) throttle that resets on its own, so
    // it is retriable — NOT a billing/quota depletion. Keep this retriable so
    // the runner's backoff can ride it out. (If a provider ever uses this exact
    // type for a *hard* quota, revisit — but the message wording is the signal
    // we trust here.)
    #[test]
    fn rpm_exhausted_quota_exceeded_429_is_retriable() {
        let msg = "CompletionError: ProviderError: Invalid status code 429 Too Many Requests with message: {\"error\":{\"message\":\"rpm exhausted\",\"type\":\"quota_exceeded_error\",\"code\":\"8\"}}";
        assert_eq!(classify_rate_limit_error(msg), RateLimitKind::Retriable);
    }
}
