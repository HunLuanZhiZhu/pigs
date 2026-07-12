//! Context compaction — summarize old messages to free context window space.

use pigs_core::{ContentBlock, Message, MessageRole};

use crate::session::Session;

/// Configuration for auto-compaction.
#[derive(Debug, Clone)]
pub struct CompactConfig {
    /// Trigger compaction when estimated tokens exceed this threshold.
    pub token_threshold: u64,
    /// Number of recent messages to keep unmodified.
    pub keep_recent: usize,
    /// Max characters of each message body to include in the summary.
    pub summary_message_chars: usize,
    /// Force compaction even if under threshold (used by /compact).
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

/// Compact the session by replacing old messages with a summary.
/// Returns true if compaction was performed.
pub fn compact_session(session: &mut Session, config: &CompactConfig) -> bool {
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

    #[test]
    fn test_no_compaction_needed() {
        let mut session = Session::new("test");
        session.add_message(Message::user("Hello"));
        session.add_message(Message::assistant(vec![ContentBlock::text("Hi!")]));
        let config = CompactConfig::default();
        assert!(!needs_compaction(&session, &config));
        assert!(!compact_session(&mut session, &config));
    }

    #[test]
    fn test_compaction_with_low_threshold() {
        let mut session = Session::new("test");
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
        assert!(compact_session(&mut session, &config));
        assert_eq!(session.message_count(), 5);
        assert_eq!(session.messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_force_compaction() {
        let mut session = Session::new("test");
        for i in 0..6 {
            session.add_message(Message::user(format!("m{i}")));
        }
        let config = CompactConfig {
            token_threshold: 10_000_000,
            keep_recent: 2,
            summary_message_chars: 50,
            force: true,
        };
        assert!(compact_session(&mut session, &config));
        assert_eq!(session.message_count(), 3);
    }
}
