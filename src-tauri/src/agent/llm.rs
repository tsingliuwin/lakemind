//! One-shot LLM completion (no tools, no window events).
//!
//! Used by the naming module (to ask for a concise table identifier) and by the
//! `tidy_okf_knowledge` tool (to ask the model to reorganize the OKF knowledge
//! base). Mirrors the provider-client construction in
//! [`crate::agent::runner::run_agent_chat_stream`] but stripped down.

use rig_core::{
    client::CompletionClient,
    completion::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
    agent::MultiTurnStreamItem,
};

use super::config::{get_provider_for_model, sanitize_endpoint};

/// One-shot LLM completion: stream the model's reply but gather all text locally
/// (no window events, no tools). Returns the concatenated assistant text.
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
/// string. Mirrors [`super::runner::run_stream_loop`] but returns text instead of
/// emitting window events. Tool/reasoning items are ignored (no tools are
/// attached for one-shot calls).
async fn collect_stream_text<R>(
    mut stream: impl futures_util::Stream<Item = Result<MultiTurnStreamItem<R>, rig_core::agent::StreamingError>> + Unpin,
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
