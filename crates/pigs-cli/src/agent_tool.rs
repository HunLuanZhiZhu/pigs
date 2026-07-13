//! Agent sub-agent tool — delegate subtasks to a nested agent instance.
//!
//! This tool creates a temporary agent with its own conversation context,
//! limited tools, and a specific task. The sub-agent runs to completion and
//! returns its final output.
//!
//! 临时禁用：此模块在 `sub_agent` feature 开启时才编译。
//! Currently disabled: this module only compiles when the `sub_agent` feature is enabled.
#![cfg(feature = "sub_agent")]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pigs_core::{
    ApiClient, ApiRequest, ContentBlock, Message, ToolError, ToolHandler,
    ToolResult, ToolSpec,
};

/// Tool for delegating subtasks to a nested agent.
///
/// The sub-agent gets its own conversation context (no history from the parent),
/// a restricted set of read-only tools, and a maximum turn limit. It runs to
/// completion and returns its final text output.
pub struct AgentTool {
    api_client: Arc<dyn ApiClient>,
}

impl AgentTool {
    /// Create a new Agent tool with the given API client (shared with the parent agent).
    pub fn new(api_client: Arc<dyn ApiClient>) -> Self {
        AgentTool { api_client }
    }
}

impl ToolHandler for AgentTool {
    fn name(&self) -> &str {
        "agent"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "agent",
            "Launch a sub-agent to handle a specific task. The sub-agent runs with its own \
             conversation context (no parent history), limited read-only tools, and a \
             maximum turn limit. Use this for complex subtasks that benefit from \
             focused context, such as searching code, analyzing files, or gathering information.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task description for the sub-agent"
                    },
                    "max_turns": {
                        "type": "integer",
                        "description": "Maximum turns for the sub-agent (default: 10)",
                        "default": 10
                    }
                },
                "required": ["task"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let task = input
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'task' field".into()))?;

            let max_turns: u32 = input
                .get("max_turns")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or(10)
                .min(20); // Hard cap at 20 turns

            // Create a sub-agent with read-only tools only
            let tool_registry = create_readonly_registry();

            // Build a focused system prompt for the sub-agent
            let system_prompt = format!(
                "You are a sub-agent. Your task is: {task}\n\n\
                 Be concise and focused. Use tools to gather information. \
                 Report your findings clearly when done."
            );

            // Create the sub-agent's messages
            let mut messages = vec![Message::user(task)];

            let mut iteration: u32 = 0;
            let mut final_output = String::new();

            loop {
                iteration += 1;
                if iteration > max_turns {
                    final_output = format!(
                        "{final_output}\n\n(Sub-agent reached max turn limit of {max_turns})"
                    );
                    break;
                }

                let tool_defs = tool_registry.definitions();

                let request = ApiRequest::new(self.api_client.model(), messages.clone())
                    .with_system_prompt(&system_prompt)
                    .with_tools(tool_defs)
                    .with_max_tokens(4096)
                    .with_temperature(0.7);

                let response = match self.api_client.send_message(request).await {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(ToolResult::error(format!(
                            "Sub-agent API error: {e}"
                        )));
                    }
                };

                // Extract text content
                let text = response.text_content();
                if !text.is_empty() {
                    if !final_output.is_empty() {
                        final_output.push_str("\n\n");
                    }
                    final_output.push_str(&text);
                }

                // Check for tool uses
                let tool_uses: Vec<(String, String, serde_json::Value)> = response
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => {
                            Some((id.clone(), name.clone(), input.clone()))
                        }
                        _ => None,
                    })
                    .collect();

                if tool_uses.is_empty() {
                    // No more tool calls — sub-agent is done
                    break;
                }

                // Execute tools and collect results
                let mut new_messages = messages.clone();
                new_messages.push(Message::assistant(response.content.clone()));

                for (tool_id, tool_name, tool_input) in &tool_uses {
                    let result = tool_registry.execute(tool_name, tool_input.clone()).await;
                    let (output, is_error) = match result {
                        Ok(r) => (r.output, r.is_error),
                        Err(e) => (format!("Tool error: {e}"), true),
                    };
                    new_messages.push(Message::tool_result(tool_id, &output, is_error));
                }

                // Replace messages for next iteration
                // Note: this doesn't work because messages was borrowed — need to restructure
                // Actually, we cloned messages above, so we can reassign
                drop(std::mem::replace(&mut messages, new_messages));
            }

            Ok(ToolResult::success(final_output))
        })
    }
}

/// Create a tool registry with only read-only tools (for sub-agents).
fn create_readonly_registry() -> pigs_core::ToolRegistry {
    let mut registry = pigs_core::ToolRegistry::new();

    // Only read-only tools for sub-agents
    registry.register(Box::new(pigs_tools::read_file::ReadFileTool::new()));
    registry.register(Box::new(pigs_tools::grep::GrepTool::new()));
    registry.register(Box::new(pigs_tools::glob::GlobTool::new()));
    registry.register(Box::new(pigs_tools::list_files::ListFilesTool::new()));
    registry.register(Box::new(pigs_tools::web_fetch::WebFetchTool::new()));

    registry
}
