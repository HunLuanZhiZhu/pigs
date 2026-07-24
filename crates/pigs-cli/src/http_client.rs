//! HTTP client that connects to the local pigs-proxy API server.
//!
//! Replaces `ProxyApiClient` (in-process `dispatch_in_process`) with real
//! HTTP requests to `http://127.0.0.1:{port}/chat/completions`. Supports
//! true SSE streaming — `send_message_streaming` sends `stream: true` and
//! parses the SSE frames incrementally, calling back `StreamEvent` for each
//! delta.

use futures_util::StreamExt;
use pigs_core::{
    ApiClient, ApiError, ApiFuture, ApiRequest, ApiResponse, ContentBlock, MessageRole,
    StreamCallback, StreamEvent, TokenUsage, ToolSpec,
};
use serde_json::{json, Value};

/// HTTP client connecting to the local pigs-proxy API server.
pub struct HttpAgentClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    /// API key injected into the Authorization header (passthrough to upstream via proxy).
    api_key: Option<String>,
}

impl HttpAgentClient {
    /// Creates a client targeting `http://{host}:{port}/chat/completions`.
    /// When `api_key` is `Some`, it is sent as `Authorization: Bearer {key}` on every
    /// request. The proxy (with `key_mode = "passthrough"`) forwards it to the upstream LLM.
    pub fn new(host: &str, port: u16, model: String, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            base_url: format!("http://{host}:{port}"),
            model,
            api_key,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// Apply the Authorization header if an api_key is configured.
    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            if !key.is_empty() {
                return req.header("Authorization", format!("Bearer {key}"));
            }
        }
        req
    }
}

impl ApiClient for HttpAgentClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            let body = build_chat_body(&request, false);
            let resp = self
                .add_auth(
                    self.client
                        .post(self.endpoint())
                        .json(&body),
                )
                .send()
                .await
                .map_err(|e| ApiError::Network(e.to_string()))?;
            let status = resp.status();
            let text = resp
                .text()
                .await
                .map_err(|e| ApiError::Network(e.to_string()))?;
            if !status.is_success() {
                return Err(ApiError::Http {
                    status: status.as_u16(),
                    body: text,
                });
            }
            let json: Value = serde_json::from_str(&text)
                .map_err(|e| ApiError::InvalidResponse(format!("invalid JSON: {e}")))?;
            parse_chat_response(&json, &request.model)
        })
    }

    fn send_message_streaming<'a>(
        &'a self,
        request: ApiRequest,
        callback: &'a dyn StreamCallback,
    ) -> ApiFuture<'a> {
        Box::pin(async move {
            let body = build_chat_body(&request, true);
            let resp = self
                .add_auth(
                    self.client
                        .post(self.endpoint())
                        .json(&body),
                )
                .send()
                .await
                .map_err(|e| ApiError::Network(e.to_string()))?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ApiError::Network(e.to_string()))?;
                return Err(ApiError::Http {
                    status: status.as_u16(),
                    body: text,
                });
            }

            let mut stream = resp.bytes_stream();
            let mut pending = String::new();
            let mut full_text = String::new();
            let mut tool_calls: Vec<Value> = Vec::new();
            let mut stop_reason: Option<String> = None;
            let mut usage: Option<TokenUsage> = None;
            let mut model_out = String::new();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| ApiError::Network(e.to_string()))?;
                pending.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(frame) = take_sse_frame(&pending) {
                    let consumed = frame.consumed;
                    let data = frame.data;
                    pending.drain(..consumed);
                    if data.trim() == "[DONE]" {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(u) = value.get("usage") {
                        usage = parse_usage(u);
                    }
                    if let Some(id) = value.get("model").and_then(Value::as_str) {
                        if model_out.is_empty() {
                            model_out = id.to_string();
                        }
                    }
                    let Some(choice) = value
                        .get("choices")
                        .and_then(Value::as_array)
                        .and_then(|c| c.first())
                    else {
                        continue;
                    };
                    if let Some(reason) = choice
                        .get("finish_reason")
                        .filter(|v| !v.is_null())
                        .and_then(Value::as_str)
                    {
                        stop_reason = Some(reason.to_string());
                    }
                    let delta = choice.get("delta");
                    if let Some(content) =
                        delta.and_then(|d| d.get("content")).and_then(Value::as_str)
                    {
                        if !content.is_empty() {
                            full_text.push_str(content);
                            callback.on_event(&StreamEvent::TextDelta(content.to_string()));
                        }
                    }
                    if let Some(calls) = delta
                        .and_then(|d| d.get("tool_calls"))
                        .and_then(Value::as_array)
                    {
                        for call in calls {
                            merge_tool_call_delta(&mut tool_calls, call);
                        }
                    }
                }
            }

            // Emit tool_use events for accumulated tool calls.
            for call in &tool_calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let function = call.get("function").unwrap_or(&Value::Null);
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args_str = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}")
                    .to_string();
                callback.on_event(&StreamEvent::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                });
                if !args_str.is_empty() {
                    callback.on_event(&StreamEvent::ToolUseInputDelta {
                        id: id.clone(),
                        partial_json: args_str,
                    });
                }
                callback.on_event(&StreamEvent::ToolUseEnd { id });
            }

            if let Some(u) = &usage {
                callback.on_event(&StreamEvent::Usage(u.clone()));
            }
            callback.on_event(&StreamEvent::Done {
                stop_reason: stop_reason.clone(),
            });

            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            if !full_text.is_empty() {
                content_blocks.push(ContentBlock::text(&full_text));
            }
            for call in &tool_calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let function = call.get("function").unwrap_or(&Value::Null);
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args_str = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                content_blocks.push(ContentBlock::tool_use(&id, &name, input));
            }

            Ok(ApiResponse {
                content: content_blocks,
                usage,
                model: if model_out.is_empty() {
                    request.model
                } else {
                    model_out
                },
                stop_reason,
            })
        })
    }
}

struct SseFrame {
    data: String,
    consumed: usize,
}

fn take_sse_frame(buffer: &str) -> Option<SseFrame> {
    let lf = buffer.find("\n\n").map(|i| (i, 2));
    let crlf = buffer.find("\r\n\r\n").map(|i| (i, 4));
    let (idx, sep) = match (lf, crlf) {
        (Some(l), Some(r)) if l.0 <= r.0 => l,
        (Some(_), Some(r)) => r,
        (Some(l), None) => l,
        (None, Some(r)) => r,
        (None, None) => return None,
    };
    let frame = buffer[..idx].replace('\r', "");
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: ").map(str::to_owned))
        .or_else(|| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data:").map(str::to_owned))
        })?;
    Some(SseFrame {
        data,
        consumed: idx + sep,
    })
}

fn merge_tool_call_delta(calls: &mut Vec<Value>, delta: &Value) {
    let index = delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    while calls.len() <= index {
        calls.push(json!({
            "id": "", "type": "function",
            "function": {"name": "", "arguments": ""}
        }));
    }
    let target = &mut calls[index];
    if let Some(id) = delta.get("id").and_then(Value::as_str) {
        if !id.is_empty() {
            target["id"] = json!(id);
        }
    }
    if let Some(func) = delta.get("function") {
        if let Some(name) = func.get("name").and_then(Value::as_str) {
            if !name.is_empty() {
                target["function"]["name"] = json!(name);
            }
        }
        if let Some(args) = func.get("arguments").and_then(Value::as_str) {
            let current = target["function"]["arguments"].as_str().unwrap_or("");
            target["function"]["arguments"] = json!(format!("{current}{args}"));
        }
    }
}

fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64),
        total_cost: None,
    })
}

fn build_chat_body(request: &ApiRequest, stream: bool) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(system) = &request.system_prompt {
        if !system.is_empty() {
            messages.push(json!({"role": "system", "content": system}));
        }
    }

    for msg in &request.messages {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };

        let text: String = msg.text_content();
        let tool_uses: Vec<&ContentBlock> = msg
            .content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect();
        let tool_results: Vec<&ContentBlock> = msg
            .content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .collect();

        if let Some(result) = tool_results.first() {
            if let ContentBlock::ToolResult {
                tool_use_id,
                output,
                is_error,
            } = result
            {
                let mut tool_msg = json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": output,
                });
                if *is_error {
                    tool_msg["is_error"] = json!(true);
                }
                messages.push(tool_msg);
                continue;
            }
        }

        if !tool_uses.is_empty() {
            let calls: Vec<Value> = tool_uses
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolUse { id, name, input } = b {
                        Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": input.to_string(),
                            }
                        }))
                    } else {
                        None
                    }
                })
                .collect();
            let content_val = if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            };
            messages.push(json!({
                "role": role,
                "content": content_val,
                "tool_calls": calls,
            }));
        } else {
            messages.push(json!({"role": role, "content": text}));
        }
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
    });

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t: &ToolSpec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }

    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = request.temperature {
        body["temperature"] = json!(temp);
    }

    body
}

fn parse_chat_response(json: &Value, model: &str) -> Result<ApiResponse, ApiError> {
    if let Some(err) = json.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(ApiError::InvalidResponse(msg.to_string()));
    }

    let choices = json
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::InvalidResponse("missing choices array".into()))?;

    let mut content_blocks: Vec<ContentBlock> = Vec::new();
    let mut stop_reason: Option<String> = None;

    for choice in choices {
        if stop_reason.is_none() {
            stop_reason = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(String::from);
        }
        let message = match choice.get("message") {
            Some(m) => m,
            None => continue,
        };
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                content_blocks.push(ContentBlock::text(text));
            }
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let function = call.get("function").unwrap_or(&Value::Null);
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args_str = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                content_blocks.push(ContentBlock::tool_use(&id, &name, input));
            }
        }
    }

    let usage = json.get("usage").map(|u| TokenUsage {
        input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: u
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64),
        total_cost: None,
    });

    let model_out = json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(model)
        .to_string();

    Ok(ApiResponse {
        content: content_blocks,
        usage,
        model: model_out,
        stop_reason,
    })
}
