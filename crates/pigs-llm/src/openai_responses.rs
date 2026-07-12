//! OpenAI Responses API client (`POST /v1/responses`).
//!
//! This is the current OpenAI agent wire format used by Codex (`WireApi::Responses`).
//! Chat Completions remains available separately for third-party compatible endpoints.
//!
//! Reference implementations consulted:
//! - codex `codex-rs/codex-api` (`ResponsesApiRequest`, SSE event processing)
//! - pi `packages/ai/src/api/openai-responses.ts` (store:false, max_output_tokens, tools)

use std::time::Duration;

use futures_util::StreamExt;
use pigs_core::{
    ApiClient, ApiError, ApiFuture, ApiRequest, ApiResponse, ContentBlock, Message, MessageRole,
    StreamCallback, StreamEvent, TokenUsage,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::http_util::{is_non_retryable, join_url, map_error_response, retry_after_secs};

/// OpenAI Responses API client.
pub struct OpenAiResponsesClient {
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
    max_retries: u32,
}

impl OpenAiResponsesClient {
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
        // Accept both ".../v1" and bare host; always call /responses relative to base.
        if self.base_url.ends_with("/responses") {
            self.base_url.clone()
        } else {
            join_url(&self.base_url, "responses")
        }
    }

    fn build_request_body(&self, request: &ApiRequest, stream: bool) -> ResponsesRequest {
        let mut input: Vec<ResponsesInputItem> = Vec::new();

        for msg in &request.messages {
            if msg.role == MessageRole::System {
                // System text is carried via `instructions`.
                continue;
            }
            extend_input_from_message(&mut input, msg);
        }

        let tools: Vec<ResponsesTool> = request
            .tools
            .iter()
            .map(|t| ResponsesTool {
                r#type: "function".into(),
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
                strict: false,
            })
            .collect();

        ResponsesRequest {
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
    }

    async fn map_http_error(&self, response: reqwest::Response) -> ApiError {
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

    async fn send_once(&self, body: &ResponsesRequest) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "responses=v1")
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
            return Err(self.map_http_error(response).await);
        }

        let parsed: ResponsesResponse = response
            .json()
            .await
            .map_err(|e| ApiError::InvalidResponse(format!("Failed to parse response JSON: {e}")))?;

        Ok(convert_non_stream_response(parsed))
    }

    async fn send_streaming_once(
        &self,
        body: &ResponsesRequest,
        callback: &dyn StreamCallback,
    ) -> Result<ApiResponse, ApiError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .header("OpenAI-Beta", "responses=v1")
            .header(reqwest::header::ACCEPT, "text/event-stream")
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
            return Err(self.map_http_error(response).await);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content: Vec<ContentBlock> = Vec::new();
        let mut text_acc = String::new();
        // call_id -> (name, arguments)
        let mut tool_args: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        let mut usage: Option<TokenUsage> = None;
        let mut stop_reason: Option<String> = None;
        let mut response_model = self.model.clone();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| ApiError::Network(format!("SSE read error: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find("\n\n") {
                let frame = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                let mut event_name = String::new();
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(rest) = line.strip_prefix("event:") {
                        event_name = rest.trim().to_string();
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(rest.trim_start());
                    }
                }
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }

                let value: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(error = %e, "skip unparseable SSE data");
                        continue;
                    }
                };

                // Some servers put type only in JSON; prefer event: line when present.
                let kind = if event_name.is_empty() {
                    value
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    event_name
                };

                match kind.as_str() {
                    "response.output_text.delta" => {
                        if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                            text_acc.push_str(delta);
                            callback.on_event(&StreamEvent::TextDelta(delta.to_string()));
                        }
                    }
                    "response.function_call_arguments.done" => {
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
                    "response.function_call_arguments.delta" => {
                        let call_id = value
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .or_else(|| value.get("item_id").and_then(|v| v.as_str()))
                            .unwrap_or("")
                            .to_string();
                        if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                            let entry = tool_args
                                .entry(call_id.clone())
                                .or_insert_with(|| (String::new(), String::new()));
                            entry.1.push_str(delta);
                            if !call_id.is_empty() {
                                callback.on_event(&StreamEvent::ToolUseInputDelta {
                                    id: call_id,
                                    partial_json: delta.to_string(),
                                });
                            }
                        }
                    }
                    "response.output_item.added" | "response.output_item.done" => {
                        if let Some(item) = value.get("item") {
                            handle_output_item(
                                item,
                                &mut content,
                                &mut text_acc,
                                &mut tool_args,
                                callback,
                                kind == "response.output_item.added",
                            );
                        }
                    }
                    "response.completed" => {
                        if let Some(resp) = value.get("response") {
                            if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                                response_model = m.to_string();
                            }
                            if let Some(u) = resp.get("usage") {
                                usage = Some(parse_usage(u));
                            }
                            if let Some(status) = resp.get("status").and_then(|v| v.as_str()) {
                                stop_reason = Some(status.to_string());
                            }
                        }
                        if let Some(u) = value.get("usage") {
                            usage = Some(parse_usage(u));
                        }
                    }
                    "response.failed" => {
                        let msg = value
                            .pointer("/response/error/message")
                            .and_then(|v| v.as_str())
                            .or_else(|| value.pointer("/error/message").and_then(|v| v.as_str()))
                            .unwrap_or("response.failed");
                        let lower = msg.to_ascii_lowercase();
                        if lower.contains("context") && lower.contains("window") {
                            return Err(ApiError::ContextWindowExceeded(msg.to_string()));
                        }
                        return Err(ApiError::Request(msg.to_string()));
                    }
                    "response.incomplete" => {
                        let reason = value
                            .pointer("/response/incomplete_details/reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        return Err(ApiError::Request(format!(
                            "Incomplete response, reason: {reason}"
                        )));
                    }
                    _ => {
                        // ignore response.created and other events
                    }
                }
            }
        }

        // Flush accumulated plain text if not already pushed as ContentBlock::Text
        if !text_acc.is_empty() && !content.iter().any(|b| matches!(b, ContentBlock::Text { .. })) {
            content.insert(0, ContentBlock::text(text_acc));
        } else if !text_acc.is_empty() {
            // If we only streamed deltas and never got a final message item, ensure text exists.
            if content.is_empty() {
                content.push(ContentBlock::text(text_acc));
            }
        }

        // Emit tool end events for completed tools
        for (id, (name, args)) in &tool_args {
            if content.iter().any(|b| matches!(b, ContentBlock::ToolUse { id: tid, .. } if tid == id))
            {
                continue;
            }
            if name.is_empty() && args.is_empty() {
                continue;
            }
            let input = serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));
            content.push(ContentBlock::tool_use(id, name, input));
            callback.on_event(&StreamEvent::ToolUseEnd { id: id.clone() });
        }

        if let Some(u) = &usage {
            callback.on_event(&StreamEvent::Usage(u.clone()));
        }
        callback.on_event(&StreamEvent::Done {
            stop_reason: stop_reason.clone(),
        });

        Ok(ApiResponse {
            content,
            usage,
            model: response_model,
            stop_reason,
        })
    }
}

impl ApiClient for OpenAiResponsesClient {
    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            let body = self.build_request_body(&request, false);
            let mut last_error: Option<ApiError> = None;
            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying OpenAI Responses request");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }
                match self.send_once(&body).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Responses request failed, will retry");
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
            let mut last_error: Option<ApiError> = None;
            for attempt in 0..=self.max_retries {
                if attempt > 0 {
                    debug!(attempt, "Retrying OpenAI Responses streaming request");
                    tokio::time::sleep(Duration::from_millis(500u64 * 2u64.pow(attempt))).await;
                }
                match self.send_streaming_once(&body, callback).await {
                    Ok(r) => return Ok(r),
                    Err(e) if is_non_retryable(&e) => return Err(e),
                    Err(e) => {
                        warn!(error = %e, attempt, "Responses stream failed, will retry");
                        last_error = Some(e);
                    }
                }
            }
            Err(last_error
                .unwrap_or_else(|| ApiError::Request("All streaming retries exhausted".into())))
        })
    }
}

fn extend_input_from_message(input: &mut Vec<ResponsesInputItem>, msg: &Message) {
    match msg.role {
        MessageRole::User => {
            let text = msg.text_content();
            if !text.is_empty() {
                input.push(ResponsesInputItem::Message {
                    r#type: "message".into(),
                    role: "user".into(),
                    content: vec![ResponsesContentPart::InputText {
                        r#type: "input_text".into(),
                        text,
                    }],
                });
            }
        }
        MessageRole::Assistant => {
            let text = msg.text_content();
            if !text.is_empty() {
                input.push(ResponsesInputItem::Message {
                    r#type: "message".into(),
                    role: "assistant".into(),
                    content: vec![ResponsesContentPart::OutputText {
                        r#type: "output_text".into(),
                        text,
                    }],
                });
            }
            for (id, name, args) in msg.tool_uses() {
                let arguments =
                    serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
                input.push(ResponsesInputItem::FunctionCall {
                    r#type: "function_call".into(),
                    name: name.to_string(),
                    arguments,
                    call_id: id.to_string(),
                });
            }
        }
        MessageRole::Tool => {
            for block in &msg.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    ..
                } = block
                {
                    input.push(ResponsesInputItem::FunctionCallOutput {
                        r#type: "function_call_output".into(),
                        call_id: tool_use_id.clone(),
                        output: output.clone(),
                    });
                }
            }
        }
        MessageRole::System => {}
    }
}

fn handle_output_item(
    item: &serde_json::Value,
    content: &mut Vec<ContentBlock>,
    text_acc: &mut String,
    tool_args: &mut std::collections::HashMap<String, (String, String)>,
    callback: &dyn StreamCallback,
    is_added: bool,
) {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "message" => {
            // Prefer structured output_text parts; fall back to accumulated deltas.
            let mut parts = String::new();
            if let Some(arr) = item.get("content").and_then(|v| v.as_array()) {
                for part in arr {
                    let t = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if t == "output_text" {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            parts.push_str(text);
                        }
                    }
                }
            }
            if !parts.is_empty() {
                if text_acc.is_empty() {
                    *text_acc = parts.clone();
                }
                if !content.iter().any(|b| matches!(b, ContentBlock::Text { .. })) {
                    content.push(ContentBlock::text(parts));
                }
            }
        }
        "function_call" => {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = item
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if call_id.is_empty() {
                return;
            }
            let entry = tool_args
                .entry(call_id.clone())
                .or_insert_with(|| (String::new(), String::new()));
            if !name.is_empty() {
                entry.0 = name;
            }
            if !arguments.is_empty() {
                entry.1 = arguments;
            }
            if is_added {
                callback.on_event(&StreamEvent::ToolUseStart {
                    id: call_id,
                    name: entry.0.clone(),
                });
            } else {
                let input =
                    serde_json::from_str(&entry.1).unwrap_or_else(|_| serde_json::json!({}));
                if !content.iter().any(|b| {
                    matches!(b, ContentBlock::ToolUse { id, .. } if id == &call_id)
                }) {
                    content.push(ContentBlock::tool_use(&call_id, &entry.0, input));
                }
                callback.on_event(&StreamEvent::ToolUseEnd { id: call_id });
            }
        }
        _ => {}
    }
}

fn convert_non_stream_response(resp: ResponsesResponse) -> ApiResponse {
    let mut content = Vec::new();
    for item in resp.output {
        match item {
            ResponsesOutputItem::Message { content: parts, .. } => {
                let mut text = String::new();
                for p in parts {
                    if let ResponsesContentPart::OutputText { text: t, .. } = p {
                        text.push_str(&t);
                    } else if let ResponsesContentPart::InputText { text: t, .. } = p {
                        text.push_str(&t);
                    }
                }
                if !text.is_empty() {
                    content.push(ContentBlock::text(text));
                }
            }
            ResponsesOutputItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let input =
                    serde_json::from_str(&arguments).unwrap_or_else(|_| serde_json::json!({}));
                content.push(ContentBlock::tool_use(call_id, name, input));
            }
            ResponsesOutputItem::Other => {}
        }
    }

    let usage = resp.usage.map(|u| TokenUsage {
        input_tokens: u.input_tokens.unwrap_or(0),
        output_tokens: u.output_tokens.unwrap_or(0),
        cache_read_tokens: u.input_tokens_details.and_then(|d| d.cached_tokens),
        total_cost: None,
    });

    ApiResponse {
        content,
        usage,
        model: resp.model.unwrap_or_default(),
        stop_reason: resp.status,
    }
}

fn parse_usage(u: &serde_json::Value) -> TokenUsage {
    TokenUsage {
        input_tokens: u
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| u.get("prompt_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        output_tokens: u
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| u.get("completion_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        cache_read_tokens: u
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(|v| v.as_u64()),
        total_cost: None,
    }
}

// --- Wire types ---

#[derive(Debug, Serialize)]
struct ResponsesRequest {
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

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ResponsesInputItem {
    Message {
        r#type: String,
        role: String,
        content: Vec<ResponsesContentPart>,
    },
    FunctionCall {
        r#type: String,
        name: String,
        arguments: String,
        call_id: String,
    },
    FunctionCallOutput {
        r#type: String,
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ResponsesContentPart {
    InputText {
        r#type: String,
        text: String,
    },
    OutputText {
        r#type: String,
        text: String,
    },
}

#[derive(Debug, Serialize)]
struct ResponsesTool {
    r#type: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
    strict: bool,
}

#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponsesOutputItem {
    Message {
        #[serde(default)]
        content: Vec<ResponsesContentPart>,
    },
    FunctionCall {
        name: String,
        arguments: String,
        call_id: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    input_tokens_details: Option<ResponsesUsageDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsageDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pigs_core::ToolSpec;

    #[test]
    fn builds_responses_body_with_function_call_output() {
        let client = OpenAiResponsesClient::new("k", "gpt-4o", "https://api.openai.com/v1");
        let messages = vec![
            Message::user("hi"),
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::tool_use(
                    "call_1",
                    "bash",
                    serde_json::json!({"command": "ls"}),
                )],
                usage: None,
            },
            Message {
                role: MessageRole::Tool,
                content: vec![ContentBlock::tool_result("call_1", "ok", false)],
                usage: None,
            },
        ];
        let req = ApiRequest::new("gpt-4o", messages)
            .with_system_prompt("you are pigs")
            .with_tools(vec![ToolSpec::new(
                "bash",
                "run shell",
                serde_json::json!({"type":"object"}),
            )]);
        let body = client.build_request_body(&req, true);
        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.instructions, "you are pigs");
        assert!(body.stream);
        assert_eq!(body.input.len(), 3);
        assert!(body.tools.as_ref().unwrap().iter().any(|t| t.name == "bash"));
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("input").is_some());
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["store"], false);
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn endpoint_appends_responses() {
        let c = OpenAiResponsesClient::new("k", "m", "https://api.openai.com/v1");
        assert_eq!(c.endpoint(), "https://api.openai.com/v1/responses");
    }
}
