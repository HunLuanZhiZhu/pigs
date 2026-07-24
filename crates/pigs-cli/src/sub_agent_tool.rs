//! Spawn tool — creates one or more sub-agents from a single tool call.
//!
//! The main agent calls this tool to delegate tasks to sub-agents.
//! Sub-agents can be:
//! - "general" (default): fully equivalent to the main agent
//! - Custom types: loaded from ~/.pig/agents/*.md or .pig/agents/*.md
//!
//! Custom agent types are defined as Markdown files with YAML frontmatter,
//! inspired by OpenCode's agent system.

use std::sync::Arc;

use pigs_core::{ToolFuture, ToolHandler, ToolResult, ToolSpec};
use serde_json::json;

use crate::sub_agent::{SubAgentManager, SubAgentMode};

/// Tool that spawns sub-agents.
pub struct SpawnTool {
    /// Shared sub-agent manager.
    pub manager: Arc<std::sync::Mutex<SubAgentManager>>,
    /// Model name for sub-agent sessions (used when agent type has no model override).
    pub model: String,
    /// Available custom agent definitions (loaded at startup).
    pub definitions: Vec<pigs_config::sub_agent_def::SubAgentDefinition>,
    /// Sessions directory for persisting sub-agent sessions.
    pub sessions_dir: std::path::PathBuf,
}

impl SpawnTool {
    pub fn new(
        manager: Arc<std::sync::Mutex<SubAgentManager>>,
        model: String,
        definitions: Vec<pigs_config::sub_agent_def::SubAgentDefinition>,
        sessions_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            manager,
            model,
            definitions,
            sessions_dir,
        }
    }

    /// Get the description of all available agent types for the LLM.
    fn agent_types_description(&self) -> String {
        let mut desc = String::from("\n\nAvailable agent types:\n");
        desc.push_str("- general: Full-featured agent with all tools (default)\n");
        for def in &self.definitions {
            desc.push_str(&format!("{}\n", def.description_for_llm()));
        }
        desc
    }
}

impl ToolHandler for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn spec(&self) -> ToolSpec {
        let base_description = "Create one or more sub-agents to work on tasks. Sub-agents are fully equivalent to the main agent (same tools, same model, same pig phased orchestration). They can recursively create their own sub-agents. Use 'foreground' mode to wait for results, or 'background' mode to run asynchronously.";

        let description = format!("{}{}", base_description, self.agent_types_description());

        ToolSpec {
            name: "spawn".to_string(),
            description,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "description": "List of sub-agents to create",
                        "items": {
                            "type": "object",
                            "properties": {
                                "task": {
                                    "type": "string",
                                    "description": "Task description for the sub-agent"
                                },
                                "agent_type": {
                                    "type": "string",
                                    "default": "general",
                                    "description": "Agent type (e.g. 'general', 'scout', 'reviewer'). Use 'general' for full-featured agent."
                                },
                                "mode": {
                                    "type": "string",
                                    "enum": ["foreground", "background"],
                                    "default": "foreground",
                                    "description": "Whether to wait for the result (foreground) or run asynchronously (background)"
                                },
                                "share_context": {
                                    "type": "boolean",
                                    "default": false,
                                    "description": "Whether to share the parent agent's conversation context"
                                }
                            },
                            "required": ["task"]
                        }
                    }
                },
                "required": ["agents"]
            }),
        }
    }

    fn execute<'a>(&'a self, input: serde_json::Value) -> ToolFuture<'a> {
        let manager = self.manager.clone();
        let model = self.model.clone();
        let definitions = self.definitions.clone();

        Box::pin(async move {
            let agents_arr = match input.get("agents").and_then(|v| v.as_array()) {
                Some(arr) => arr,
                None => {
                    return Ok(ToolResult::error("'agents' field is required and must be an array"));
                }
            };

            let mut requests: Vec<crate::sub_agent::SpawnRequest> = Vec::new();
            for agent_def in agents_arr {
                let task = agent_def
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if task.is_empty() {
                    return Ok(ToolResult::error("each agent must have a 'task' field"));
                }

                let mode = match agent_def
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("foreground")
                {
                    "background" => SubAgentMode::Background,
                    _ => SubAgentMode::Foreground,
                };

                let agent_type = agent_def
                    .get("agent_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("general");

                let def = definitions.iter().find(|d| d.name == agent_type);
                let share_context = agent_def
                    .get("share_context")
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(|| def.map(|d| d.share_context).unwrap_or(false));

                requests.push(crate::sub_agent::SpawnRequest {
                    task,
                    mode,
                    share_context,
                    agent_type: agent_type.to_string(),
                });
            }

            if requests.is_empty() {
                return Ok(ToolResult::error("'agents' array must not be empty"));
            }

            // Spawn the sub-agents with type information
            let ids = {
                let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
                mgr.spawn_with_types(requests.clone(), &model, &[], &definitions, &self.sessions_dir)
            };

            // Separate foreground and background
            let mut foreground_ids = Vec::new();
            let mut background_ids = Vec::new();
            for (id, req) in ids.iter().zip(requests.iter()) {
                match req.mode {
                    SubAgentMode::Foreground => foreground_ids.push(id.clone()),
                    SubAgentMode::Background => background_ids.push(id.clone()),
                }
            }

            let mut result = format!("Created {} sub-agent(s):\n", ids.len());
            for (id, req) in ids.iter().zip(requests.iter()) {
                result.push_str(&format!(
                    "  {} [{}/{}] task=\"{}\" context={}\n",
                    id,
                    req.mode.as_str(),
                    req.agent_type,
                    req.task,
                    if req.share_context { "shared" } else { "independent" }
                ));
            }

            if !foreground_ids.is_empty() {
                result.push_str(&format!("\nForeground: {}\n", foreground_ids.join(", ")));
            }
            if !background_ids.is_empty() {
                result.push_str(&format!("\nBackground: {}\n", background_ids.join(", ")));
            }

            {
                let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
                for id in &foreground_ids {
                    mgr.mark_running(id);
                }
            }

            Ok(ToolResult::success(result))
        })
    }
}
