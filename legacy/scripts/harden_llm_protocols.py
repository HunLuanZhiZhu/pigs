#!/usr/bin/env python3
"""Harden Anthropic / OpenAI Chat / OpenAI Responses clients using reference patterns."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LLM = ROOT / "crates" / "pigs-llm" / "src"


def patch_lib() -> None:
    path = LLM / "lib.rs"
    path.write_text(
        """//! LLM provider clients — OpenAI Responses / Chat Completions and Anthropic Messages.
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
""",
        encoding="utf-8",
    )
    print("lib.rs ok")


def rewrite_openai_chat() -> None:
    """Reference: claw-code openai_compat + pi openai-completions."""
    path = LLM / "openai.rs"
    path.write_text(
        r'''//! OpenAI Chat Completions client (`POST {base}/chat/completions`).
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
        let (max_tokens, max_completion_tokens) = split_max_tokens_field(&request.model, request.max_tokens);

        OpenAiRequest {
            model: request.model.clone(),
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(serde_json::Value::String("auto".into()))
            },
            max_tokens,
            max_completion_tokens,
            temperature: request.temperature,
            stream,
            stream_options: if stream {
                Some(StreamOptions { include_usage: true })
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

        let response_body: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| ApiError::InvalidResponse(format!("Failed to parse response JSON: {e}")))?;
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
            let chunk = chunk_result.map_err(|e| ApiError::Network(format!("Stream error: {e}")))?;
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
                                                callback.on_event(&StreamEvent::ToolUseInputDelta {
                                                    id,
                                                    partial_json: args.clone(),
                                                });
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
                let input = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
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
    let stop_reason = response.choices.first().and_then(|c| c.finish_reason.clone());
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
        msg.content.iter().find_map(|b| match b {
            ContentBlock::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .or_else(|| if text_content.is_empty() { None } else { Some(text_content) })
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
        let req = ApiRequest::new("gpt-4o", vec![Message::user("hi")])
            .with_system_prompt("you are pigs");
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
        let body = client.build_request_body(&ApiRequest::new("gpt-4o", vec![Message::user("x")]), true);
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
        let req = ApiRequest::new("gpt-4o", vec![Message::user("x")]).with_tools(vec![
            ToolSpec::new("bash", "run", serde_json::json!({"type":"object"})),
        ]);
        let body = client.build_request_body(&req, false);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tool_choice"], "auto");
        assert!(json["tools"].as_array().unwrap().len() == 1);
    }
}
''',
        encoding="utf-8",
    )
    print("openai.rs rewritten")


def patch_openai_responses() -> None:
    """Reference: codex ResponsesApiRequest + pi openai-responses."""
    path = LLM / "openai_responses.rs"
    text = path.read_text(encoding="utf-8")

    # module docs
    if "Reference implementations" not in text:
        text = text.replace(
            "//! Chat Completions remains available separately for third-party compatible endpoints.\n",
            "//! Chat Completions remains available separately for third-party compatible endpoints.\n"
            "//!\n"
            "//! Reference implementations consulted:\n"
            "//! - codex `codex-rs/codex-api` (`ResponsesApiRequest`, SSE event processing)\n"
            "//! - pi `packages/ai/src/api/openai-responses.ts` (store:false, max_output_tokens, tools)\n",
        )

    if "use crate::http_util" not in text:
        text = text.replace(
            "use tracing::{debug, warn};\n",
            "use tracing::{debug, warn};\n\nuse crate::http_util::{is_non_retryable, join_url, map_error_response, retry_after_secs};\n",
        )

    # endpoint via join_url
    text = text.replace(
        """    fn endpoint(&self) -> String {
        // Accept both ".../v1" and bare host; always call /responses relative to base.
        if self.base_url.ends_with("/responses") {
            self.base_url.clone()
        } else {
            format!("{}/responses", self.base_url)
        }
    }
""",
        """    fn endpoint(&self) -> String {
        // Accept both ".../v1" and bare host; always call /responses relative to base.
        if self.base_url.ends_with("/responses") {
            self.base_url.clone()
        } else {
            join_url(&self.base_url, "responses")
        }
    }
""",
    )

    # stream_options + include usage-like include field in request body
    if "stream_options:" not in text.split("struct ResponsesRequest")[0]:
        # add to build_request_body
        text = text.replace(
            """        ResponsesRequest {
            model: request.model.clone(),
            instructions: request.system_prompt.clone().unwrap_or_default(),
            input,
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            store: false,
            stream,
            max_output_tokens: request.max_tokens,
            temperature: request.temperature,
        }
""",
            """        ResponsesRequest {
            model: request.model.clone(),
            instructions: request.system_prompt.clone().unwrap_or_default(),
            input,
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            store: false,
            stream,
            // codex/pi: include usage on the stream when supported by the endpoint.
            stream_options: if stream {
                Some(ResponsesStreamOptions {
                    include_usage: true,
                })
            } else {
                None
            },
            max_output_tokens: request.max_tokens,
            temperature: request.temperature,
        }
""",
        )

    # map_http_error use shared helpers + retry-after
    old_map = """    async fn map_http_error(&self, response: reqwest::Response) -> ApiError {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        match status.as_u16() {
            401 | 403 => ApiError::Auth(text),
            404 => ApiError::ModelNotFound(text),
            429 => {
                // Retry-After not always present on Responses; default 30s.
                ApiError::RateLimited {
                    retry_after_secs: 30,
                }
            }
            status => {
                let lower = text.to_ascii_lowercase();
                if lower.contains("context") && lower.contains("window") {
                    ApiError::ContextWindowExceeded(text)
                } else {
                    ApiError::Http {
                        status,
                        body: text,
                    }
                }
            }
        }
    }
"""
    new_map = """    async fn map_http_error(&self, response: reqwest::Response) -> ApiError {
        // Prefer shared mapping; preserve Retry-After for 429.
        if response.status().as_u16() == 429 {
            let secs = retry_after_secs(&response, 30);
            let _ = response.text().await;
            return ApiError::RateLimited {
                retry_after_secs: secs,
            };
        }
        map_error_response(response).await
    }
"""
    if old_map in text:
        text = text.replace(old_map, new_map)

    # use is_non_retryable in loops
    text = text.replace(
        """                match self.send_once(&body).await {
                    Ok(r) => return Ok(r),
                    Err(e) => match &e {
                        ApiError::Auth(_) | ApiError::ModelNotFound(_) => return Err(e),
                        ApiError::RateLimited { .. } => {
                            warn!("Rate limited, will retry");
                            last_error = Some(e);
                        }
                        _ => {
                            warn!(error = %e, attempt, "Responses request failed, will retry");
                            last_error = Some(e);
                        }
                    },
                }
""",
        """                match self.send_once(&body).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Responses request failed, will retry");
                        last_error = Some(e);
                    }
                }
""",
    )
    text = text.replace(
        """                match self.send_streaming_once(&body, callback).await {
                    Ok(r) => return Ok(r),
                    Err(e) => match &e {
                        ApiError::Auth(_) | ApiError::ModelNotFound(_) => return Err(e),
                        ApiError::RateLimited { .. } => {
                            warn!("Rate limited on stream, will retry");
                            last_error = Some(e);
                        }
                        _ => {
                            warn!(error = %e, attempt, "Responses stream failed, will retry");
                            last_error = Some(e);
                        }
                    },
                }
""",
        """                match self.send_streaming_once(&body, callback).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Responses stream failed, will retry");
                        last_error = Some(e);
                    }
                }
""",
    )

    # ResponsesRequest struct fields
    text = text.replace(
        """struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    instructions: String,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    tool_choice: String,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}
""",
        """struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    instructions: String,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    tool_choice: String,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<ResponsesStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct ResponsesStreamOptions {
    include_usage: bool,
}
""",
    )

    # handle response.function_call_arguments.done if missing — add next to delta handler
    if "response.function_call_arguments.done" not in text:
        text = text.replace(
            '"response.function_call_arguments.delta" => {',
            '''"response.function_call_arguments.done" => {
                        // Some servers only send the final arguments blob here.
                        let call_id = value
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .or_else(|| value.get("item_id").and_then(|v| v.as_str()))
                            .unwrap_or("")
                            .to_string();
                        if let Some(args) = value.get("arguments").and_then(|v| v.as_str()) {
                            let entry = tool_args
                                .entry(call_id)
                                .or_insert_with(|| (String::new(), String::new()));
                            if entry.1.is_empty() {
                                entry.1 = args.to_string();
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {''',
        )

    # test stream_options
    if "stream_options" not in text.split("mod tests")[-1]:
        text = text.replace(
            "assert_eq!(json[\"store\"], false);\n    }\n",
            "assert_eq!(json[\"store\"], false);\n"
            "        assert_eq!(json[\"stream_options\"][\"include_usage\"], true);\n"
            "    }\n",
        )

    path.write_text(text, encoding="utf-8")
    print("openai_responses.rs patched")


def patch_anthropic() -> None:
    """Reference: claw-code api types/sse + pi anthropic-messages."""
    path = LLM / "anthropic.rs"
    text = path.read_text(encoding="utf-8")

    if "Reference implementations" not in text:
        text = text.replace(
            "//! Supports both streaming (SSE) and non-streaming requests.\n",
            "//! Supports both streaming (SSE) and non-streaming requests.\n"
            "//!\n"
            "//! Reference implementations consulted:\n"
            "//! - claw-code `rust/crates/api` (Messages types, SSE framing, tool_result blocks)\n"
            "//! - pi `packages/ai/src/api/anthropic-messages.ts` (headers, streaming events)\n",
        )

    if "use crate::http_util" not in text:
        text = text.replace(
            "use tracing::{debug, warn};\n",
            "use tracing::{debug, warn};\n\nuse crate::http_util::{is_non_retryable, join_url, map_error_response};\n",
        )

    text = text.replace(
        """    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }
""",
        """    fn endpoint(&self) -> String {
        // Accept base URLs that already include /v1.
        if self.base_url.ends_with("/v1") || self.base_url.ends_with("/v1/") {
            join_url(&self.base_url, "messages")
        } else if self.base_url.ends_with("/messages") {
            self.base_url.clone()
        } else {
            join_url(&self.base_url, "v1/messages")
        }
    }
""",
    )

    # tool_choice when tools present
    text = text.replace(
        """        Ok(AnthropicRequest {
            model: request.model.clone(),
            messages,
            system: request.system_prompt.clone(),
            tools: if tools.is_empty() { None } else { Some(tools) },
            max_tokens,
            temperature: request.temperature,
            stream: false,
        })
""",
        """        Ok(AnthropicRequest {
            model: request.model.clone(),
            messages,
            system: request.system_prompt.clone(),
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(serde_json::json!({"type": "auto"}))
            },
            max_tokens,
            temperature: request.temperature,
            stream: false,
        })
""",
    )

    # AnthropicRequest struct
    text = text.replace(
        """struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}
""",
        """struct AnthropicRequest {
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
""",
    )

    # Replace duplicated HTTP error handling in send_request with map_error_response
    # Keep simple by replacing the big status blocks with helper.
    import re

    send_request_pattern = re.compile(
        r"async fn send_request\(&self, body: &AnthropicRequest\) -> Result<ApiResponse, ApiError> \{.*?\n    \}\n\n    /// Send a streaming",
        re.S,
    )
    send_request_new = '''async fn send_request(&self, body: &AnthropicRequest) -> Result<ApiResponse, ApiError> {
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

        let response_body: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| ApiError::InvalidResponse(format!("Failed to parse response JSON: {e}")))?;
        Ok(self.convert_response(response_body))
    }

    /// Send a streaming'''
    text2, n = send_request_pattern.subn(send_request_new, text, count=1)
    if n != 1:
        print("WARN: anthropic send_request not replaced", n)
    else:
        text = text2

    # streaming HTTP error mapping
    text = text.replace(
        """        let status = response.status();

        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(30);
            return Err(ApiError::RateLimited { retry_after_secs: retry_after });
        }

        if status.as_u16() == 401 || status.as_u16() == 403 {
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::Auth(text));
        }

        if status.as_u16() == 404 {
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::ModelNotFound(text));
        }

        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::Http { status: status.as_u16(), body: text });
        }

        // Stream the response body
""",
        """        if !response.status().is_success() {
            return Err(map_error_response(response).await);
        }

        // Stream the response body
""",
    )

    # message_start usage input_tokens if present
    if "message_start" in text and "input_tokens" not in text.split("message_start")[1][:500]:
        text = text.replace(
            '''                    "message_start" => {
                        if let Ok(msg_start) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(m) = msg_start.get("message") {
                                if let Some(mdl) = m.get("model").and_then(|v| v.as_str()) {
                                    model_name = mdl.to_string();
                                }
                            }
                        }
                    }
''',
            '''                    "message_start" => {
                        if let Ok(msg_start) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(m) = msg_start.get("message") {
                                if let Some(mdl) = m.get("model").and_then(|v| v.as_str()) {
                                    model_name = mdl.to_string();
                                }
                                if let Some(u) = m.get("usage") {
                                    let existing = usage.get_or_insert(TokenUsage::default());
                                    if let Some(input) = u.get("input_tokens").and_then(|v| v.as_u64()) {
                                        existing.input_tokens = input;
                                    }
                                    if let Some(cached) = u
                                        .get("cache_read_input_tokens")
                                        .and_then(|v| v.as_u64())
                                    {
                                        existing.cache_read_tokens = Some(cached);
                                    }
                                }
                            }
                        }
                    }
''',
        )

    # retry loops use is_non_retryable
    text = text.replace(
        """                match self.send_request(&body).await {
                    Ok(response) => return Ok(response),
                    Err(e) => {
                        match &e {
                            ApiError::Auth(_) | ApiError::ModelNotFound(_) => return Err(e),
                            ApiError::RateLimited { .. } => {
                                warn!("Rate limited, will retry");
                                last_error = Some(e);
                            }
                            _ => {
                                warn!(error = %e, attempt, "Request failed, will retry");
                                last_error = Some(e);
                            }
                        }
                    }
                }
""",
        """                match self.send_request(&body).await {
                    Ok(response) => return Ok(response),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Anthropic request failed, will retry");
                        last_error = Some(e);
                    }
                }
""",
    )

    # streaming retry
    text = text.replace(
        """                match self.send_streaming_request(&body, callback).await {
                    Ok(response) => return Ok(response),
                    Err(e) => {
                        match &e {
                            ApiError::Auth(_) | ApiError::ModelNotFound(_) => return Err(e),
                            ApiError::RateLimited { .. } => {
                                warn!("Rate limited, will retry");
                                last_error = Some(e);
                            }
                            _ => {
                                warn!(error = %e, attempt, "Streaming request failed, will retry");
                                last_error = Some(e);
                            }
                        }
                    }
                }
""",
        """                match self.send_streaming_request(&body, callback).await {
                    Ok(response) => return Ok(response),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Anthropic stream failed, will retry");
                        last_error = Some(e);
                    }
                }
""",
    )

    # network_err helper at end before tests or at end of file
    if "fn network_err(" not in text:
        text += """

fn network_err(e: reqwest::Error) -> ApiError {
    if e.is_timeout() {
        ApiError::Network(format!("Request timed out: {e}"))
    } else if e.is_connect() {
        ApiError::Network(format!("Connection failed: {e}"))
    } else {
        ApiError::Network(e.to_string())
    }
}
"""

    # add unit tests module
    if "mod tests" not in text:
        text += """

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pigs_core::ToolSpec;

    #[test]
    fn endpoint_handles_base_with_v1() {
        let c = AnthropicClient::new("k", "claude-sonnet-4-20250514", "https://api.anthropic.com/v1");
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
"""

    # fix streaming send path if still has old map_err chains using ApiError::Network directly
    # Also need futures_util import if not present
    if "use futures_util::StreamExt" not in text and "StreamExt" in text:
        text = text.replace(
            "use std::time::Duration;\n",
            "use std::time::Duration;\n\nuse futures_util::StreamExt;\n",
        )

    path.write_text(text, encoding="utf-8")
    print("anthropic.rs patched")


def main() -> None:
    patch_lib()
    rewrite_openai_chat()
    patch_openai_responses()
    patch_anthropic()
    print("done")


if __name__ == "__main__":
    main()
