//! LLM API abstraction — the ApiClient trait and request/response types.

use std::future::Future;
use std::pin::Pin;

use crate::{Message, TokenUsage, ToolSpec};

/// Error type for API operations.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("HTTP error {status}: {body}")]
    Http { status: u16, body: String },
    #[error("Authentication failed: {0}")]
    Auth(String),
    #[error("Rate limited. Retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    #[error("Context window exceeded: {0}")]
    ContextWindowExceeded(String),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Request error: {0}")]
    Request(String),
}

/// A request to send to the LLM.
#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

impl ApiRequest {
    /// Create a new API request.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        ApiRequest {
            model: model.into(),
            messages,
            system_prompt: None,
            tools: Vec::new(),
            max_tokens: None,
            temperature: None,
        }
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the available tools.
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    /// Set max output tokens.
    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }
}

/// A response from the LLM.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub content: Vec<crate::ContentBlock>,
    pub usage: Option<TokenUsage>,
    pub model: String,
    pub stop_reason: Option<String>,
}

impl ApiResponse {
    /// Create a new API response.
    pub fn new(model: impl Into<String>, content: Vec<crate::ContentBlock>) -> Self {
        ApiResponse {
            content,
            usage: None,
            model: model.into(),
            stop_reason: None,
        }
    }

    /// Check if the response contains any tool-use requests.
    pub fn has_tool_uses(&self) -> bool {
        self.content.iter().any(|b| b.is_tool_use())
    }

    /// Get all text content from the response.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Type alias for the boxed future returned by `ApiClient::send_message`.
pub type ApiFuture<'a> = Pin<Box<dyn Future<Output = Result<ApiResponse, ApiError>> + Send + 'a>>;

/// A streaming event from the LLM, emitted incrementally as the response is generated.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of text content.
    TextDelta(String),
    /// The start of a tool-use block.
    ToolUseStart { id: String, name: String },
    /// A chunk of tool-use input (partial JSON).
    ToolUseInputDelta { id: String, partial_json: String },
    /// The end of a tool-use block.
    ToolUseEnd { id: String },
    /// Token usage information.
    Usage(TokenUsage),
    /// The response is complete.
    Done { stop_reason: Option<String> },
}

/// A callback trait for receiving streaming events.
pub trait StreamCallback: Send + Sync {
    fn on_event(&self, event: &StreamEvent);
}

/// Trait for LLM API clients. Implementations include OpenAI and Anthropic.
pub trait ApiClient: Send + Sync {
    /// Send a message request to the LLM and get a response.
    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a>;

    /// Get the model name this client is configured for.
    fn model(&self) -> &str;

    /// Send a message with streaming. Default implementation falls back to non-streaming.
    fn send_message_streaming<'a>(
        &'a self,
        request: ApiRequest,
        callback: &'a dyn StreamCallback,
    ) -> ApiFuture<'a> {
        // Default: emit the full response as a single event, then Done
        Box::pin(async move {
            let response = self.send_message(request).await?;

            // Emit text as a single delta
            let text = response.text_content();
            if !text.is_empty() {
                callback.on_event(&StreamEvent::TextDelta(text));
            }

            // Emit tool-use events
            for block in &response.content {
                if let crate::ContentBlock::ToolUse { id, name, input } = block {
                    callback.on_event(&StreamEvent::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    let json_str = serde_json::to_string(input).unwrap_or_default();
                    callback.on_event(&StreamEvent::ToolUseInputDelta {
                        id: id.clone(),
                        partial_json: json_str,
                    });
                    callback.on_event(&StreamEvent::ToolUseEnd { id: id.clone() });
                }
            }

            // Emit usage
            if let Some(usage) = &response.usage {
                callback.on_event(&StreamEvent::Usage(usage.clone()));
            }

            callback.on_event(&StreamEvent::Done {
                stop_reason: response.stop_reason.clone(),
            });

            Ok(response)
        })
    }
}
