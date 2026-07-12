//! Permission policy — the decision engine for tool access control.

use std::collections::{HashMap, HashSet};

use crate::{PermissionMode, PermissionPrompter};

/// The outcome of a permission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The tool is allowed to execute.
    Allow,
    /// The tool is denied.
    Deny { reason: String },
    /// The user should be asked for approval.
    Ask,
}

/// A request for permission to execute a tool, presented to the user.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub required_mode: PermissionMode,
    pub description: String,
}

/// The permission policy controls which tools can be executed.
#[derive(Clone)]
pub struct PermissionPolicy {
    /// The active permission mode.
    pub active_mode: PermissionMode,
    /// Per-tool minimum required permission mode.
    pub tool_requirements: HashMap<String, PermissionMode>,
    /// Tools that are unconditionally denied.
    pub denied_tools: HashSet<String>,
    /// If true, all tool executions require user approval.
    pub always_ask: bool,
    /// If true, skip all permission checks (allow everything).
    pub allow_all: bool,
}

impl PermissionPolicy {
    /// Create a new policy with the given active mode.
    pub fn new(mode: PermissionMode) -> Self {
        PermissionPolicy {
            active_mode: mode,
            tool_requirements: HashMap::new(),
            denied_tools: HashSet::new(),
            always_ask: false,
            allow_all: false,
        }
    }

    /// Set a required permission mode for a specific tool.
    pub fn with_tool_requirement(mut self, tool_name: impl Into<String>, mode: PermissionMode) -> Self {
        self.tool_requirements.insert(tool_name.into(), mode);
        self
    }

    /// Deny a specific tool unconditionally.
    pub fn deny_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.denied_tools.insert(tool_name.into());
        self
    }

    /// Enable "always ask" mode — every tool execution requires approval.
    pub fn always_ask(mut self) -> Self {
        self.always_ask = true;
        self
    }

    /// Enable "allow all" mode — skip all permission checks.
    pub fn allow_all(mut self) -> Self {
        self.allow_all = true;
        self
    }

    /// Set the active permission mode.
    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.active_mode = mode;
    }

    /// Check if a tool can be executed, potentially asking the user for approval.
    pub fn check(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        // Allow all bypasses everything
        if self.allow_all {
            return PermissionOutcome::Allow;
        }

        // Check deny list
        if self.denied_tools.contains(tool_name) {
            return PermissionOutcome::Deny {
                reason: format!("Tool '{tool_name}' is in the deny list"),
            };
        }

        // Always ask mode
        if self.always_ask {
            return self.ask_user(tool_name, tool_input, prompter);
        }

        // Get required mode for this tool
        let required = self
            .tool_requirements
            .get(tool_name)
            .cloned()
            .unwrap_or(PermissionMode::ReadOnly);

        // If active mode is sufficient, allow
        if self.active_mode >= required {
            return PermissionOutcome::Allow;
        }

        // Need to escalate — ask the user
        self.ask_user(tool_name, tool_input, prompter)
    }

    /// Check if a file path is within the workspace root.
    pub fn check_file_path(&self, path: &std::path::Path, workspace_root: &std::path::Path) -> Result<(), String> {
        let canonical_path = path
            .canonicalize()
            .map_err(|e| format!("Cannot canonicalize path '{path:?}': {e}"))?;

        let canonical_root = workspace_root
            .canonicalize()
            .map_err(|e| format!("Cannot canonicalize workspace root '{workspace_root:?}': {e}"))?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(format!(
                "Path '{canonical_path:?}' is outside the workspace root '{canonical_root:?}'"
            ));
        }

        Ok(())
    }

    fn ask_user(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        prompter: Option<&mut dyn PermissionPrompter>,
    ) -> PermissionOutcome {
        let required = self
            .tool_requirements
            .get(tool_name)
            .cloned()
            .unwrap_or(PermissionMode::ReadOnly);

        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            tool_input: tool_input.clone(),
            required_mode: required.clone(),
            description: format!("Tool '{tool_name}' requires {required} permission"),
        };

        match prompter {
            Some(p) => match p.decide(&request) {
                crate::PermissionDecision::Allow => PermissionOutcome::Allow,
                crate::PermissionDecision::Deny { reason } => PermissionOutcome::Deny { reason },
            },
            None => PermissionOutcome::Deny {
                reason: format!(
                    "Tool '{tool_name}' requires {required} permission but no prompter is available"
                ),
            },
        }
    }
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self::new(PermissionMode::WorkspaceWrite)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_allow_all() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly).allow_all();
        let outcome = policy.check("bash", &serde_json::json!({}), None);
        assert_eq!(outcome, PermissionOutcome::Allow);
    }

    #[test]
    fn test_deny_list() {
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess).deny_tool("rm");
        let outcome = policy.check("rm", &serde_json::json!({}), None);
        assert!(matches!(outcome, PermissionOutcome::Deny { .. }));
    }

    #[test]
    fn test_mode_sufficient() {
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let outcome = policy.check("bash", &serde_json::json!({}), None);
        assert_eq!(outcome, PermissionOutcome::Allow);
    }

    #[test]
    fn test_mode_insufficient_no_prompter() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess);
        let outcome = policy.check("bash", &serde_json::json!({}), None);
        assert!(matches!(outcome, PermissionOutcome::Deny { .. }));
    }

    #[test]
    fn test_readonly_allows_readonly_tool() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("read_file", PermissionMode::ReadOnly);
        let outcome = policy.check("read_file", &serde_json::json!({}), None);
        assert_eq!(outcome, PermissionOutcome::Allow);
    }
}
