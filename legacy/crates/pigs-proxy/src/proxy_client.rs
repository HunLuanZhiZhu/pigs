//! ProxyApiClient — 通过 pigs-proxy 的进程内调度实现 ApiClient trait。
//! ProxyApiClient — implements ApiClient trait via pigs-proxy in-process dispatch.
//!
//! 不直接连上游 LLM，也不走 HTTP loopback；
//! 而是把 `ApiRequest` 转成 JSON，调用 `pigs_proxy::dispatch_in_process`，
//! 自动享受重试 + body 清洗 + 思考强度注入。
//!
//! Does not connect to upstream LLM directly, nor via HTTP loopback;
//! instead, converts `ApiRequest` to JSON and calls `pigs_proxy::dispatch_in_process`,
//! automatically benefiting from retry + body cleaning + thinking-effort injection.

use std::sync::Arc;

use crate::config::Config as ProxyConfig;
use crate::protocol::Protocol;
use crate::upstream::UpstreamClient;
use pigs_core::{
    ApiClient, ApiError, ApiFuture, ApiRequest, ApiResponse, ContentBlock, MessageRole,
    StreamCallback, StreamEvent, TokenUsage,
};

/// 通过 pigs-proxy 进程内调度的 ApiClient 实现。
/// ApiClient implementation that dispatches via pigs-proxy in-process.
pub struct ProxyApiClient {
    /// 代理配置（含 provider/endpoint 信息）。
    /// Proxy configuration (providers, endpoints).
    config: Arc<ProxyConfig>,
    /// 上游 HTTP 客户端（透传 + 重试用）。
    /// Upstream HTTP client (for passthrough + retry).
    client: Arc<UpstreamClient>,
    /// 请求协议（决定上游 URL 后缀）。
    /// Request protocol (determines upstream URL suffix).
    protocol: Protocol,
    /// 模型 ID（不含 -pig 后缀，用于端点匹配和日志）。
    /// Model ID (without -pig suffix, for endpoint matching and logging).
    model: String,
}

impl ProxyApiClient {
    /// 创建新的 ProxyApiClient。
    /// Create a new ProxyApiClient.
    ///
    /// # 参数 / Parameters
    /// - `config`: pigs-proxy 配置
    /// - `protocol`: 请求协议
    /// - `model`: 模型 ID（不含 -pig）
    pub fn new(config: Arc<ProxyConfig>, protocol: Protocol, model: String) -> Self {
        Self {
            config,
            client: Arc::new(UpstreamClient::new()),
            protocol,
            model,
        }
    }
}

impl ApiClient for ProxyApiClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn send_message<'a>(&'a self, request: ApiRequest) -> ApiFuture<'a> {
        Box::pin(async move {
            // 把 ApiRequest 转成 OpenAI Chat 格式 JSON body
            // Convert ApiRequest to OpenAI Chat format JSON body
            let body = build_openai_chat_body(&request);

            // 进程内调度（走 pigs-proxy 重试逻辑）
            // In-process dispatch (goes through pigs-proxy retry logic)
            let resp_json = crate::dispatch_in_process(
                &self.config,
                &self.client,
                self.protocol,
                &request.model,
                &body,
            )
            .await
            .map_err(|e| ApiError::Network(e))?;

            // 从上游响应 JSON 解析出 ApiResponse
            // Parse ApiResponse from upstream response JSON
            parse_openai_chat_response(&resp_json, &request.model)
        })
    }

    fn send_message_streaming<'a>(
        &'a self,
        request: ApiRequest,
        callback: &'a dyn StreamCallback,
    ) -> ApiFuture<'a> {
        Box::pin(async move {
            // ProxyApiClient 不支持真正的流式——上游响应是非流式的。
            // 用默认行为：拿到完整响应后一次性发 TextDelta + Done。
            // ProxyApiClient doesn't support true streaming — upstream response is non-streaming.
            // Default behavior: emit TextDelta + Done after receiving the full response.
            let body = build_openai_chat_body(&request);

            // 进程内需要禁用流式（确保上游返回非流式响应）
            // Ensure non-streaming response from upstream
            let mut body = body;
            if let Some(obj) = body.as_object_mut() {
                obj.insert("stream".into(), serde_json::Value::Bool(false));
            }

            let resp_json = crate::dispatch_in_process(
                &self.config,
                &self.client,
                self.protocol,
                &request.model,
                &body,
            )
            .await
            .map_err(|e| ApiError::Network(e))?;

            let response = parse_openai_chat_response(&resp_json, &request.model)?;

            // 发 TextDelta 事件 / Emit TextDelta events
            let text = response.text_content();
            if !text.is_empty() {
                callback.on_event(&StreamEvent::TextDelta(text));
            }

            // 发 ToolUse 事件 / Emit ToolUse events
            for block in &response.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    callback.on_event(&StreamEvent::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    callback.on_event(&StreamEvent::ToolUseInputDelta {
                        id: id.clone(),
                        partial_json: input.to_string(),
                    });
                    callback.on_event(&StreamEvent::ToolUseEnd { id: id.clone() });
                }
            }

            // 发 Done 事件 / Emit Done event
            callback.on_event(&StreamEvent::Done {
                stop_reason: response.stop_reason.clone(),
            });

            Ok(response)
        })
    }
}

/// 把 ApiRequest 转成 OpenAI Chat Completions 格式的 JSON body。
/// Convert ApiRequest to OpenAI Chat Completions format JSON body.
fn build_openai_chat_body(request: &ApiRequest) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    // system prompt → messages[0] role=system
    if let Some(system) = &request.system_prompt {
        if !system.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }
    }

    // 转换 messages / Convert messages
    for msg in &request.messages {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let text: String = msg
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("\n");

        let tool_calls: Vec<serde_json::Value> = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": input.to_string()
                    }
                })),
                _ => None,
            })
            .collect();

        let tool_result = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    ..
                } => Some((tool_use_id.clone(), output.clone())),
                _ => None,
            })
            .next();

        if let Some((tool_use_id, output)) = tool_result {
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": tool_use_id,
                "content": output
            }));
        } else if !tool_calls.is_empty() {
            messages.push(serde_json::json!({
                "role": role,
                "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) },
                "tool_calls": tool_calls
            }));
        } else {
            messages.push(serde_json::json!({
                "role": role,
                "content": text
            }));
        }
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
    });

    // tools / 工具定义
    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(tools);
        body["tool_choice"] = serde_json::json!("auto");
    }

    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::Value::Number(max_tokens.into());
    }

    if let Some(temperature) = request.temperature {
        body["temperature"] = serde_json::Value::Number(
            serde_json::Number::from_f64(temperature as f64)
                .unwrap_or_else(|| serde_json::Number::from(0)),
        );
    }

    body
}

/// 从 OpenAI Chat Completions 响应 JSON 解析出 ApiResponse。
/// Parse an ApiResponse from an OpenAI Chat Completions response JSON.
fn parse_openai_chat_response(
    json: &serde_json::Value,
    model: &str,
) -> Result<ApiResponse, ApiError> {
    // 检查错误 / Check for error
    if let Some(error) = json.get("error") {
        let msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(ApiError::InvalidResponse(msg.to_string()));
    }

    let mut content: Vec<ContentBlock> = Vec::new();
    let mut stop_reason: Option<String> = None;
    let mut response_model: String = model.to_string();

    if let Some(choices) = json.get("choices").and_then(|v| v.as_array()) {
        for choice in choices {
            // finish_reason
            if stop_reason.is_none() {
                stop_reason = choice
                    .get("finish_reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }

            // message content
            if let Some(message) = choice.get("message") {
                if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        content.push(ContentBlock::text(text));
                    }
                }

                // tool_calls
                if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let function = tc.get("function").cloned().unwrap_or_default();
                        let name = function
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments_str = function
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let input: serde_json::Value =
                            serde_json::from_str(arguments_str).unwrap_or(serde_json::Value::Null);
                        content.push(ContentBlock::tool_use(&id, &name, input));
                    }
                }
            }
        }
    }

    // model 字段
    if let Some(m) = json.get("model").and_then(|v| v.as_str()) {
        response_model = m.to_string();
    }

    // usage
    let usage = json.get("usage").map(|u| {
        let prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let completion_tokens = u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_read_tokens = u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64());
        TokenUsage {
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
            cache_read_tokens,
            total_cost: None,
        }
    });

    Ok(ApiResponse {
        content,
        usage,
        model: response_model,
        stop_reason,
    })
}

// 避免 unused import warning / Avoid unused import warnings
#[allow(unused_imports)]
use pigs_core::ToolResult;
