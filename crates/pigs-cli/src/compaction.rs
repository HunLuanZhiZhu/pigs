//! Agent 层的会话压缩实现。
//!
//! 本模块为 [`pigs_session::Compactor`] trait 提供了一个基于 `ApiClient` 的实现
//! [`AgentCompactor`]，用于手动 `/compact` 和自动压缩路径的 LLM 摘要式压缩。
//!
//! 设计说明 / Design notes:
//! - 不直接为 `Agent` 实现 `Compactor`：`compact_session` 需要同时可变借用
//!   `agent.session` 和不可变借用 `agent`（作为 compactor），这会触发借用冲突。
//! - 改为提供一个独立的 `AgentCompactor`，它持有 `Arc<dyn ApiClient>` 和模型名，
//!   可独立于 `Agent` 传递给 `compact_session`。

use async_trait::async_trait;
use pigs_core::{ApiClient, ApiRequest, Message};
use pigs_session::Compactor;

/// LLM 摘要提示词：要求模型把旧对话压缩成结构化摘要。
/// Summarization prompt: ask the model to compress old conversation into a structured summary.
pub const SUMMARY_PROMPT: &str = r#"You are a conversation summarizer. Summarize the following conversation context into a structured summary with these sections:

## Objective
What the user is trying to accomplish.

## Important Details
Key technical decisions, constraints, and context.

## Work State
- Completed: What has been done.
- Active: What is being worked on.
- Blocked: Any blockers.

## Relevant Files
Files that have been read, modified, or discussed.

## Key Code & Commands
Important code snippets, commands, or configurations.

## Next Step
What should happen next.

Rules:
- Preserve exact file paths, symbol names, and commands.
- Be concise but complete.
- Never mention the compaction process."#;

/// 基于 `ApiClient` 的 `Compactor` 实现。
///
/// 持有一个共享的 LLM 客户端和模型名，用于在压缩时发起摘要请求。
/// 不依赖 `Agent`，避免与 `&mut agent.session` 的借用冲突。
///
/// `Compactor` implementation backed by a shared `ApiClient`.
pub struct AgentCompactor {
    /// 共享的 LLM API 客户端（通常是 `ProxyApiClient` 或 `pigs-llm` 客户端）。
    /// Shared LLM API client (typically `ProxyApiClient` or a `pigs-llm` client).
    pub api_client: std::sync::Arc<dyn ApiClient>,
    /// 用于摘要请求的模型名（通常是当前会话的远端模型名）。
    /// Model name used for the summarization request.
    pub model: String,
}

impl AgentCompactor {
    /// 创建一个新的 compactor。
    /// Create a new compactor.
    pub fn new(api_client: std::sync::Arc<dyn ApiClient>, model: String) -> Self {
        Self { api_client, model }
    }
}

#[async_trait]
impl Compactor for AgentCompactor {
    async fn summarize(
        &self,
        messages: &[Message],
        model: &str,
    ) -> anyhow::Result<String> {
        // 把旧消息序列化为可读文本，供 LLM 摘要 / Serialize old messages to readable text.
        let conversation_text = serialize_messages_for_summary(messages);

        // 构建摘要请求 / Build the summarization request.
        let request = ApiRequest::new(
            model.to_string(),
            vec![Message::user(format!(
                "Summarize the following conversation:\n\n{conversation_text}"
            ))],
        )
        .with_system_prompt(SUMMARY_PROMPT)
        .with_max_tokens(4096);

        // 发送非流式请求 / Send a non-streaming request.
        let response = self
            .api_client
            .send_message(request)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        Ok(response.text_content())
    }
}

/// 把旧消息序列化为 LLM 可读的文本格式（用于摘要提示词）。
/// Serialize old messages into a readable text format for the summarization prompt.
///
/// 对超长文本/工具输入做截断，避免提示词过长。
/// Truncates overly long text / tool inputs to keep the prompt manageable.
pub fn serialize_messages_for_summary(messages: &[Message]) -> String {
    let mut out = String::new();
    for (i, msg) in messages.iter().enumerate() {
        let role = match msg.role {
            pigs_core::MessageRole::System => "system",
            pigs_core::MessageRole::User => "user",
            pigs_core::MessageRole::Assistant => "assistant",
            pigs_core::MessageRole::Tool => "tool",
        };
        out.push_str(&format!("--- Message {i} [{role}] ---\n"));
        for block in &msg.content {
            match block {
                pigs_core::ContentBlock::Text { text } => {
                    let truncated = if text.len() > 2000 {
                        format!("{}...(truncated)", &text[..2000])
                    } else {
                        text.clone()
                    };
                    out.push_str(&truncated);
                    out.push('\n');
                }
                pigs_core::ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str(&format!("[Tool Call: {name}]\n"));
                    let input_str = serde_json::to_string_pretty(input).unwrap_or_default();
                    let truncated = if input_str.len() > 1000 {
                        format!("{}...(truncated)", &input_str[..1000])
                    } else {
                        input_str
                    };
                    out.push_str(&truncated);
                    out.push('\n');
                }
                pigs_core::ContentBlock::ToolResult { output, .. } => {
                    out.push_str("[Tool Result]\n");
                    let truncated = if output.len() > 1000 {
                        format!("{}...(truncated)", &output[..1000])
                    } else {
                        output.clone()
                    };
                    out.push_str(&truncated);
                    out.push('\n');
                }
            }
        }
        out.push('\n');
    }
    out
}
