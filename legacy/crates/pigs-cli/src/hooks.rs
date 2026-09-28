//! Tool lifecycle hooks — run shell commands before/after tool execution.
//!
//! Environment variables provided to hooks:
//! - `PIG_TOOL_NAME` — tool name
//! - `PIG_TOOL_INPUT` — tool input JSON
//! - `PIG_HOOK_EVENT` — `pre_tool_use` or `post_tool_use`
//! - `PIG_TOOL_OUTPUT` — tool output text (post only)
//! - `PIG_TOOL_IS_ERROR` — `true`/`false` (post only)
//!
//! Pre-tool hooks: exit code 0 allows, non-zero denies.

use std::process::Stdio;
use std::time::Duration;

use pigs_config::{HookEntry, HooksConfig};
use tracing::{debug, warn};

/// Result of running pre-tool hooks.
#[derive(Debug, Clone)]
pub enum HookDecision {
    Allow,
    Deny { reason: String },
}

/// Run all matching pre-tool-use hooks.
pub async fn run_pre_tool_hooks(
    hooks: &HooksConfig,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> HookDecision {
    for hook in hooks.pre_tool_use.iter().filter(|h| h.enabled) {
        if !matcher_matches(&hook.matcher, tool_name) {
            continue;
        }
        match run_hook(hook, "pre_tool_use", tool_name, tool_input, None, false).await {
            Ok(0) => {
                debug!(tool = tool_name, matcher = %hook.matcher, "Pre-tool hook allowed");
            }
            Ok(code) => {
                let reason = format!(
                    "Pre-tool hook denied tool '{tool_name}' (matcher '{}', exit {code})",
                    hook.matcher
                );
                warn!(%reason);
                return HookDecision::Deny { reason };
            }
            Err(e) => {
                let reason = format!(
                    "Pre-tool hook failed for tool '{tool_name}' (matcher '{}'): {e}",
                    hook.matcher
                );
                warn!(%reason);
                return HookDecision::Deny { reason };
            }
        }
    }
    HookDecision::Allow
}

/// Run all matching post-tool-use hooks (best-effort, never blocks the agent loop).
pub async fn run_post_tool_hooks(
    hooks: &HooksConfig,
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_output: &str,
    is_error: bool,
) {
    for hook in hooks.post_tool_use.iter().filter(|h| h.enabled) {
        if !matcher_matches(&hook.matcher, tool_name) {
            continue;
        }
        match run_hook(
            hook,
            "post_tool_use",
            tool_name,
            tool_input,
            Some(tool_output),
            is_error,
        )
        .await
        {
            Ok(code) => {
                debug!(
                    tool = tool_name,
                    matcher = %hook.matcher,
                    exit = code,
                    "Post-tool hook finished"
                );
            }
            Err(e) => {
                warn!(
                    tool = tool_name,
                    matcher = %hook.matcher,
                    error = %e,
                    "Post-tool hook failed"
                );
            }
        }
    }
}

fn matcher_matches(matcher: &str, tool_name: &str) -> bool {
    if matcher == "*" {
        return true;
    }
    if let Some(prefix) = matcher.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    matcher == tool_name
}

async fn run_hook(
    hook: &HookEntry,
    event: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_output: Option<&str>,
    is_error: bool,
) -> Result<i32, String> {
    let input_json = serde_json::to_string(tool_input).unwrap_or_else(|_| "{}".to_string());

    #[cfg(target_os = "windows")]
    let (program, flag) = ("cmd", "/C");
    #[cfg(not(target_os = "windows"))]
    let (program, flag) = ("sh", "-c");

    let mut cmd = tokio::process::Command::new(program);
    cmd.arg(flag)
        .arg(&hook.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PIG_HOOK_EVENT", event)
        .env("PIG_TOOL_NAME", tool_name)
        .env("PIG_TOOL_INPUT", input_json)
        .env(
            "PIG_TOOL_IS_ERROR",
            if is_error { "true" } else { "false" },
        );

    if let Some(output) = tool_output {
        // Bound env size for very large tool outputs
        let truncated = if output.len() > 16_000 {
            format!("{}...(truncated)", &output[..16_000])
        } else {
            output.to_string()
        };
        cmd.env("PIG_TOOL_OUTPUT", truncated);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn hook command: {e}"))?;

    let output = tokio::time::timeout(
        Duration::from_secs(hook.timeout.max(1)),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("Hook timed out after {}s", hook.timeout))?
    .map_err(|e| format!("Hook process failed: {e}"))?;

    let code = output.status.code().unwrap_or(-1);
    if !output.stdout.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(hook_stdout = %stdout.trim(), "Hook stdout");
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(hook_stderr = %stderr.trim(), "Hook stderr");
    }
    Ok(code)
}

/// Summarize configured hooks for display.
pub fn summarize_hooks(hooks: &HooksConfig) -> String {
    let mut lines = Vec::new();
    if hooks.pre_tool_use.is_empty() && hooks.post_tool_use.is_empty() {
        return "No hooks configured.".to_string();
    }
    if !hooks.pre_tool_use.is_empty() {
        lines.push(format!("pre_tool_use ({}):", hooks.pre_tool_use.len()));
        for h in &hooks.pre_tool_use {
            let status = if h.enabled { "on" } else { "off" };
            lines.push(format!(
                "  [{status}] matcher={} timeout={}s\n    cmd: {}",
                h.matcher, h.timeout, h.command
            ));
        }
    }
    if !hooks.post_tool_use.is_empty() {
        lines.push(format!("post_tool_use ({}):", hooks.post_tool_use.len()));
        for h in &hooks.post_tool_use {
            let status = if h.enabled { "on" } else { "off" };
            lines.push(format!(
                "  [{status}] matcher={} timeout={}s\n    cmd: {}",
                h.matcher, h.timeout, h.command
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_matcher() {
        assert!(matcher_matches("*", "bash"));
        assert!(matcher_matches("bash", "bash"));
        assert!(!matcher_matches("bash", "read"));
        assert!(matcher_matches("mcp_*", "mcp_fs_read"));
        assert!(!matcher_matches("mcp_*", "bash"));
    }
}
