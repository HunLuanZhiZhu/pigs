//! Permission modes — escalation levels for tool access control.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// The permission mode determines what level of access the agent has.
/// Modes are ordered by escalation: ReadOnly < WorkspaceWrite < DangerFullAccess.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Read-only: only tools that do not modify state (read_file, grep, glob, etc.)
    ReadOnly,
    /// Write within the workspace: tools that modify files within the workspace root.
    WorkspaceWrite,
    /// Full access: any tool, including bash commands with no restrictions.
    DangerFullAccess,
}

impl PermissionMode {
    /// Get a human-readable description of the mode.
    pub fn description(&self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "Read-only — tools that do not modify state",
            PermissionMode::WorkspaceWrite => "Workspace write — file modifications within workspace",
            PermissionMode::DangerFullAccess => "Full access — all tools including shell commands",
        }
    }

    /// Get the short name used in CLI/config.
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "readonly",
            PermissionMode::WorkspaceWrite => "workspace_write",
            PermissionMode::DangerFullAccess => "danger",
        }
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "readonly" | "read-only" | "read_only" | "ro" => Ok(PermissionMode::ReadOnly),
            "workspace_write" | "workspace-write" | "workspace" | "write" | "ww" => {
                Ok(PermissionMode::WorkspaceWrite)
            }
            "danger" | "danger_full_access" | "full" | "full_access" | "dangerous" => {
                Ok(PermissionMode::DangerFullAccess)
            }
            _ => Err(format!(
                "Unknown permission mode: '{s}'. Valid: readonly, workspace_write, danger"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_ordering() {
        assert!(PermissionMode::ReadOnly < PermissionMode::WorkspaceWrite);
        assert!(PermissionMode::WorkspaceWrite < PermissionMode::DangerFullAccess);
        assert!(PermissionMode::ReadOnly < PermissionMode::DangerFullAccess);
    }

    #[test]
    fn test_from_str() {
        assert_eq!(PermissionMode::from_str("readonly").unwrap(), PermissionMode::ReadOnly);
        assert_eq!(
            PermissionMode::from_str("workspace_write").unwrap(),
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            PermissionMode::from_str("danger").unwrap(),
            PermissionMode::DangerFullAccess
        );
        assert!(PermissionMode::from_str("invalid").is_err());
    }

    #[test]
    fn test_as_str_roundtrip() {
        for mode in &[PermissionMode::ReadOnly, PermissionMode::WorkspaceWrite, PermissionMode::DangerFullAccess] {
            let s = mode.as_str();
            let parsed = PermissionMode::from_str(s).unwrap();
            assert_eq!(mode, &parsed);
        }
    }
}
