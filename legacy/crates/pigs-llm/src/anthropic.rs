//! Anthropic Messages API client.
//!
//! Uses the Anthropic Messages API: `POST {base_url}/v1/messages`
//! with `x-api-key: {key}` and `anthropic-version: 2023-06-01` headers.
//!
//! Supports both streaming (SSE) and non-streaming requests.
//!
//! Reference implementations consulted:
//! - claw-code `rust/crates/api` (Messages types, SSE framing, tool_result blocks)
//! - pi `packages/ai/src/api/anthropic-messages.ts` (headers, streaming events)

use std::time::Duration;

use pigs_core::{
    ApiClient, ApiError, ApiFuture, ApiRequest, ApiResponse, StreamCallback, StreamEvent,
};
use pigs_core::{ContentBlock, Message, MessageRole, TokenUsage};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::http_util::{is_non_retryable, join_url, map_error_response};

/// Anthropic API client.
pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
    max_retries: u32,
}

impl AnthropicClient {
    /// Create a new Anthropic client.
    pub fn new(api_key: &str, model: &str, base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        AnthropicClient {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            max_retries: 3,
        }
    }

    /// Get the API endpoint URL.
    fn endpoint(&self) -> String {
        // Accept base URLs that already include /v1.
        if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            join_url(&self.base_url, "messages")
        } else if self.base_url.ends_with("/messages") {
            self.base_url.clone()
        } else {
            join_url(&self.base_url, "v1/messages")
        }
    }
}

impl ApiClient for AnthropicClient {
    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            let body = self.build_request_body(&request)?;

            let mut last_error: Option<ApiError> = None;

            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying Anthropic request");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }

                match self.send_request(&body).await {
                    Ok(response) => return Ok(response),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Anthropic request failed, will retry");
                        last_error = Some(e);
                    }
                }
            }

            Err(last_error
                .unwrap_or_else(|| ApiError::Request("All retries exhausted".to_string())))
        })
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn send_message_streaming<'a>(
        &'a self,
        request: ApiRequest,
        callback: &'a dyn StreamCallback,
    ) -> ApiFuture<'a> {
        Box::pin(async move {
            let mut body = self.build_request_body(&request)?;
            body.stream = true;

            let mut last_error: Option<ApiError> = None;

            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying Anthropic streaming request");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }

                match self.send_streaming_request(&body, callback).await {
                    Ok(response) => return Ok(response),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Anthropic stream failed, will retry");
                        last_error = Some(e);
                    }
                }
            }

            Err(last_error.unwrap_or_else(|| {
                ApiError::Request("All streaming retries exhausted".to_string())
            }))
        })
    }
}

impl AnthropicClient {
    /// Build the Anthropic API request body.
    fn build_request_body(&self, request: &ApiRequest) -> Result<AnthropicRequest, ApiError> {
        let messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .map(convert_message_to_anthropic)
            .collect();

        let tools: Vec<AnthropicTool> = request
            .tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        // Anthropic requires max_tokens
        let max_tokens = request.max_tokens.unwrap_or(4096);

        Ok(AnthropicRequest {
            model: request.model.clone(),
            messages,
            system: request.system_prompt.clone(),
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(serde_json::json!({"type": "auto"}))
            },
            tools: if tools.is_empty() { None } else { Some(tools) },
            max_tokens,
            temperature: request.temperature,
            stream: false,
        })
    }

    /// Send the HTTP request and parse the response.
    async fn send_request(&self, body: &AnthropicRequest) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(network_err)?;

        if !response.status().is_success() {
            return Err(map_error_response(response).await);
        }

        let response_body: AnthropicResponse = response.json().await.map_err(|e| {
            ApiError::InvalidResponse(format!("Failed to parse response JSON: {e}"))
        })?;
        Ok(self.convert_response(response_body))
    }

    /// Send a streaming request and parse Anthropic SSE events in real time.
    async fn send_streaming_request(
        &self,
        body: &AnthropicRequest,
        callback: &dyn StreamCallback,
    ) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ApiError::Network(format!("Request timed out: {e}"))
                } else if e.is_connect() {
                    ApiError::Network(format!("Connection failed: {e}"))
                } else {
                    ApiError::Network(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            return Err(map_error_response(response).await);
        }

        // Stream the response body
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();

        // State for assembling the response
        let mut full_text = String::new();
        let mut tool_uses: Vec<(String, String, String)> = Vec::new(); // (id, name, partial_input_json)
        let mut usage: Option<TokenUsage> = None;
        let mut stop_reason: Option<String> = None;
        let mut model_name = self.model.clone();

        // SSE parsing state
        let mut buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk =
                chunk_result.map_err(|e| ApiError::Network(format!("Stream error: {e}")))?;

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE events (separated by \n\n)
            while let Some(event_end) = buffer.find("\n\n") {
                let event_block = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                // Parse the event block
                let mut event_type = String::new();
                let mut data_lines = Vec::new();

                for line in event_block.lines() {
                    if let Some(et) = line.strip_prefix("event: ") {
                        event_type = et.trim().to_string();
                    } else if let Some(data) = line.strip_prefix("data: ") {
                        data_lines.push(data.to_string());
                    }
                }

                if data_lines.is_empty() {
                    continue;
                }

                let data = data_lines.join("\n");

                match event_type.as_str() {
                    "message_start" => {
                        if let Ok(msg_start) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(m) = msg_start.get("message") {
                                if let Some(mdl) = m.get("model").and_then(|v| v.as_str()) {
                                    model_name = mdl.to_string();
                                }
                                if let Some(u) = m.get("usage") {
                                    let existing = usage.get_or_insert(TokenUsage::default());
                                    if let Some(input) =
                                        u.get("input_tokens").and_then(|v| v.as_u64())
                                    {
                                        existing.input_tokens = input;
                                    }
                                    if let Some(cached) =
                                        u.get("cache_read_input_tokens").and_then(|v| v.as_u64())
                                    {
                                        existing.cache_read_tokens = Some(cached);
                                    }
                                }
                            }
                        }
                    }

                    "content_block_start" => {
                        if let Ok(block_start) =
                            serde_json::from_str::<AnthropicStreamBlockStart>(&data)
                        {
                            let block = block_start.content_block;
                            match block.r#type.as_str() {
                                "text" => {
                                    // Text block started, content will arrive in deltas
                                }
                                "tool_use" => {
                                    let id = block.id.unwrap_or_default();
                                    let name = block.name.unwrap_or_default();
                                    tool_uses.push((id.clone(), name.clone(), String::new()));
                                    callback.on_event(&StreamEvent::ToolUseStart { id, name });
                                }
                                _ => {}
                            }
                        }
                    }

                    "content_block_delta" => {
                        if let Ok(delta_event) =
                            serde_json::from_str::<AnthropicStreamDeltaEvent>(&data)
                        {
                            let delta = delta_event.delta;
                            match delta.r#type.as_str() {
                                "text_delta" => {
                                    if let Some(text) = &delta.text {
                                        full_text.push_str(text);
                                        callback.on_event(&StreamEvent::TextDelta(text.clone()));
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(partial) = &delta.partial_json {
                                        if !tool_uses.is_empty() {
                                            let last = tool_uses.len() - 1;
                                            tool_uses[last].2.push_str(partial);
                                            callback.on_event(&StreamEvent::ToolUseInputDelta {
                                                id: tool_uses[last].0.clone(),
                                                partial_json: partial.clone(),
                                            });
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    "content_block_stop" => {
                        // Emit ToolUseEnd if we have an active tool_use
                        if !tool_uses.is_empty() {
                            let last = tool_uses.len() - 1;
                            if !tool_uses[last].0.is_empty() {
                                callback.on_event(&StreamEvent::ToolUseEnd {
                                    id: tool_uses[last].0.clone(),
                                });
                            }
                        }
                    }

                    "message_delta" => {
                        if let Ok(msg_delta) =
                            serde_json::from_str::<AnthropicStreamMessageDelta>(&data)
                        {
                            if let Some(reason) = msg_delta.delta.stop_reason {
                                stop_reason = Some(reason);
                            }
                            if let Some(u) = msg_delta.usage {
                                if let Some(output) = u.output_tokens {
                                    // Merge with any existing usage from message_start
                                    let existing = usage.get_or_insert(TokenUsage::default());
                                    existing.output_tokens = output;
                                }
                            }
                        }
                    }

                    "message_stop" => {
                        // Stream is complete
                    }

                    "ping" | "error" => {
                        if event_type == "error" {
                            if let Ok(err_val) = serde_json::from_str::<serde_json::Value>(&data) {
                                let msg = err_val
                                    .get("error")
                                    .and_then(|e| e.get("message"))
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown streaming error");
                                return Err(ApiError::InvalidResponse(msg.to_string()));
                            }
                        }
                    }

                    _ => {
                        debug!(event_type = %event_type, "Unknown SSE event type");
                    }
                }
            }
        }

        // Emit usage event
        if let Some(ref u) = usage {
            callback.on_event(&StreamEvent::Usage(u.clone()));
        }

        callback.on_event(&StreamEvent::Done {
            stop_reason: stop_reason.clone(),
        });

        // Build the final response
        let mut content = Vec::new();
        if !full_text.is_empty() {
            content.push(ContentBlock::text(full_text));
        }
        for (id, name, input_json) in &tool_uses {
            if !id.is_empty() {
                let input: serde_json::Value =
                    serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null);
                content.push(ContentBlock::tool_use(id, name, input));
            }
        }

        Ok(ApiResponse {
            content,
            usage,
            model: model_name,
            stop_reason,
        })
    }

    /// Convert the Anthropic response to our internal ApiResponse.
    fn convert_response(&self, response: AnthropicResponse) -> ApiResponse {
        let mut content = Vec::new();

        for block in &response.content {
            match block {
                AnthropicResponseBlock::Text { text } => {
                    content.push(ContentBlock::text(text.clone()));
                }
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    content.push(ContentBlock::tool_use(id, name, input.clone()));
                }
            }
        }

        let usage = response.usage.map(|u| TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_tokens: u.cache_read_input_tokens,
            total_cost: None,
        });

        ApiResponse {
            content,
            usage,
            model: response.model,
            stop_reason: response.stop_reason,
        }
    }
}

// --- Request wire types ---

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// --- Response wire types ---

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    model: String,
    content: Vec<AnthropicResponseBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicResponseBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

/// Convert our internal Message to an Anthropic-format message.
/// Returns a JSON value for the content field (can be string or array of blocks).
fn convert_message_to_anthropic(msg: &Message) -> AnthropicMessage {
    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        // Anthropic doesn't have a "tool" role — tool results go as user messages
        // with tool_result content blocks. System messages are filtered out separately.
        MessageRole::Tool => "user",
        MessageRole::System => "user",
    }
    .to_string();

    let mut blocks: Vec<serde_json::Value> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                blocks.push(serde_json::json!({
                    "type": "text",
                    "text": text
                }));
            }
            ContentBlock::ToolUse { id, name, input } => {
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                output,
                is_error,
            } => {
                let mut result = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": output
                });
                if *is_error {
                    result["is_error"] = serde_json::json!(true);
                }
                blocks.push(result);
            }
        }
    }

    // If there's only a single text block, use the simple string format
    let content = if blocks.len() == 1 {
        if let Some(text) = blocks[0].get("text").and_then(|t| t.as_str()) {
            if blocks[0].get("type").and_then(|t| t.as_str()) == Some("text") {
                serde_json::Value::String(text.to_string())
            } else {
                serde_json::Value::Array(blocks)
            }
        } else {
            serde_json::Value::Array(blocks)
        }
    } else if blocks.is_empty() {
        serde_json::Value::String(String::new())
    } else {
        serde_json::Value::Array(blocks)
    };

    AnthropicMessage { role, content }
}

// --- Streaming wire types ---

#[derive(Debug, Deserialize)]
struct AnthropicStreamBlockStart {
    content_block: AnthropicStreamContentBlock,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamContentBlock {
    r#type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamDeltaEvent {
    delta: AnthropicStreamDelta,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamDelta {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessageDelta {
    delta: AnthropicStreamMessageDeltaInner,
    #[serde(default)]
    usage: Option<AnthropicStreamMessageUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessageDeltaInner {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessageUsage {
    #[serde(default)]
    output_tokens: Option<u64>,
}

fn network_err(e: reqwest::Error) -> ApiError {
    if e.is_timeout() {
        ApiError::Network(format!("Request timed out: {e}"))
    } else if e.is_connect() {
        ApiError::Network(format!("Connection failed: {e}"))
    } else {
        ApiError::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pigs_core::ToolSpec;

    #[test]
    fn endpoint_handles_base_with_v1() {
        let c = AnthropicClient::new(
            "k",
            "claude-sonnet-4-20250514",
            "https://api.anthropic.com/v1",
        );
        assert_eq!(c.endpoint(), "https://api.anthropic.com/v1/messages");
        let c2 = AnthropicClient::new("k", "claude-sonnet-4-20250514", "https://api.anthropic.com");
        assert_eq!(c2.endpoint(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn tools_set_tool_choice_auto() {
        let c = AnthropicClient::new("k", "claude-sonnet-4-20250514", "https://api.anthropic.com");
        let req = ApiRequest::new("claude-sonnet-4-20250514", vec![Message::user("hi")])
            .with_tools(vec![ToolSpec::new(
                "bash",
                "run",
                serde_json::json!({"type": "object"}),
            )]);
        let body = c.build_request_body(&req).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tool_choice"]["type"], "auto");
        assert!(json["tools"].as_array().unwrap().len() == 1);
        assert_eq!(json["max_tokens"], 4096);
    }

    #[test]
    fn tool_result_is_user_role_with_is_error() {
        let msg = Message {
            role: MessageRole::Tool,
            content: vec![ContentBlock::tool_result("call1", "boom", true)],
            usage: None,
        };
        let converted = convert_message_to_anthropic(&msg);
        assert_eq!(converted.role, "user");
        let content = converted.content;
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["is_error"], true);
    }
}
