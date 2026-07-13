//! 三种 API 格式的请求解析与响应构造。
//! Request parsing and response construction for three API formats.
//!
//! pigs-api 支持三种上游 API 格式，输入什么格式就输出什么格式：
//! pigs-api supports three upstream API formats — what comes in is what goes out:
//!
//! | 路径 / Path          | 格式 / Format        |
//! |----------------------|-----------------------|
//! | `/chat/completions`  | OpenAI Chat Completions |
//! | `/v1/messages`       | Anthropic Messages    |
//! | `/responses`         | OpenAI Responses      |
//!
//! 内部统一用 `pigs_core::Message` 数组，三种格式各自负责：
//! Internally all messages are `pigs_core::Message`; each format handles:
//! - 解析请求体 → `ConvertedTurn`（提取 system + history + user question）
//! - 构造非流式响应 JSON
//! - 构造 SSE 流式帧（role chunk / content chunk / stop chunk）

use serde_json::Value;

use crate::phased_api_convert::{ConvertError, ConvertedTurn};
use crate::phased_runtime::TurnResult;

/// 三种支持的 API 格式。
/// Three supported API formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    /// OpenAI Chat Completions (`/chat/completions`)。
    /// OpenAI Chat Completions (`/chat/completions`).
    OpenAIChat,
    /// Anthropic Messages (`/v1/messages`)。
    /// Anthropic Messages (`/v1/messages`).
    Anthropic,
    /// OpenAI Responses (`/responses`)。
    /// OpenAI Responses (`/responses`).
    OpenAIResponses,
}

impl ApiFormat {
    /// 从请求路径推断 API 格式。
    /// Infer API format from the request path.
    ///
    /// - `/chat/completions` → OpenAIChat
    /// - `/v1/messages` → Anthropic
    /// - `/responses` → OpenAIResponses
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/chat/completions" => Some(Self::OpenAIChat),
            "/v1/messages" => Some(Self::Anthropic),
            "/responses" => Some(Self::OpenAIResponses),
            _ => None,
        }
    }

    /// 路径标签（用于日志/显示）。
    /// Path label for logging/display.
    pub fn path(self) -> &'static str {
        match self {
            Self::OpenAIChat => "/chat/completions",
            Self::Anthropic => "/v1/messages",
            Self::OpenAIResponses => "/responses",
        }
    }

    /// 解析请求体为 `ConvertedTurn`。
    ///
    /// Parse a request body into a `ConvertedTurn`.
    ///
    /// 内部统一调用 `ConvertedTurn::from_request_*`，提取 system + history +
    /// 最后一条 user，记录来源格式。
    pub fn parse_request(&self, body: &Value) -> Result<ConvertedTurn, ConvertError> {
        match self {
            Self::OpenAIChat => parse_openai_chat(body),
            Self::Anthropic => parse_anthropic(body),
            Self::OpenAIResponses => parse_openai_responses(body),
        }
        .map(|mut ct| {
            ct.format = *self;
            ct
        })
    }

    /// 构造非流式响应 JSON。
    ///
    /// Build a non-streaming response JSON value.
    ///
    /// 根据 `TurnResult` 和模型名，返回符合该格式规范的 JSON。
    pub fn build_response(&self, result: &TurnResult, model: &str) -> Value {
        match self {
            Self::OpenAIChat => build_openai_chat_response(result, model),
            Self::Anthropic => build_anthropic_response(result, model),
            Self::OpenAIResponses => build_openai_responses_response(result, model),
        }
    }

    // --- SSE 流式帧构造 / SSE frame construction ---

    /// 构造首帧（role chunk 或 message_start）。
    /// Build the first frame (role chunk or message_start).
    pub fn role_chunk(&self, id: &str, created: i64, model: &str) -> String {
        match self {
            Self::OpenAIChat => serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": null
                }]
            }).to_string(),
            Self::Anthropic => format!(
                "event: message_start\ndata: {}\n\n",
                serde_json::json!({
                    "type": "message_start",
                    "message": {
                        "id": id,
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": null,
                        "usage": {"input_tokens": 0, "output_tokens": 0}
                    }
                })
            ),
            Self::OpenAIResponses => format!(
                "event: response.created\ndata: {}\n\n",
                serde_json::json!({
                    "type": "response.created",
                    "response": {
                        "id": id,
                        "object": "response",
                        "created_at": created,
                        "model": model,
                        "status": "in_progress",
                        "output": []
                    }
                })
            ),
        }
    }

    /// 构造内容增量帧。
    /// Build a content delta frame.
    pub fn content_chunk(&self, id: &str, created: i64, model: &str, content: &str) -> String {
        match self {
            Self::OpenAIChat => serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {"content": content},
                    "finish_reason": null
                }]
            }).to_string(),
            Self::Anthropic => format!(
                "event: content_block_delta\ndata: {}\n\n",
                serde_json::json!({
                    "type": "content_block_delta",
                    "delta": {"type": "text_delta", "text": content}
                })
            ),
            Self::OpenAIResponses => format!(
                "event: response.output_text.delta\ndata: {}\n\n",
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "delta": content
                })
            ),
        }
    }

    /// 构造结束帧（stop chunk 或 message_stop）。
    /// Build the stop/end frame.
    pub fn stop_chunk(&self, id: &str, created: i64, model: &str) -> String {
        match self {
            Self::OpenAIChat => serde_json::json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            }).to_string(),
            Self::Anthropic => format!(
                "event: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
                serde_json::json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {"output_tokens": 0}
                }),
                serde_json::json!({"type": "message_stop"})
            ),
            Self::OpenAIResponses => format!(
                "event: response.completed\ndata: {}\n\n",
                serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "id": id,
                        "object": "response",
                        "created_at": created,
                        "model": model,
                        "status": "completed",
                        "output": []
                    }
                })
            ),
        }
    }

    /// 流结束哨兵帧（OpenAI Chat 用 `data: [DONE]`，其余为空串）。
    /// Stream-end sentinel frame (OpenAI Chat uses `data: [DONE]`; others empty).
    pub fn done_sentinel(&self) -> Option<String> {
        match self {
            Self::OpenAIChat => Some("data: [DONE]\n\n".into()),
            _ => None,
        }
    }

    /// SSE 帧之间的分隔符。
    /// Separator between SSE frames.
    pub fn sse_separator(&self) -> &'static str {
        match self {
            // OpenAI Chat 的 SSE 用 \n 分隔每行 data:
            // OpenAI Chat SSE uses \n for each data: line.
            Self::OpenAIChat => "",
            // Anthropic 和 Responses 用 \n\n 分隔事件块
            // Anthropic and Responses use \n\n between event blocks.
            _ => "",
        }
    }
}

// ===========================================================================
// 请求解析 / Request parsing
// ===========================================================================

/// 解析 OpenAI Chat Completions 请求。
/// Parse an OpenAI Chat Completions request.
fn parse_openai_chat(body: &Value) -> Result<ConvertedTurn, ConvertError> {
    use crate::phased_api_convert::{ChatCompletionsRequest, ConvertedTurn};
    let req: ChatCompletionsRequest = serde_json::from_value(body.clone())
        .map_err(|e| ConvertError::Invalid(format!("invalid openai-chat request: {e}")))?;
    ConvertedTurn::from_request(&req)
}

/// 解析 Anthropic Messages 请求。
/// Parse an Anthropic Messages request.
///
/// Anthropic 格式特点 / Anthropic format specifics:
/// - system 是顶层字段（不在 messages 里）
/// - messages 里 role 只有 user / assistant
/// - content 可以是 string 或 content block 数组
fn parse_anthropic(body: &Value) -> Result<ConvertedTurn, ConvertError> {
    use pigs_core::{ContentBlock, Message};

    let messages_arr = body
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ConvertError::Invalid("anthropic request missing 'messages' array".into()))?;

    if messages_arr.is_empty() {
        return Err(ConvertError::Invalid("messages must not be empty".into()));
    }

    let mut out: Vec<Message> = Vec::new();

    // 顶层 system 字段 → Message::system / top-level system field → Message::system
    if let Some(system) = body.get("system").and_then(|v| v.as_str()) {
        if !system.is_empty() {
            out.push(Message::system(system));
        }
    }

    for m in messages_arr {
        let role = m
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConvertError::Invalid("anthropic message missing 'role'".into()))?;
        let content = m.get("content");
        let text = anth_content_to_text(content);
        match role {
            "user" => out.push(Message::user(text)),
            "assistant" => out.push(Message::assistant(vec![ContentBlock::Text { text }])),
            other => {
                return Err(ConvertError::Invalid(format!(
                    "unsupported anthropic role `{other}`"
                )));
            }
        }
    }

    // 最后一条非 system 消息是当前用户问题 / last non-system msg = current user question
    let (history, last) = split_last_user(&out)?;
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("pigs")
        .to_string();
    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(ConvertedTurn {
        messages: history.into_iter().chain(std::iter::once(last)).collect(),
        model,
        stream,
        format: ApiFormat::Anthropic,
    })
}

/// 解析 OpenAI Responses 请求。
/// Parse an OpenAI Responses request.
///
/// Responses 格式特点 / Responses format specifics:
/// - system 是顶层 `instructions` 字段
/// - 消息在 `input` 数组里，每项有 `type` 字段
/// - type=message 的 content 是 [{type: input_text|output_text, text: ...}]
/// - type=function_call_output 是工具结果
fn parse_openai_responses(body: &Value) -> Result<ConvertedTurn, ConvertError> {
    use pigs_core::{ContentBlock, Message};

    let input_arr = body
        .get("input")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ConvertError::Invalid("responses request missing 'input' array".into()))?;

    if input_arr.is_empty() {
        return Err(ConvertError::Invalid("input must not be empty".into()));
    }

    let mut out: Vec<Message> = Vec::new();

    // 顶层 instructions 字段 → Message::system / top-level instructions → Message::system
    if let Some(instructions) = body.get("instructions").and_then(|v| v.as_str()) {
        if !instructions.is_empty() {
            out.push(Message::system(instructions));
        }
    }

    for item in input_arr {
        let item_type = item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("message");
        match item_type {
            "message" => {
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user");
                let text = resp_extract_message_text(item);
                if role == "assistant" {
                    out.push(Message::assistant(vec![ContentBlock::Text { text }]));
                } else {
                    out.push(Message::user(text));
                }
            }
            // 工具调用结果，暂忽略（API 自有工具）/ tool output, ignored for now
            "function_call_output" | "function_call" => {}
            _ => {}
        }
    }

    let (history, last) = split_last_user(&out)?;
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("pigs")
        .to_string();
    let stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(ConvertedTurn {
        messages: history.into_iter().chain(std::iter::once(last)).collect(),
        model,
        stream,
        format: ApiFormat::OpenAIResponses,
    })
}

// ===========================================================================
// 响应构造 / Response construction
// ===========================================================================

/// 构造 OpenAI Chat Completions 非流式响应。
/// Build an OpenAI Chat Completions non-streaming response.
fn build_openai_chat_response(result: &TurnResult, model: &str) -> Value {
    let created = chrono::Utc::now().timestamp();
    let completion_tokens = (result.final_text.len() / 4) as u32;
    let phases: Vec<String> = result
        .events
        .iter()
        .filter(|e| e.kind == "phase_start")
        .filter_map(|e| e.phase.clone())
        .collect();
    serde_json::json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": result.final_text
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": completion_tokens,
            "total_tokens": completion_tokens
        },
        "pigs": {
            "ended_with": result.ended_with,
            "phases": phases
        }
    })
}

/// 构造 Anthropic Messages 非流式响应。
/// Build an Anthropic Messages non-streaming response.
fn build_anthropic_response(result: &TurnResult, model: &str) -> Value {
    let phases: Vec<String> = result
        .events
        .iter()
        .filter(|e| e.kind == "phase_start")
        .filter_map(|e| e.phase.clone())
        .collect();
    serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{
            "type": "text",
            "text": result.final_text
        }],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 0,
            "output_tokens": (result.final_text.len() / 4) as u32
        },
        "pigs": {
            "ended_with": result.ended_with,
            "phases": phases
        }
    })
}

/// 构造 OpenAI Responses 非流式响应。
/// Build an OpenAI Responses non-streaming response.
fn build_openai_responses_response(result: &TurnResult, model: &str) -> Value {
    let created = chrono::Utc::now().timestamp();
    let phases: Vec<String> = result
        .events
        .iter()
        .filter(|e| e.kind == "phase_start")
        .filter_map(|e| e.phase.clone())
        .collect();
    serde_json::json!({
        "id": format!("resp_{}", uuid::Uuid::new_v4()),
        "object": "response",
        "created_at": created,
        "model": model,
        "status": "completed",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": result.final_text
            }]
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": (result.final_text.len() / 4) as u32
        },
        "pigs": {
            "ended_with": result.ended_with,
            "phases": phases
        }
    })
}

// ===========================================================================
// 辅助函数 / Helpers
// ===========================================================================

/// 从消息列表中分离最后一条非 system 消息和前面的 history。
/// Split the last non-system message from the rest as history.
fn split_last_user(messages: &[pigs_core::Message]) -> Result<(Vec<pigs_core::Message>, pigs_core::Message), ConvertError> {
    // 从后往前找第一条非 system 消息 / find last non-system message from the end
    let idx = messages
        .iter()
        .rposition(|m| m.role != pigs_core::MessageRole::System)
        .ok_or_else(|| ConvertError::Invalid("no user message found".into()))?;
    let history = messages[..idx].to_vec();
    let last = messages[idx].clone();
    Ok((history, last))
}

/// 从 Anthropic content 值提取文本。
/// Extract text from an Anthropic content value (string or array of blocks).
fn anth_content_to_text(content: Option<&Value>) -> String {
    match content {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
            out
        }
        Some(other) => other.to_string(),
    }
}

/// 从 Responses message item 提取文本（拼接 input_text / output_text）。
/// Extract text from a Responses message item (concatenate input_text/output_text).
fn resp_extract_message_text(item: &Value) -> String {
    let mut out = String::new();
    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
        for part in content {
            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                out.push_str(t);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn from_path_recognizes_three_endpoints() {
        assert_eq!(ApiFormat::from_path("/chat/completions"), Some(ApiFormat::OpenAIChat));
        assert_eq!(ApiFormat::from_path("/v1/messages"), Some(ApiFormat::Anthropic));
        assert_eq!(ApiFormat::from_path("/responses"), Some(ApiFormat::OpenAIResponses));
        assert_eq!(ApiFormat::from_path("/unknown"), None);
    }

    #[test]
    fn parse_openai_chat_simple() {
        let body = serde_json::json!({
            "model": "pigs",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": false
        });
        let ct = ApiFormat::OpenAIChat.parse_request(&body).unwrap();
        assert_eq!(ct.format, ApiFormat::OpenAIChat);
        assert_eq!(ct.messages.len(), 1);
        assert_eq!(ct.messages[0].text_content(), "hello");
    }

    #[test]
    fn parse_anthropic_simple() {
        let body = serde_json::json!({
            "model": "claude-3",
            "system": "You are helpful.",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        });
        let ct = ApiFormat::Anthropic.parse_request(&body).unwrap();
        assert_eq!(ct.format, ApiFormat::Anthropic);
        // system + user = 2 messages
        assert_eq!(ct.messages.len(), 2);
        assert_eq!(ct.messages[0].role.to_string(), "system");
        assert_eq!(ct.messages[1].text_content(), "hi");
    }

    #[test]
    fn parse_responses_simple() {
        let body = serde_json::json!({
            "model": "gpt-4o",
            "instructions": "You are helpful.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}]
            }],
            "stream": false
        });
        let ct = ApiFormat::OpenAIResponses.parse_request(&body).unwrap();
        assert_eq!(ct.format, ApiFormat::OpenAIResponses);
        // system + user = 2
        assert_eq!(ct.messages.len(), 2);
        assert_eq!(ct.messages[0].role.to_string(), "system");
        assert_eq!(ct.messages[1].text_content(), "hello");
    }

    #[test]
    fn build_openai_chat_response_has_choices() {
        let result = TurnResult {
            final_text: "hello world".into(),
            events: vec![],
            ended_with: "PIGEND".into(),
        };
        let resp = ApiFormat::OpenAIChat.build_response(&result, "pigs");
        assert_eq!(resp["object"], "chat.completion");
        assert_eq!(resp["choices"][0]["message"]["content"], "hello world");
        assert_eq!(resp["pigs"]["ended_with"], "PIGEND");
    }

    #[test]
    fn build_anthropic_response_has_content_blocks() {
        let result = TurnResult {
            final_text: "hi".into(),
            events: vec![],
            ended_with: "PIGEND".into(),
        };
        let resp = ApiFormat::Anthropic.build_response(&result, "claude-3");
        assert_eq!(resp["type"], "message");
        assert_eq!(resp["content"][0]["type"], "text");
        assert_eq!(resp["content"][0]["text"], "hi");
    }

    #[test]
    fn build_responses_response_has_output() {
        let result = TurnResult {
            final_text: "yo".into(),
            events: vec![],
            ended_with: "PIGEND".into(),
        };
        let resp = ApiFormat::OpenAIResponses.build_response(&result, "gpt-4o");
        assert_eq!(resp["object"], "response");
        assert_eq!(resp["output"][0]["type"], "message");
        assert_eq!(resp["output"][0]["content"][0]["text"], "yo");
    }

    #[test]
    fn sse_chunks_for_all_formats() {
        let fmt = ApiFormat::OpenAIChat;
        let role = fmt.role_chunk("id", 0, "m");
        assert!(role.contains("assistant"));
        let content = fmt.content_chunk("id", 0, "m", "text");
        assert!(content.contains("text"));
        let stop = fmt.stop_chunk("id", 0, "m");
        assert!(stop.contains("stop"));

        let fmt = ApiFormat::Anthropic;
        let role = fmt.role_chunk("id", 0, "m");
        assert!(role.contains("message_start"));
        let content = fmt.content_chunk("id", 0, "m", "text");
        assert!(content.contains("content_block_delta"));
        let stop = fmt.stop_chunk("id", 0, "m");
        assert!(stop.contains("message_stop"));

        let fmt = ApiFormat::OpenAIResponses;
        let role = fmt.role_chunk("id", 0, "m");
        assert!(role.contains("response.created"));
        let content = fmt.content_chunk("id", 0, "m", "text");
        assert!(content.contains("response.output_text.delta"));
        let stop = fmt.stop_chunk("id", 0, "m");
        assert!(stop.contains("response.completed"));
    }
}
