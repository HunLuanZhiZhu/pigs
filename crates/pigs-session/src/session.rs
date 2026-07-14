//! Session model and JSONL persistence.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use pigs_core::{Message, TokenUsage};

/// Error type for session operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Failed to create session directory: {0}")]
    DirectoryCreate(String),
    #[error("Failed to write session file: {0}")]
    Write(String),
    #[error("Failed to read session file: {0}")]
    Read(String),
    #[error("Failed to parse session line: {0}")]
    Parse(String),
    #[error("Session not found: {0}")]
    NotFound(String),
    #[error("Invalid session data: {0}")]
    Invalid(String),
}

/// Metadata about a saved session (without full message history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub model: String,
    pub message_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_usage: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A conversation session with persistence support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub messages: Vec<Message>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_usage: TokenUsage,
    /// Auto-generated or user-provided session title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip)]
    #[serde(default = "default_false")]
    pub dirty: bool,
}

fn default_false() -> bool {
    false
}

impl Session {
    /// Create a new session with the given model.
    pub fn new(model: impl Into<String>) -> Self {
        let now = Utc::now();
        Session {
            session_id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            model: model.into(),
            workspace_root: None,
            created_at: now,
            updated_at: now,
            total_usage: TokenUsage::default(),
            title: None,
            dirty: false,
        }
    }

    /// Set the workspace root.
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(root);
        self
    }

    /// Add a message to the session.
    pub fn add_message(&mut self, message: Message) {
        if let Some(usage) = &message.usage {
            self.total_usage.add(usage);
        }
        // Auto-title from the first user message.
        if self.title.is_none() && matches!(message.role, pigs_core::MessageRole::User) {
            let text = message.text_content();
            if !text.trim().is_empty() {
                self.title = Some(auto_title_from_text(&text));
            }
        }
        self.messages.push(message);
        self.updated_at = Utc::now();
        self.dirty = true;
    }

    /// Set or replace the session title.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
        self.updated_at = Utc::now();
        self.dirty = true;
    }

    /// Display title (falls back to short id).
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("session-{}", self.short_id()))
    }

    /// Add usage from an API response.
    pub fn add_usage(&mut self, usage: &TokenUsage) {
        self.total_usage.add(usage);
        self.updated_at = Utc::now();
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.title = None;
        self.updated_at = Utc::now();
        self.dirty = true;
    }

    /// Get the number of messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get the session file path.
    pub fn file_path(sessions_dir: &Path, session_id: &str) -> PathBuf {
        sessions_dir.join(format!("{session_id}.jsonl"))
    }

    /// Save the session to a JSONL file.
    /// Writes the full session metadata as the first line, then each message as a line.
    pub fn save(&mut self, sessions_dir: &Path) -> Result<(), SessionError> {
        std::fs::create_dir_all(sessions_dir)
            .map_err(|e| SessionError::DirectoryCreate(e.to_string()))?;

        let path = Self::file_path(sessions_dir, &self.session_id);

        // Write full session as a single JSON line
        let json = serde_json::to_string(self)
            .map_err(|e| SessionError::Write(format!("Failed to serialize session: {e}")))?;

        std::fs::write(&path, format!("{json}\n"))
            .map_err(|e| SessionError::Write(format!("Failed to write session file: {e}")))?;

        self.dirty = false;
        Ok(())
    }

    /// Load a session from a JSONL file.
    pub fn load(sessions_dir: &Path, session_id: &str) -> Result<Self, SessionError> {
        let path = Self::resolve_session_path(sessions_dir, session_id)?;

        let content = std::fs::read_to_string(&path)
            .map_err(|e| SessionError::Read(format!("Failed to read session file: {e}")))?;

        let line = content
            .lines()
            .next()
            .ok_or_else(|| SessionError::Parse("Session file is empty".to_string()))?;

        let mut session: Session = serde_json::from_str(line)
            .map_err(|e| SessionError::Parse(format!("Failed to parse session JSON: {e}")))?;

        session.dirty = false;
        Ok(session)
    }

    /// Delete a session file by id (supports full id or unique prefix).
    pub fn delete(
        sessions_dir: &Path,
        session_id_or_prefix: &str,
    ) -> Result<PathBuf, SessionError> {
        let path = Self::resolve_session_path(sessions_dir, session_id_or_prefix)?;
        std::fs::remove_file(&path)
            .map_err(|e| SessionError::Write(format!("Failed to delete session: {e}")))?;
        Ok(path)
    }

    /// Resolve a session file path from full id or unique prefix.
    pub fn resolve_session_path(
        sessions_dir: &Path,
        session_id_or_prefix: &str,
    ) -> Result<PathBuf, SessionError> {
        let direct = Self::file_path(sessions_dir, session_id_or_prefix);
        if direct.exists() {
            return Ok(direct);
        }
        if !sessions_dir.exists() {
            return Err(SessionError::NotFound(session_id_or_prefix.to_string()));
        }
        let mut matches = Vec::new();
        let entries = std::fs::read_dir(sessions_dir)
            .map_err(|e| SessionError::Read(format!("Failed to read sessions directory: {e}")))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem.starts_with(session_id_or_prefix) {
                matches.push(path);
            }
        }
        match matches.len() {
            0 => Err(SessionError::NotFound(session_id_or_prefix.to_string())),
            1 => Ok(matches.remove(0)),
            _ => Err(SessionError::Invalid(format!(
                "Ambiguous session prefix '{session_id_or_prefix}' matched {} sessions",
                matches.len()
            ))),
        }
    }

    /// List all saved sessions' metadata.
    pub fn list(sessions_dir: &Path) -> Result<Vec<SessionMetadata>, SessionError> {
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();

        let entries = std::fs::read_dir(sessions_dir)
            .map_err(|e| SessionError::Read(format!("Failed to read sessions directory: {e}")))?;

        for entry in entries {
            let entry = entry
                .map_err(|e| SessionError::Read(format!("Failed to read directory entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let line = match content.lines().next() {
                Some(l) => l,
                None => continue,
            };

            if let Ok(session) = serde_json::from_str::<Session>(line) {
                sessions.push(SessionMetadata {
                    session_id: session.session_id,
                    model: session.model,
                    message_count: session.messages.len(),
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                    total_usage: session.total_usage,
                    title: session.title,
                });
            }
        }

        // Sort by updated_at descending (most recent first)
        #[allow(clippy::unnecessary_sort_by)]
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    /// Get a short display string for the session ID (first 8 chars).
    pub fn short_id(&self) -> &str {
        if self.session_id.len() >= 8 {
            &self.session_id[..8]
        } else {
            &self.session_id
        }
    }

    /// Estimate the total token count (rough: 1 token ≈ 4 chars).
    pub fn estimated_tokens(&self) -> u64 {
        let total_chars: usize = self
            .messages
            .iter()
            .map(|m| {
                m.content
                    .iter()
                    .map(|b| match b {
                        pigs_core::ContentBlock::Text { text } => text.len(),
                        pigs_core::ContentBlock::ToolUse { input, .. } => {
                            serde_json::to_string(input).map(|s| s.len()).unwrap_or(0)
                        }
                        pigs_core::ContentBlock::ToolResult { output, .. } => output.len(),
                    })
                    .sum::<usize>()
            })
            .sum();

        (total_chars as u64) / 4
    }
}

/// Build a short title from free-form user text.
pub fn auto_title_from_text(text: &str) -> String {
    let cleaned = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("untitled")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 60 {
        let truncated: String = collapsed.chars().take(57).collect();
        format!("{truncated}...")
    } else if collapsed.is_empty() {
        "untitled".to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pigs_core::{ContentBlock, MessageRole};

    #[test]
    fn test_session_creation() {
        let session = Session::new("gpt-4o");
        assert!(!session.session_id.is_empty());
        assert_eq!(session.model, "gpt-4o");
        assert_eq!(session.message_count(), 0);
        assert!(!session.dirty);
    }

    #[test]
    fn test_add_message() {
        let mut session = Session::new("gpt-4o");
        session.add_message(Message::user("Hello"));
        assert_eq!(session.message_count(), 1);
        assert!(session.dirty);
        assert_eq!(session.title.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_auto_title() {
        assert_eq!(auto_title_from_text("  hello world  "), "hello world");
        let long = "x".repeat(80);
        assert!(auto_title_from_text(&long).ends_with("..."));
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = std::env::temp_dir().join("pigs_test_sessions");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut session = Session::new("test-model");
        session.add_message(Message::user("Hello"));
        session.add_message(Message::assistant(vec![ContentBlock::text("Hi there!")]));

        session.save(&temp_dir).unwrap();
        assert!(!session.dirty);

        let loaded = Session::load(&temp_dir, &session.session_id).unwrap();
        assert_eq!(loaded.session_id, session.session_id);
        assert_eq!(loaded.model, "test-model");
        assert_eq!(loaded.message_count(), 2);
        assert_eq!(loaded.messages[0].role, MessageRole::User);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_not_found() {
        let result = Session::load(std::path::Path::new("/tmp"), "nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_estimated_tokens() {
        let mut session = Session::new("test");
        session.add_message(Message::user("Hello world")); // 11 chars ≈ 2 tokens
        let tokens = session.estimated_tokens();
        assert!((2..=3).contains(&tokens));
    }

    #[test]
    fn test_short_id() {
        let session = Session::new("test");
        assert_eq!(session.short_id().len(), 8);
    }

    #[test]
    fn test_delete_and_prefix_load() {
        let temp_dir =
            std::env::temp_dir().join(format!("pigs_test_sessions_del_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut session = Session::new("test-model");
        session.add_message(Message::user("hello delete me"));
        session.save(&temp_dir).unwrap();
        let prefix = session.short_id().to_string();

        let loaded = Session::load(&temp_dir, &prefix).unwrap();
        assert_eq!(loaded.session_id, session.session_id);

        let path = Session::delete(&temp_dir, &prefix).unwrap();
        assert!(!path.exists());
        assert!(Session::load(&temp_dir, &prefix).is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
