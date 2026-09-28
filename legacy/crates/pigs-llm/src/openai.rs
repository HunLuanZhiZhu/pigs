//! OpenAI Chat Completions client (`POST {base}/chat/completions`).
//!
//! Reference implementations consulted:
//! - claw-code `rust/crates/api/src/providers/openai_compat.rs`
//!   (system as first message, stream_options.include_usage, tool-message pairing)
//! - pi `packages/ai/src/api/openai-completions.ts`
//!   (max_tokens vs max_completion_tokens, stream usage)

use std::time::Duration;

use futures_util::StreamExt;
use pigs_core::{
    ApiClient, ApiError, ApiFuture, ApiRequest, ApiResponse, ContentBlock, Message, MessageRole,
    StreamCallback, StreamEvent, TokenUsage,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::http_util::{is_non_retryable, join_url, map_error_response};

/// OpenAI-compatible Chat Completions client.
pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
    max_retries: u32,
}

impl OpenAiClient {
    pub fn new(api_key: &str, model: &str, base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            max_retries: 3,
        }
    }

    fn endpoint(&self) -> String {
        join_url(&self.base_url, "chat/completions")
    }

    fn build_request_body(&self, request: &ApiRequest, stream: bool) -> OpenAiRequest {
        // claw-code/pi: inject system prompt as the first chat message (not a top-level field).
        let mut messages: Vec<OpenAiMessage> = Vec::new();
        if let Some(system) = request
            .system_prompt
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            messages.push(OpenAiMessage {
                role: "system".into(),
                content: Some(system.to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for msg in &request.messages {
            if msg.role == MessageRole::System {
                continue;
            }
            messages.push(convert_message_to_openai(msg));
        }
        messages = sanitize_tool_message_pairing(messages);

        let tools: Vec<OpenAiTool> = request
            .tools
            .iter()
            .map(|t| OpenAiTool {
                r#type: "function".into(),
                function: OpenAiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        // claw-code: gpt-5* wants max_completion_tokens; older models use max_tokens.
        let (max_tokens, max_completion_tokens) =
            split_max_tokens_field(&request.model, request.max_tokens);

        OpenAiRequest {
            model: request.model.clone(),
            messages,
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(serde_json::Value::String("auto".into()))
            },
            tools: if tools.is_empty() { None } else { Some(tools) },
            max_tokens,
            max_completion_tokens,
            temperature: request.temperature,
            stream,
            stream_options: if stream {
                Some(StreamOptions {
                    include_usage: true,
                })
            } else {
                None
            },
        }
    }
}

impl ApiClient for OpenAiClient {
    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            let body = self.build_request_body(&request, false);
            let mut last_error = None;
            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying OpenAI Chat Completions request");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }
                match self.send_request(&body).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Chat Completions request failed");
                        last_error = Some(e);
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| ApiError::Request("All retries exhausted".into())))
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
            let body = self.build_request_body(&request, true);
            let mut last_error = None;
            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying OpenAI Chat Completions stream");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }
                match self.send_streaming_request(&body, callback).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Chat Completions stream failed");
                        last_error = Some(e);
                    }
                }
            }
            Err(last_error
                .unwrap_or_else(|| ApiError::Request("All streaming retries exhausted".into())))
        })
    }
}

impl OpenAiClient {
    async fn send_request(&self, body: &OpenAiRequest) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .map_err(network_err)?;

        if !response.status().is_success() {
            return Err(map_error_response(response).await);
        }

        let response_body: OpenAiResponse = response.json().await.map_err(|e| {
            ApiError::InvalidResponse(format!("Failed to parse response JSON: {e}"))
        })?;
        Ok(convert_response(response_body))
    }

    async fn send_streaming_request(
        &self,
        body: &OpenAiRequest,
        callback: &dyn StreamCallback,
    ) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .map_err(network_err)?;

        if !response.status().is_success() {
            return Err(map_error_response(response).await);
        }

        let mut stream = response.bytes_stream();
        let mut full_text = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new();
        let mut usage = None;
        let mut stop_reason = None;
        let mut buffer = String::new();
        let mut response_model = self.model.clone();

        while let Some(chunk_result) = stream.next().await {
            let chunk =
                chunk_result.map_err(|e| ApiError::Network(format!("Stream error: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                let Ok(chunk) = serde_json::from_str::<OpenAiStreamChunk>(data) else {
                    continue;
                };
                if let Some(m) = chunk.model.filter(|s| !s.is_empty()) {
                    response_model = m;
                }
                for choice in &chunk.choices {
                    if let Some(delta) = &choice.delta {
                        if let Some(text) = &delta.content {
                            if !text.is_empty() {
                                full_text.push_str(text);
                                callback.on_event(&StreamEvent::TextDelta(text.clone()));
                            }
                        }
                        if let Some(tc_arr) = &delta.tool_calls {
                            for tc in tc_arr {
                                let idx = tc.index as usize;
                                while tool_calls.len() <= idx {
                                    tool_calls.push((String::new(), String::new(), String::new()));
                                }
                                if let Some(id) = &tc.id {
                                    if !id.is_empty() {
                                        tool_calls[idx].0 = id.clone();
                                    }
                                }
                                if let Some(function) = &tc.function {
                                    if let Some(name) = &function.name {
                                        if !name.is_empty() {
                                            tool_calls[idx].1 = name.clone();
                                            let id = tool_calls[idx].0.clone();
                                            if !id.is_empty() {
                                                callback.on_event(&StreamEvent::ToolUseStart {
                                                    id,
                                                    name: name.clone(),
                                                });
                                            }
                                        }
                                    }
                                    if let Some(args) = &function.arguments {
                                        if !args.is_empty() {
                                            tool_calls[idx].2.push_str(args);
                                            let id = tool_calls[idx].0.clone();
                                            if !id.is_empty() {
                                                callback.on_event(
                                                    &StreamEvent::ToolUseInputDelta {
                                                        id,
                                                        partial_json: args.clone(),
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(reason) = &choice.finish_reason {
                        stop_reason = Some(reason.clone());
                        for (id, _, _) in &tool_calls {
                            if !id.is_empty() {
                                callback.on_event(&StreamEvent::ToolUseEnd { id: id.clone() });
                            }
                        }
                    }
                }
                if let Some(u) = chunk.usage {
                    usage = Some(TokenUsage {
                        input_tokens: u.prompt_tokens,
                        output_tokens: u.completion_tokens,
                        cache_read_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
                        total_cost: None,
                    });
                }
            }
        }

        if let Some(u) = &usage {
            callback.on_event(&StreamEvent::Usage(u.clone()));
        }
        callback.on_event(&StreamEvent::Done {
            stop_reason: stop_reason.clone(),
        });

        let mut content = Vec::new();
        if !full_text.is_empty() {
            content.push(ContentBlock::text(full_text));
        }
        for (id, name, args_json) in &tool_calls {
            if id.is_empty() {
                continue;
            }
            let input = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
            content.push(ContentBlock::tool_use(id, name, input));
        }

        Ok(ApiResponse {
            content,
            usage,
            model: response_model,
            stop_reason,
        })
    }
}

fn convert_response(response: OpenAiResponse) -> ApiResponse {
    let mut content = Vec::new();
    for choice in &response.choices {
        if let Some(text) = &choice.message.content {
            if !text.is_empty() {
                content.push(ContentBlock::text(text.clone()));
            }
        }
        if let Some(tool_calls) = &choice.message.tool_calls {
            for tc in tool_calls {
                let input =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
                content.push(ContentBlock::tool_use(&tc.id, &tc.function.name, input));
            }
        }
    }
    let usage = response.usage.map(|u| TokenUsage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        cache_read_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
        total_cost: None,
    });
    let stop_reason = response
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone());
    ApiResponse {
        content,
        usage,
        model: response.model,
        stop_reason,
    }
}

fn convert_message_to_openai(msg: &Message) -> OpenAiMessage {
    let role = match msg.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
    .to_string();

    let text_content: String = msg
        .content
        .iter()
        .filter_map(|b| b.as_text())
        .collect::<Vec<_>>()
        .join("\n");

    let tool_calls: Vec<OpenAiToolCall> = msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some(OpenAiToolCall {
                id: id.clone(),
                r#type: "function".into(),
                function: OpenAiToolCallFunction {
                    name: name.clone(),
                    arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                },
            }),
            _ => None,
        })
        .collect();

    let tool_call_id = msg.content.iter().find_map(|b| match b {
        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
        _ => None,
    });

    let content = if msg.role == MessageRole::Tool {
        msg.content
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            })
            .or(if text_content.is_empty() {
                None
            } else {
                Some(text_content)
            })
    } else if text_content.is_empty() {
        // OpenAI allows null content when tool_calls is present.
        None
    } else {
        Some(text_content)
    };

    OpenAiMessage {
        role,
        content,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        tool_call_id,
    }
}

/// Drop orphan tool messages without a preceding assistant tool_calls id.
/// Mirrors claw-code `sanitize_tool_message_pairing`.
fn sanitize_tool_message_pairing(messages: Vec<OpenAiMessage>) -> Vec<OpenAiMessage> {
    let mut known_ids = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages {
        if msg.role == "assistant" {
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    known_ids.insert(tc.id.clone());
                }
            }
            out.push(msg);
            continue;
        }
        if msg.role == "tool" {
            match &msg.tool_call_id {
                Some(id) if known_ids.contains(id) => out.push(msg),
                _ => {
                    debug!("dropping orphan tool message without matching tool_calls id");
                }
            }
            continue;
        }
        out.push(msg);
    }
    out
}

fn split_max_tokens_field(model: &str, max: Option<u32>) -> (Option<u32>, Option<u32>) {
    let Some(max) = max else {
        return (None, None);
    };
    let m = model.to_ascii_lowercase();
    // claw-code: gpt-5* requires max_completion_tokens
    if m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        (None, Some(max))
    } else {
        (Some(max), None)
    }
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

// --- Wire types ---

#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    r#type: String,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    model: String,
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCall {
    index: u32,
    id: Option<String>,
    function: Option<OpenAiStreamToolCallFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pigs_core::ToolSpec;

    #[test]
    fn system_prompt_is_first_message_not_top_level_field() {
        let client = OpenAiClient::new("k", "gpt-4o", "https://api.openai.com/v1");
        let req =
            ApiRequest::new("gpt-4o", vec![Message::user("hi")]).with_system_prompt("you are pigs");
        let body = client.build_request_body(&req, false);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("system").is_none());
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][0]["content"], "you are pigs");
        assert_eq!(json["messages"][1]["role"], "user");
    }

    #[test]
    fn stream_options_include_usage_when_streaming() {
        let client = OpenAiClient::new("k", "gpt-4o", "https://api.openai.com/v1");
        let body =
            client.build_request_body(&ApiRequest::new("gpt-4o", vec![Message::user("x")]), true);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["stream"], true);
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn gpt5_uses_max_completion_tokens() {
        let client = OpenAiClient::new("k", "gpt-5", "https://api.openai.com/v1");
        let req = ApiRequest::new("gpt-5", vec![Message::user("x")]).with_max_tokens(100);
        let body = client.build_request_body(&req, false);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("max_tokens").is_none());
        assert_eq!(json["max_completion_tokens"], 100);
    }

    #[test]
    fn orphan_tool_messages_are_dropped() {
        let msgs = vec![
            OpenAiMessage {
                role: "user".into(),
                content: Some("hi".into()),
                tool_calls: None,
                tool_call_id: None,
            },
            OpenAiMessage {
                role: "tool".into(),
                content: Some("orphan".into()),
                tool_calls: None,
                tool_call_id: Some("missing".into()),
            },
        ];
        let cleaned = sanitize_tool_message_pairing(msgs);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].role, "user");
    }

    #[test]
    fn tools_set_tool_choice_auto() {
        let client = OpenAiClient::new("k", "gpt-4o", "https://api.openai.com/v1");
        let req =
            ApiRequest::new("gpt-4o", vec![Message::user("x")]).with_tools(vec![ToolSpec::new(
                "bash",
                "run",
                serde_json::json!({"type":"object"}),
            )]);
        let body = client.build_request_body(&req, false);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert!(json["tools"].as_array().unwrap().len() == 1);
    }
}
