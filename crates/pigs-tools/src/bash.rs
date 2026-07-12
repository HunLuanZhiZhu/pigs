//! Bash tool — execute shell commands with timeout.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for executing shell commands.
pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        BashTool
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "bash",
            "Execute a shell command and return its output. \
             Commands run in the current working directory with a configurable timeout. \
             Use this for running tests, building projects, inspecting files, etc.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120)",
                        "default": 120
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the command. Defaults to current directory."
                    }
                },
                "required": ["command"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let command = input
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'command' field".into()))?;

            let timeout_secs = input
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(120);

            let cwd = input.get("cwd").and_then(|v| v.as_str());

            // Determine the shell based on platform
            #[cfg(target_os = "windows")]
            let (program, flag) = ("cmd", "/C");
            #[cfg(not(target_os = "windows"))]
            let (program, flag) = ("sh", "-c");

            let mut cmd = tokio::process::Command::new(program);
            cmd.arg(flag).arg(command);

            if let Some(dir) = cwd {
                cmd.current_dir(dir);
            }

            // Don't inherit stdin
            cmd.stdin(std::process::Stdio::null());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            let child = cmd
                .spawn()
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn process: {e}")))?;

            let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
                .await
                .map_err(|_| ToolError::Timeout(timeout_secs))?;

            let output = output
                .map_err(|e| ToolError::ExecutionFailed(format!("Process failed: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);

            // Truncate very large outputs
            const MAX_OUTPUT: usize = 50_000;
            let stdout = if stdout.len() > MAX_OUTPUT {
                format!("{}... (truncated)", &stdout[..MAX_OUTPUT])
            } else {
                stdout
            };
            let stderr = if stderr.len() > MAX_OUTPUT {
                format!("{}... (truncated)", &stderr[..MAX_OUTPUT])
            } else {
                stderr
            };

            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }
            if !result.is_empty() {
                result.push_str(&format!("\n[exit code: {exit_code}]"));
            } else {
                result = format!("[exit code: {exit_code}]");
            }

            let is_error = !output.status.success();
            Ok(ToolResult {
                output: result,
                is_error,
            })
        })
    }
}
