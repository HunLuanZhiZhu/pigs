//! Message model — the core data structure for conversation history.

use serde::{Deserialize, Serialize};

/// Role of a message in the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::System => write!(f, "system"),
            MessageRole::User => write!(f, "user"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::Tool => write!(f, "tool"),
        }
    }
}

/// A block of content within a message. Messages can contain multiple blocks
/// (e.g., an assistant message might contain text followed by tool-use requests).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text {
        text: String,
    },
    /// A tool-use request from the assistant.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result of a tool execution, sent back to the model.
    ToolResult {
        tool_use_id: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl ContentBlock {
    /// Create a text content block.
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into() }
    }

    /// Create a tool-use content block.
    pub fn tool_use(id: impl Into<String>, name: impl Into<String>, input: serde_json::Value) -> Self {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    /// Create a tool-result content block.
    pub fn tool_result(tool_use_id: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        ContentBlock::ToolResult {
            tool_use_id: tool_use_id.into(),
            output: output.into(),
            is_error,
        }
    }

    /// Extract text if this is a Text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Check if this is a tool-use block.
    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse { .. })
    }

    /// Extract the tool name if this is a ToolUse block.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            ContentBlock::ToolUse { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// A single message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::TokenUsage>,
}

impl Message {
    /// Create a system message with the given text.
    pub fn system(text: impl Into<String>) -> Self {
        Message {
            role: MessageRole::System,
            content: vec![ContentBlock::text(text)],
            usage: None,
        }
    }

    /// Create a user message with the given text.
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: MessageRole::User,
            content: vec![ContentBlock::text(text)],
            usage: None,
        }
    }

    /// Create an assistant message with the given content blocks.
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Message {
            role: MessageRole::Assistant,
            content,
            usage: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_use_id: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        Message {
            role: MessageRole::Tool,
            content: vec![ContentBlock::tool_result(tool_use_id, output, is_error)],
            usage: None,
        }
    }

    /// Get all text content from this message, concatenated.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract all tool-use blocks from this message.
    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }

    /// Check if this message contains any tool-use requests.
    pub fn has_tool_uses(&self) -> bool {
        self.content.iter().any(|b| b.is_tool_use())
    }

    /// Set the usage information for this message.
    pub fn with_usage(mut self, usage: crate::TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::user("Hello, world!");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.text_content(), "Hello, world!");
        assert!(!msg.has_tool_uses());
    }

    #[test]
    fn test_tool_use_extraction() {
        let msg = Message::assistant(vec![
            ContentBlock::text("Let me check."),
            ContentBlock::tool_use("call_1", "bash", serde_json::json!({"command": "ls"})),
        ]);
        assert!(msg.has_tool_uses());
        let uses = msg.tool_uses();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].0, "call_1");
        assert_eq!(uses[0].1, "bash");
    }

    #[test]
    fn test_role_display() {
        assert_eq!(MessageRole::System.to_string(), "system");
        assert_eq!(MessageRole::User.to_string(), "user");
        assert_eq!(MessageRole::Assistant.to_string(), "assistant");
        assert_eq!(MessageRole::Tool.to_string(), "tool");
    }

    #[test]
    fn test_serde_roundtrip() {
        let msg = Message::assistant(vec![
            ContentBlock::text("Hello"),
            ContentBlock::tool_use("id1", "bash", serde_json::json!({"command": "echo hi"})),
        ]);
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, MessageRole::Assistant);
        assert_eq!(deserialized.content.len(), 2);
    }
}
