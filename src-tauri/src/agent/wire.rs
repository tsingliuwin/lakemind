//! Wire protocol: ordered segment transcript streamed to the frontend.
//!
//! An assistant message is a list of `Segment`s in arrival order:
//!   reasoning → tool → reasoning → tool → text (final answer)
//! Each tool is one `Segment::Tool` whose status transitions running → ok|error
//! when the matching tool_result event arrives (updated in place by id).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::model::SqlResult;

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        right_y_fields: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_field_labels: Option<HashMap<String, String>>,
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
pub(super) struct ChatMessageDto {
    pub role: String, // "user" | "assistant"
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub segments: Option<Vec<Segment>>,
    pub ts: i64,
}
