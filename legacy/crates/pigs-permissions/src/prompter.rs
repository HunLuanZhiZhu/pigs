//! Permission prompter — interactive approval interface.

use std::io::{self, Write};

use crate::PermissionRequest;

/// The decision returned by a permission prompter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
}

/// Trait for prompting the user for tool execution approval.
pub trait PermissionPrompter: Send {
    /// Ask the user whether to allow a tool execution.
    fn decide(&mut self, request: &PermissionRequest) -> PermissionDecision;
}

/// A CLI-based permission prompter that asks the user interactively.
pub struct CliPermissionPrompter {
    /// If true, automatically allow all requests without prompting.
    auto_allow: bool,
}

impl CliPermissionPrompter {
    /// Create a new CLI prompter.
    pub fn new() -> Self {
        CliPermissionPrompter { auto_allow: false }
    }

    /// Create a prompter that automatically allows all requests.
    pub fn auto_allow() -> Self {
        CliPermissionPrompter { auto_allow: true }
    }

    /// Prompt the user for a yes/no decision.
    fn prompt_yes_no(&self, message: &str) -> bool {
        if self.auto_allow {
            return true;
        }

        print!("{message} [y/N] ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }

        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

impl Default for CliPermissionPrompter {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionPrompter for CliPermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionDecision {
        let input_str = if request.tool_input.is_null() {
            String::new()
        } else {
            serde_json::to_string_pretty(&request.tool_input).unwrap_or_else(|_| String::new())
        };

        let message = format!(
            "\n┌─ Permission Request ─────────────────────────\n\
             │ Tool: {}\n\
             │ Required: {}\n\
             │ Input: {}\n\
             └────────────────────────────────────────────────\n\
             Allow this action?",
            request.tool_name, request.required_mode, input_str
        );

        if self.prompt_yes_no(&message) {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny {
                reason: "User denied the request".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::PermissionMode;

    #[test]
    fn test_auto_allow_prompter() {
        let mut prompter = CliPermissionPrompter::auto_allow();
        let request = PermissionRequest {
            tool_name: "bash".into(),
            tool_input: serde_json::json!({"command": "rm -rf /"}),
            required_mode: PermissionMode::DangerFullAccess,
            description: "test".into(),
        };
        let decision = prompter.decide(&request);
        assert_eq!(decision, PermissionDecision::Allow);
    }
}
