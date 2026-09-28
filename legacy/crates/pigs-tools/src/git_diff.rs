//! Git diff tool — show working tree / staged changes.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for viewing git diffs.
pub struct GitDiffTool;

impl GitDiffTool {
    pub fn new() -> Self {
        GitDiffTool
    }
}

impl Default for GitDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "git_diff",
            "Show git working tree changes. Useful for reviewing uncommitted modifications \
             before committing or summarizing what changed. Defaults to unstaged diff; \
             can include staged changes or a specific path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional path to limit the diff"
                    },
                    "staged": {
                        "type": "boolean",
                        "description": "If true, show staged changes only. Default: false (unstaged).",
                        "default": false
                    },
                    "stat": {
                        "type": "boolean",
                        "description": "If true, show --stat summary instead of full patch. Default: false.",
                        "default": false
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Maximum characters of diff output to return. Default: 30000.",
                        "default": 30000
                    }
                }
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let path = input.get("path").and_then(|v| v.as_str());
            let staged = input
                .get("staged")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let stat = input.get("stat").and_then(|v| v.as_bool()).unwrap_or(false);
            let max_chars = input
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(30_000) as usize;

            let mut args = vec!["diff".to_string()];
            if staged {
                args.push("--cached".to_string());
            }
            if stat {
                args.push("--stat".to_string());
            }
            args.push("--".to_string());
            if let Some(p) = path {
                args.push(p.to_string());
            }

            let mut cmd = tokio::process::Command::new("git");
            cmd.args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let child = cmd
                .spawn()
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn git: {e}")))?;

            let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
                .await
                .map_err(|_| ToolError::Timeout(30))?
                .map_err(|e| ToolError::ExecutionFailed(format!("git failed: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !output.status.success() {
                let msg = if stderr.is_empty() {
                    format!("git diff failed with exit code {:?}", output.status.code())
                } else {
                    stderr
                };
                return Ok(ToolResult::error(msg));
            }

            if stdout.trim().is_empty() {
                return Ok(ToolResult::success(
                    if staged {
                        "No staged changes."
                    } else {
                        "No unstaged changes."
                    }
                    .to_string(),
                ));
            }

            let truncated = if stdout.len() > max_chars {
                format!(
                    "{}...\n\n[diff truncated at {max_chars} chars; total {} chars]",
                    &stdout[..max_chars],
                    stdout.len()
                )
            } else {
                stdout
            };

            Ok(ToolResult::success(truncated))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_tool_name() {
        assert_eq!(GitDiffTool::new().name(), "git_diff");
    }
}
