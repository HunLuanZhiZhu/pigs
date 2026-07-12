//! Client factory for Anthropic Messages and OpenAI Chat Completions.
//!
//! Wire formats only. Multi-vendor endpoints are configured via named providers
//! in `pigs-config` (`[[providers]]` + `[[models]]`).

use std::sync::Arc;

use pigs_core::ApiClient;

use crate::anthropic::AnthropicClient;
use crate::openai::OpenAiClient;
use crate::openai_responses::OpenAiResponsesClient;

/// Wire format used to talk to an LLM endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenAI Responses API (`/v1/responses`) — current OpenAI agent protocol (Codex default).
    OpenAI,
    /// OpenAI Chat Completions (`/v1/chat/completions`) — legacy / third-party compatible.
    OpenAIChat,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::OpenAIChat => "openai-chat",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Credentials and endpoint for creating a client.
#[derive(Debug, Clone, Default)]
pub struct ClientConfig<'a> {
    pub openai_api_key: Option<&'a str>,
    pub openai_base_url: Option<&'a str>,
    pub anthropic_api_key: Option<&'a str>,
    pub anthropic_base_url: Option<&'a str>,
}

/// Create a client from an explicit API format + credentials.
pub fn create_client_for_endpoint(
    api: Provider,
    model: &str,
    api_key: &str,
    base_url: &str,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    if model.trim().is_empty() {
        return Err(pigs_core::ApiError::Config("model id is empty".into()));
    }
    if api_key.trim().is_empty() {
        return Err(pigs_core::ApiError::Config("api_key is empty".into()));
    }
    let base_url = if base_url.trim().is_empty() {
        match api {
            Provider::OpenAI | Provider::OpenAIChat => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com",
        }
    } else {
        base_url
    };

    match api {
        Provider::Anthropic => Ok(Arc::new(AnthropicClient::new(api_key, model, base_url))),
        Provider::OpenAI => Ok(Arc::new(OpenAiResponsesClient::new(api_key, model, base_url))),
        Provider::OpenAIChat => Ok(Arc::new(OpenAiClient::new(api_key, model, base_url))),
    }
}

/// Legacy helper: infer format from model name (`claude*` => Anthropic).
pub fn detect_provider(model: &str) -> Provider {
    if model.to_lowercase().starts_with("claude") {
        Provider::Anthropic
    } else {
        Provider::OpenAI
    }
}

/// Built-in short aliases (not a vendor catalog).
pub fn resolve_model_alias(alias: &str) -> String {
    match alias.to_lowercase().as_str() {
        "opus" => "claude-opus-4-20250514".to_string(),
        "sonnet" => "claude-sonnet-4-20250514".to_string(),
        "haiku" => "claude-3-5-haiku-20241022".to_string(),
        "gpt-4" | "gpt-4o" => "gpt-4o".to_string(),
        "gpt-4-mini" | "gpt-4o-mini" => "gpt-4o-mini".to_string(),
        _ => alias.to_string(),
    }
}

/// Create an API client using legacy dual-slot credentials and model-name routing.
pub fn create_client(
    model: &str,
    openai_api_key: Option<&str>,
    openai_base_url: Option<&str>,
    anthropic_api_key: Option<&str>,
    anthropic_base_url: Option<&str>,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    create_client_with_config(
        model,
        ClientConfig {
            openai_api_key,
            openai_base_url,
            anthropic_api_key,
            anthropic_base_url,
        },
    )
}

/// Create an API client with full legacy configuration.
pub fn create_client_with_config(
    model: &str,
    config: ClientConfig<'_>,
) -> Result<Arc<dyn ApiClient>, pigs_core::ApiError> {
    let model = resolve_model_alias(model);
    match detect_provider(&model) {
        Provider::Anthropic => {
            let api_key = config.anthropic_api_key.ok_or_else(|| {
                pigs_core::ApiError::Config(
                    "ANTHROPIC_API_KEY is required for Claude models.".into(),
                )
            })?;
            let base_url = config
                .anthropic_base_url
                .unwrap_or("https://api.anthropic.com");
            create_client_for_endpoint(Provider::Anthropic, &model, api_key, base_url)
        }
        // detect_provider never returns OpenAIChat; default OpenAI path uses Responses.
        Provider::OpenAI | Provider::OpenAIChat => {
            let api_key = config.openai_api_key.ok_or_else(|| {
                pigs_core::ApiError::Config(
                    "OPENAI_API_KEY is required for OpenAI models.".into(),
                )
            })?;
            let base_url = config
                .openai_base_url
                .unwrap_or("https://api.openai.com/v1");
            create_client_for_endpoint(Provider::OpenAI, &model, api_key, base_url)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_detect_provider() {
        assert_eq!(detect_provider("claude-sonnet-4-20250514"), Provider::Anthropic);
        assert_eq!(detect_provider("gpt-4o"), Provider::OpenAI);
        assert_eq!(detect_provider("deepseek-chat"), Provider::OpenAI);
    }

    #[test]
    fn test_create_for_endpoint() {
        let client = create_client_for_endpoint(
            Provider::OpenAI,
            "my-model",
            "dummy",
            "http://127.0.0.1:8080/v1",
        )
        .unwrap();
        assert_eq!(client.model(), "my-model");
    }

    #[test]
    fn test_create_client_missing_key() {
        assert!(create_client("claude-sonnet-4-20250514", None, None, None, None).is_err());
        assert!(create_client("gpt-4o", None, None, None, None).is_err());
    }
}
