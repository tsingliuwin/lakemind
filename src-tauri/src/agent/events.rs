//! Event-emission layer: pushes structured `AgentStreamEvent`s to the frontend
//! via `window.emit("agent-event", ...)`. All tools and the streaming runner go
//! through these helpers so the wire format stays consistent.

use tauri::Emitter;

use super::wire::{AgentStreamEvent, Segment};
use crate::model::SqlResult;
use crate::usage::{self};

pub(super) fn now_ms() -> i64 {
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

pub(super) fn next_tool_id(prefix: &str) -> String {
    let n = TOOL_CALL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    format!("tool-{prefix}-{}-{n}", now_ms())
}

pub(super) fn emit_event(window: &tauri::Window, task_id: &str, kind: &str, text: Option<String>, segment: Option<Segment>) {
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
pub(super) fn emit_delta(window: &tauri::Window, task_id: &str, kind: &str, text: &str) {
    emit_event(window, task_id, kind, Some(text.to_string()), None);
}

/// Emit a `tool_call` segment (status: running) — opens a new tool step in the
/// transcript.
pub(super) fn emit_tool_call(window: &tauri::Window, task_id: &str, id: &str, tool: &str, args: serde_json::Value) {
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
pub(super) fn emit_tool_result(
    window: &tauri::Window,
    task_id: &str,
    id: &str,
    status: &str, // "ok" | "error" | "running"
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
pub(super) fn emit_tool_awaiting(
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
pub(super) fn emit_chart(
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

/// Emit a usage *estimate* event — sent before/during streaming, before the
/// API's exact FinalResponse usage arrives. `prompt_tokens_est` is our
/// `k = 1` estimate of the upcoming call's prompt; `output_tokens_est` grows
/// as text streams in (0 on the initial pre-stream estimate).
///
/// The raw (uncalibrated) preamble/tools estimates are always attached so the
/// frontend can render the composition breakdown even mid-stream. Cache fields
/// are intentionally omitted: they are unknown until FinalResponse, and the
/// frontend freezes the last real cache values instead of showing a fake 0.
pub(super) fn emit_usage_estimate(
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
pub(super) fn emit_usage_real(
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
pub(super) fn emit_usage_run_summary(
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
