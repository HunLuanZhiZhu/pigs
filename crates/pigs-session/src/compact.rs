//! Context compaction — summarize old messages to free context window space.
//!
//! 提供两种压缩策略：
//! - `compact_session`（默认，LLM 摘要式）：通过上层实现的 [`Compactor`] trait
//!   调用 LLM 生成对话摘要，替换旧消息。手动 `/compact` 和自动压缩共用此路径。
//! - `compact_session_truncate`（截断式，fallback）：不调 LLM，把每条旧消息
//!   截断到固定字符数后拼接成摘要。`/compact truncate` 使用此路径。
//!
//! Two compaction strategies:
//! - `compact_session` (default, LLM summary): invokes an LLM via the
//!   [`Compactor`] trait implemented by the upper layer to summarize old
//!   messages. Used by both manual `/compact` and auto-compaction.
//! - `compact_session_truncate` (truncation, fallback): no LLM call; truncates
//!   each old message to a fixed char count and concatenates them. Used by
//!   `/compact truncate`.

use async_trait::async_trait;

use pigs_core::{ContentBlock, Message, MessageRole};

use crate::session::Session;

/// 压缩器 trait，由上层（pigs-cli）实现 LLM 摘要能力。
/// Compactor trait implemented by the upper layer (pigs-cli) to provide
/// LLM-backed summarization.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// 用 LLM 摘要旧消息，返回 summary 文本。
    /// Summarize the given old messages via an LLM, returning the summary text.
    async fn summarize(
        &self,
        messages: &[Message],
        model: &str,
    ) -> anyhow::Result<String>;
}

/// 压缩配置。
/// Configuration for compaction.
#[derive(Debug, Clone)]
pub struct CompactConfig {
    /// 触发压缩的 token 阈值（估算值）。
    /// Trigger compaction when estimated tokens exceed this threshold.
    pub token_threshold: u64,
    /// 保留的最近消息数（不参与摘要）。
    /// Number of recent messages to keep unmodified.
    pub keep_recent: usize,
    /// 截断式压缩时每条消息正文的最大字符数。
    /// Max characters of each message body to include in the truncation summary.
    pub summary_message_chars: usize,
    /// 是否强制压缩（忽略阈值，用于 `/compact`）。
    /// Force compaction even if under threshold (used by `/compact`).
    pub force: bool,
}

impl Default for CompactConfig {
    fn default() -> Self {
        CompactConfig {
            token_threshold: 100_000,
            keep_recent: 4,
            summary_message_chars: 400,
            force: false,
        }
    }
}

/// 压缩错误类型。
/// Compaction error type.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    /// 消息太少，无需压缩。
    /// Not enough messages to compact.
    #[error("not enough messages to compact (have {have}, keep_recent={keep_recent})")]
    TooFewMessages { have: usize, keep_recent: usize },
    /// 低于阈值且未强制压缩。
    /// Below threshold and not forced.
    #[error("below threshold ({est} < {threshold}) and not forced")]
    BelowThreshold { est: u64, threshold: u64 },
    /// LLM 摘要调用失败。
    /// LLM summarization call failed.
    #[error("summarize failed: {0}")]
    Summarize(String),
}

/// 用 LLM 摘要式压缩会话。
/// Compact the session by summarizing old messages via an LLM.
///
/// 流程 / Flow:
/// 1. 判断是否需要压缩（阈值 + `keep_recent` + `force`）。
/// 2. 切分旧消息与最近消息。
/// 3. 调用 `compactor.summarize(...)` 生成摘要。
/// 4. 用摘要 system 消息替换旧消息，保留最近消息。
///
/// Returns `true` if compaction was performed, `false` if skipped.
pub async fn compact_session(
    session: &mut Session,
    config: &CompactConfig,
    compactor: &dyn Compactor,
    model: &str,
) -> Result<bool, CompactionError> {
    // 判断是否需要压缩 / Decide if compaction is needed.
    if !config.force && session.estimated_tokens() < config.token_threshold {
        return Err(CompactionError::BelowThreshold {
            est: session.estimated_tokens(),
            threshold: config.token_threshold,
        });
    }
    if session.messages.len() <= config.keep_recent {
        return Err(CompactionError::TooFewMessages {
            have: session.messages.len(),
            keep_recent: config.keep_recent,
        });
    }

    let split_point = session.messages.len() - config.keep_recent;
    let old_messages = &session.messages[..split_point];

    // 调用 LLM 摘要 / Invoke the LLM summarizer.
    let summary_text = compactor
        .summarize(old_messages, model)
        .await
        .map_err(|e| CompactionError::Summarize(e.to_string()))?;

    // 构建摘要消息 / Build the summary system message.
    let summary_message = Message {
        role: MessageRole::System,
        content: vec![ContentBlock::text(format!(
            "--- Conversation Summary (auto-compacted) ---\n{summary_text}\n--- End Summary ---"
        ))],
        usage: None,
    };

    // 替换旧消息：摘要 + 最近消息 / Replace old messages with summary + recent.
    let recent: Vec<Message> = session.messages[split_point..].to_vec();
    session.messages.clear();
    session.messages.push(summary_message);
    session.messages.extend(recent);
    session.dirty = true;
    Ok(true)
}

/// 截断式压缩（不调 LLM，fallback）。
/// Truncation-based compaction (no LLM call, fallback).
///
/// 把每条旧消息的正文截断到 `summary_message_chars` 字符后拼接成摘要，
/// 替换旧消息。保留最近 `keep_recent` 条消息不变。
///
/// Replaces old messages with a truncation-based summary, keeping the most
/// recent `keep_recent` messages intact.
pub fn compact_session_truncate(session: &mut Session, config: &CompactConfig) -> bool {
    if !config.force && session.estimated_tokens() < config.token_threshold {
        return false;
    }

    if session.messages.len() <= config.keep_recent {
        return false;
    }

    let split_point = session.messages.len() - config.keep_recent;
    let old_messages = &session.messages[..split_point];

    let mut summary = String::from("--- Conversation Summary (auto-compacted) ---\n\n");
    summary.push_str(&format!(
        "Compacted {} earlier messages. Recent {} messages retained in full.\n\n",
        old_messages.len(),
        config.keep_recent
    ));

    for (i, msg) in old_messages.iter().enumerate() {
        let role_str = match msg.role {
            MessageRole::System => "System",
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::Tool => "Tool",
        };

        let text = truncate_chars(&msg.text_content(), config.summary_message_chars);
        if !text.is_empty() {
            summary.push_str(&format!("{i}. [{role_str}]: {text}\n\n"));
        }

        for (id, name, input) in msg.tool_uses() {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            let input_str = truncate_chars(&input_str, 200);
            summary.push_str(&format!("   - Tool call [{id}]: {name}({input_str})\n"));
        }
    }

    summary.push_str("--- End Summary ---\n");

    let summary_message = Message {
        role: MessageRole::System,
        content: vec![ContentBlock::text(summary)],
        usage: None,
    };

    let recent: Vec<Message> = session.messages[split_point..].to_vec();
    session.messages.clear();
    session.messages.push(summary_message);
    session.messages.extend(recent);
    session.dirty = true;
    true
}

/// 检查当前会话是否需要压缩。
/// Check if compaction is needed based on the current session state.
pub fn needs_compaction(session: &Session, config: &CompactConfig) -> bool {
    (config.force || session.estimated_tokens() >= config.token_threshold)
        && session.messages.len() > config.keep_recent
}

fn truncate_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{t}...")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 一个永远返回固定摘要的测试用 Compactor。
    /// A test Compactor that always returns a fixed summary.
    struct StaticCompactor;

    #[async_trait]
    impl Compactor for StaticCompactor {
        async fn summarize(
            &self,
            _messages: &[Message],
            _model: &str,
        ) -> anyhow::Result<String> {
            Ok("static summary".to_string())
        }
    }

    #[test]
    fn test_no_compaction_needed() {
        let dir = std::env::temp_dir().join("pigs_test_compact_1");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        session.add_message(Message::user("Hello"));
        session.add_message(Message::assistant(vec![ContentBlock::text("Hi!")]));
        let config = CompactConfig::default();
        assert!(!needs_compaction(&session, &config));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_compaction_with_low_threshold() {
        let dir = std::env::temp_dir().join("pigs_test_compact_2");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        for i in 0..20 {
            session.add_message(Message::user(format!(
                "Message number {i} with some text content"
            )));
        }
        let config = CompactConfig {
            token_threshold: 10,
            keep_recent: 4,
            summary_message_chars: 100,
            force: false,
        };
        assert!(needs_compaction(&session, &config));
        let compactor = StaticCompactor;
        let compacted = compact_session(&mut session, &config, &compactor, "test")
            .await
            .unwrap();
        assert!(compacted);
        assert_eq!(session.message_count(), 5);
        assert_eq!(session.messages[0].role, MessageRole::System);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_force_compaction() {
        let dir = std::env::temp_dir().join("pigs_test_compact_3");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        for i in 0..6 {
            session.add_message(Message::user(format!("m{i}")));
        }
        let config = CompactConfig {
            token_threshold: 10_000_000,
            keep_recent: 2,
            summary_message_chars: 50,
            force: true,
        };
        let compactor = StaticCompactor;
        let compacted = compact_session(&mut session, &config, &compactor, "test")
            .await
            .unwrap();
        assert!(compacted);
        assert_eq!(session.message_count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_compaction_too_few_messages() {
        let dir = std::env::temp_dir().join("pigs_test_compact_4");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        session.add_message(Message::user("only one"));
        let config = CompactConfig {
            token_threshold: 1,
            keep_recent: 4,
            summary_message_chars: 100,
            force: true,
        };
        let compactor = StaticCompactor;
        let result = compact_session(&mut session, &config, &compactor, "test").await;
        assert!(matches!(result, Err(CompactionError::TooFewMessages { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_truncate_compaction_with_low_threshold() {
        let dir = std::env::temp_dir().join("pigs_test_compact_trunc_1");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        for i in 0..20 {
            session.add_message(Message::user(format!(
                "Message number {i} with some text content"
            )));
        }
        let config = CompactConfig {
            token_threshold: 10,
            keep_recent: 4,
            summary_message_chars: 100,
            force: false,
        };
        assert!(needs_compaction(&session, &config));
        assert!(compact_session_truncate(&mut session, &config));
        assert_eq!(session.message_count(), 5);
        assert_eq!(session.messages[0].role, MessageRole::System);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_truncate_force_compaction() {
        let dir = std::env::temp_dir().join("pigs_test_compact_trunc_2");
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::new("test", &dir);
        for i in 0..6 {
            session.add_message(Message::user(format!("m{i}")));
        }
        let config = CompactConfig {
            token_threshold: 10_000_000,
            keep_recent: 2,
            summary_message_chars: 50,
            force: true,
        };
        assert!(compact_session_truncate(&mut session, &config));
        assert_eq!(session.message_count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
