//! 共享的 OpenAI 格式请求转换 → 相位运行时。
//! Shared OpenAI-shaped request conversion → phased runtime.
//!
//! 这是 **API 转化** 层，被以下入口复用：
//! This is the **API 转化** layer reused by:
//! - HTTP server（外部消费者，本地端口）/ HTTP server (external consumers on a local port)
//! - CLI / `--once`（进程内，无端口）/ CLI / `--once` (in-process, no port)
//!
//! 传输层（HTTP SSE vs 终端打印）不在此模块内。
//! Transport (HTTP SSE vs terminal print) stays outside this module.
//!
//! 设计：调用方的 `messages` 数组保持原样（包括 `system` 角色）。
//! 相位运行时 clone 一份，去掉最后一条 user 消息，追加相位特定的 user 消息。
//! 原始 system / history 不被解构或重新组装。
//! Design: the caller's `messages` array is kept intact (including the
//! `system` role). The phased runtime clones it, drops the last user
//! message, and appends a phase-specific user message. The original
//! system / history are never deconstructed or reassembled.

use std::sync::Arc;

use pigs_core::Message;
use serde::{Deserialize, Serialize};

use crate::format::ApiFormat;
use crate::phased_runtime::{PhasedRuntime, ProgressSink, TurnResult};

/// OpenAI 兼容的聊天消息（请求侧）。
/// OpenAI-compatible chat message (request side).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    /// 角色：system / user / assistant / tool
    /// Role: system / user / assistant / tool
    pub role: String,
    /// 内容（可为字符串或多段文本数组）
    /// Content (may be a string or an array of text parts)
    pub content: Option<serde_json::Value>,
}

/// pigs 使用的最小化 OpenAI chat-completions 请求体。
/// Minimal OpenAI chat-completions request shape used by pigs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionsRequest {
    /// 请求的模型 ID（可选，缺省 "pigs"）
    /// Requested model ID (optional, defaults to "pigs")
    #[serde(default)]
    pub model: Option<String>,
    /// 消息数组
    /// Messages array
    pub messages: Vec<ChatMessage>,
    /// 是否流式响应（可选，缺省 false）
    /// Whether to stream the response (optional, defaults to false)
    #[serde(default)]
    pub stream: Option<bool>,
}

/// OpenAI 格式请求转换后的相位运行时输入。
///
/// `messages` 是**完整、未修改的**调用方消息数组
/// （含 `system` 角色如有）。相位运行时原样接收，
/// 只在每相位替换最后一条 user 消息。
///
/// Result of converting an OpenAI-shaped request into runtime inputs.
///
/// `messages` is the **complete, unmodified** caller message array
/// (including `system` role if present). The phased runtime takes this
/// as-is and only replaces the last user turn per-phase.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConvertedTurn {
    /// 完整的调用方消息 —— system + history + 最后一条 user，原样保留。
    /// Full caller messages — system + history + last user, untouched.
    pub messages: Vec<Message>,
    /// 请求的模型 ID
    /// Requested model ID
    pub model: String,
    /// 是否流式
    /// Whether streaming
    pub stream: bool,
    /// 请求来源的 API 格式（用于构造同格式响应）。
    /// Source API format (used to build a same-format response).
    pub format: ApiFormat,
}

/// 请求转换错误。
/// Request conversion error.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("{0}")]
    Invalid(String),
}

impl ConvertedTurn {
    /// 将 OpenAI 格式消息解析为相位运行时输入。
    /// Parse OpenAI-shaped chat messages into phased-runtime inputs.
    pub fn from_request(req: &ChatCompletionsRequest) -> Result<Self, ConvertError> {
        let messages = convert_messages(&req.messages)?;
        Ok(Self {
            messages,
            model: req.model.clone().unwrap_or_else(|| "pigs".into()),
            stream: req.stream.unwrap_or(false),
            format: ApiFormat::OpenAIChat,
        })
    }

    /// 便捷构造：从纯用户问题构建（CLI / --once 用）。
    /// Convenience: build from a plain user question (CLI / --once).
    pub fn from_user_question(question: impl Into<String>, history: Vec<Message>) -> Self {
        let mut messages = history;
        messages.push(Message::user(question.into()));
        Self {
            messages,
            model: "pigs".into(),
            stream: false,
            format: ApiFormat::OpenAIChat,
        }
    }
}

/// 将 OpenAI 格式 `ChatMessage` 数组转换为 `pigs_core::Message` 数组。
///
/// 所有角色原样保留。调用方的 `system` 消息保持为 `Message::system()`
/// 变体，下游 `ApiRequest` 通过 `with_system_prompt` 字段使用它。
///
/// Convert OpenAI-shaped `ChatMessage` array into `pigs_core::Message` array.
///
/// All roles are preserved as-is. The caller's `system` message stays a
/// `Message::system()` variant, picked up downstream via `with_system_prompt`.
fn convert_messages(messages: &[ChatMessage]) -> Result<Vec<Message>, ConvertError> {
    if messages.is_empty() {
        return Err(ConvertError::Invalid("messages must not be empty".into()));
    }
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        let text = content_to_text(m.content.as_ref());
        match m.role.as_str() {
            // system 角色 → Message::system / system role → Message::system
            "system" => {
                if !text.is_empty() {
                    out.push(Message::system(&text));
                }
            }
            // user 角色 → Message::user / user role → Message::user
            "user" => out.push(Message::user(text)),
            // assistant 角色 → Message::assistant / assistant role → Message::assistant
            "assistant" => out.push(Message::assistant(vec![pigs_core::ContentBlock::Text {
                text,
            }])),
            // tool 角色暂忽略（API 自有工具）/ tool role ignored for now
            "tool" => {}
            other => {
                return Err(ConvertError::Invalid(format!("unsupported role `{other}`")));
            }
        }
    }
    Ok(out)
}

/// 从 `serde_json::Value` 提取文本内容。
/// Extract text content from a `serde_json::Value`.
fn content_to_text(content: Option<&serde_json::Value>) -> String {
    match content {
        None => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                } else if let Some(t) = p.as_str() {
                    out.push_str(t);
                }
            }
            out
        }
        Some(other) => other.to_string(),
    }
}

/// 在共享相位运行时上执行一轮转换后的对话。
/// Run one converted turn on the shared phased runtime.
///
/// 从 `converted.model` 中剥离 `-pig` 后缀，得到真实模型名，
/// 传入 `run_turn_with_progress` 作为 `model_override`。
/// 这样一个共享的 `PhasedRuntime` 实例可以服务不同的 `-pig` 模型请求。
///
/// Strips the `-pig` suffix from `converted.model` to get the real model name,
/// passing it to `run_turn_with_progress` as `model_override`. This lets a
/// single shared `PhasedRuntime` serve different `-pig` model requests.
pub async fn run_converted_turn(
    runtime: &PhasedRuntime,
    converted: &ConvertedTurn,
    progress: Option<ProgressSink>,
) -> anyhow::Result<TurnResult> {
    // 剥离 -pig 后缀得到真实模型名 / Strip -pig suffix to get the real model name
    let real_model = converted
        .model
        .strip_suffix("-pig")
        .unwrap_or(&converted.model);
    runtime
        .run_turn_with_progress(&converted.messages, progress, Some(real_model))
        .await
}

/// 同 [`run_converted_turn`]，但使用 `Arc` 运行时（HTTP 处理器用）。
/// Same as [`run_converted_turn`] with an `Arc` runtime (HTTP handlers).
pub async fn run_converted_turn_arc(
    runtime: &Arc<PhasedRuntime>,
    converted: &ConvertedTurn,
    progress: Option<ProgressSink>,
) -> anyhow::Result<TurnResult> {
    run_converted_turn(runtime.as_ref(), converted, progress).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn converts_simple_user_message() {
        let req = ChatCompletionsRequest {
            model: Some("pigs".into()),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some(serde_json::json!("hello")),
            }],
            stream: Some(false),
        };
        let c = ConvertedTurn::from_request(&req).unwrap();
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages[0].text_content(), "hello");
        assert!(!c.stream);
    }

    #[test]
    fn keeps_full_history_including_system() {
        let req = ChatCompletionsRequest {
            model: None,
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: Some(serde_json::json!("You are helpful.")),
                },
                ChatMessage {
                    role: "user".into(),
                    content: Some(serde_json::json!("q1")),
                },
                ChatMessage {
                    role: "assistant".into(),
                    content: Some(serde_json::json!("a1")),
                },
                ChatMessage {
                    role: "user".into(),
                    content: Some(serde_json::json!("q2")),
                },
            ],
            stream: None,
        };
        let c = ConvertedTurn::from_request(&req).unwrap();
        // 全部 4 条消息按序保留 / All 4 messages preserved in order.
        assert_eq!(c.messages.len(), 4);
        assert_eq!(c.messages[0].role.to_string(), "system");
        assert_eq!(c.messages[1].text_content(), "q1");
        assert_eq!(c.messages[3].text_content(), "q2");
    }
}
