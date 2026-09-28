//! LLM provider clients — OpenAI Responses / Chat Completions and Anthropic Messages.
//!
//! Protocol references used while implementing clients:
//! - Anthropic Messages: claw-code `api`, pi `anthropic-messages`
//! - OpenAI Chat Completions: claw-code `openai_compat`, pi `openai-completions`
//! - OpenAI Responses: codex `codex-api` / `WireApi::Responses`, pi `openai-responses`

pub mod anthropic;
pub mod http_util;
pub mod openai;
pub mod openai_responses;
pub mod provider;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;
pub use openai_responses::OpenAiResponsesClient;
pub use provider::{
    create_client, create_client_for_endpoint, create_client_with_config, detect_provider,
    resolve_model_alias, ClientConfig, Provider,
};
