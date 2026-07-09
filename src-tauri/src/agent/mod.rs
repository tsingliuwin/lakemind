//! Agent orchestration: rig tools, wire protocol, streaming runner, and the
//! single public entry point [`run_agent_chat_stream`].
//!
//! Originally a single ~2944-line `agent.rs`; split per concern:
//! - [`wire`]     — frontend-facing segment/event types
//! - [`events`]   — emit_* helpers + tool-call id generation
//! - [`config`]   — settings.json + provider lookup + endpoint sanitizing
//! - [`llm`]      — one-shot LLM completion (naming / OKF tidy)
//! - [`error`]    — shared ToolError
//! - [`sample_guard`] — sampled-aggregation interception
//! - [`okf_io`]   — OKF knowledge-base file helpers
//! - [`tools`]    — the 14 rig Tool implementations
//! - [`runner`]   — streaming driver + agent assembly + public entry point

mod config;
mod error;
mod events;
mod llm;
mod okf_io;
mod runner;
mod sample_guard;
mod tools;
mod wire;

// Re-export the public/crate-facing API so external callers keep using
// `crate::agent::<item>` unchanged (commands.rs, duckdb/*). Sub-modules
// (llm.rs, runner.rs) reach config/llm helpers directly via `super::config::`,
// so only the symbols actually consumed outside `agent` are re-exported here.
pub(crate) use config::{first_enabled_model, get_query_hard_timeout, get_query_timeout};
pub(crate) use llm::{complete_one_shot, test_connection};
pub(crate) use runner::run_agent_chat_stream;
pub(crate) use wire::AgentStreamEvent;
