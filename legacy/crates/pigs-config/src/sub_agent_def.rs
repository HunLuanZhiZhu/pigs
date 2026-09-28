//! Custom sub-agent definitions — Markdown files with YAML frontmatter.
//!
//! Inspired by OpenCode's agent system. Sub-agents are defined as `.md` files
//! in `~/.pig/agents/` (global) or `.pig/agents/` (project-level).
//!
//! Each file has YAML frontmatter with agent metadata, and the body becomes
//! the sub-agent's system prompt (replacing the default pig prompt).
//!
//! Example file (`~/.pig/agents/scout.md`):
//! ```markdown
//! ---
//! description: Fast reconnaissance agent for exploring codebases
//! model: haiku
//! tools:
//!   - read
//!   - grep
//!   - find
//!   - ls
//! share_context: false
//! ---
//! You are a fast reconnaissance agent. Your job is to quickly explore
//! a codebase and report back key findings...
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A custom sub-agent definition loaded from a Markdown file.
#[derive(Debug, Clone)]
pub struct SubAgentDefinition {
    /// Agent type name (derived from filename, e.g. "scout" from "scout.md").
    pub name: String,
    /// When to use this agent (shown to the main agent in the spawn tool description).
    pub description: String,
    /// Custom system prompt (the Markdown body after frontmatter).
    /// If empty, the sub-agent uses the default pig system prompt.
    pub system_prompt: String,
    /// Optional model override (e.g. "haiku", "gpt-4o-mini").
    /// If None, inherits the main agent's model.
    pub model: Option<String>,
    /// Restricted tool set. If empty, all tools are available.
    /// If non-empty, only the listed tools are available to this sub-agent.
    pub tools: Vec<String>,
    /// Whether to share the parent's conversation context by default.
    /// Can be overridden at spawn time.
    pub share_context: bool,
    /// File path this definition was loaded from.
    pub source: PathBuf,
}

impl SubAgentDefinition {
    /// Parse a Markdown file with YAML frontmatter into a SubAgentDefinition.
    pub fn from_markdown(path: &Path, content: &str) -> Option<Self> {
        // Extract filename without extension as the agent name
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();

        // Split frontmatter and body
        let (frontmatter, body) = split_frontmatter(content);

        // Parse frontmatter (simple YAML parsing — no external dependency)
        let fm = parse_simple_yaml(&frontmatter);

        let description = fm
            .get("description")
            .cloned()
            .unwrap_or_default();

        let model = fm.get("model").cloned();

        // Parse tools list
        let tools = fm
            .get("tools")
            .map(|s| {
                s.lines()
                    .map(|line| line.trim().trim_start_matches('-').trim().trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let share_context = fm
            .get("share_context")
            .map(|s| s.trim() == "true")
            .unwrap_or(false);

        let system_prompt = body.trim().to_string();

        Some(Self {
            name,
            description,
            system_prompt,
            model,
            tools,
            share_context,
            source: path.to_path_buf(),
        })
    }

    /// Check if a tool is allowed for this agent.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if self.tools.is_empty() {
            true // No restriction = all tools allowed
        } else {
            self.tools.iter().any(|t| t == tool_name)
        }
    }

    /// Format the agent description for the spawn tool's LLM-facing text.
    pub fn description_for_llm(&self) -> String {
        let tools_desc = if self.tools.is_empty() {
            "all tools".to_string()
        } else {
            self.tools.join(", ")
        };
        let model_desc = self.model.as_deref().unwrap_or("(inherit)");
        format!(
            "- {}: {} [tools: {}, model: {}]",
            self.name, self.description, tools_desc, model_desc
        )
    }
}

/// Split a Markdown file into (frontmatter, body).
/// Frontmatter is delimited by `---` at the start of the file.
fn split_frontmatter(content: &str) -> (String, String) {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return (String::new(), content.to_string());
    }

    // Find the closing ---
    let rest = &content[3..]; // Skip opening ---
    if let Some(end) = rest.find("\n---") {
        let frontmatter = rest[..end].trim().to_string();
        let body = rest[end + 4..].trim().to_string();
        (frontmatter, body)
    } else {
        (String::new(), content.to_string())
    }
}

/// Simple YAML parser for flat key-value pairs and lists.
/// Supports:
///   key: value
///   key: true
///   tools:
///     - item1
///     - item2
fn parse_simple_yaml(yaml: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut current_key: Option<String> = None;
    let mut list_items: Vec<String> = Vec::new();

    for line in yaml.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // List item (under a list key)
        if trimmed.starts_with('-') && current_key.is_some() {
            let item = trimmed
                .trim_start_matches('-')
                .trim()
                .trim_matches('"')
                .to_string();
            list_items.push(item);
            continue;
        }

        // If we were collecting a list, save it
        if let Some(key) = current_key.take() {
            if !list_items.is_empty() {
                map.insert(key, list_items.join("\n"));
                list_items.clear();
            }
        }

        // Key: value
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_string();
            let value = trimmed[colon_pos + 1..].trim().trim_matches('"').to_string();

            if value.is_empty() {
                // Could be a list header
                current_key = Some(key);
            } else {
                map.insert(key, value);
            }
        }
    }

    // Save last list if any
    if let Some(key) = current_key.take() {
        if !list_items.is_empty() {
            map.insert(key, list_items.join("\n"));
        }
    }

    map
}

/// Load all sub-agent definitions from the standard directories.
///
/// Directories searched (first match wins for a given name):
/// 1. `{workspace}/.pig/agents/` (project-level)
/// 2. `~/.pig/agents/` (global user-level)
pub fn load_sub_agent_definitions(workspace: &Path) -> Vec<SubAgentDefinition> {
    let mut definitions = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    // Search directories in priority order (project first)
    let dirs = get_agent_directories(workspace);

    for dir in &dirs {
        if !dir.exists() {
            continue;
        }

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }

                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();

                if name.is_empty() || seen_names.contains(&name) {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Some(def) = SubAgentDefinition::from_markdown(&path, &content) {
                        seen_names.insert(name);
                        definitions.push(def);
                    }
                }
            }
        }
    }

    definitions
}

/// Get the agent definition directories in priority order.
fn get_agent_directories(workspace: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Project-level: {workspace}/.pigs/agents/
    dirs.push(workspace.join(".pigs").join("agents"));

    // Global: ~/.pigs/agents/
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".pigs").join("agents"));
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_basic() {
        let content = r#"---
description: A scout agent
model: haiku
share_context: false
---
You are a scout agent. Explore quickly."#;
        let path = Path::new("scout.md");
        let def = SubAgentDefinition::from_markdown(path, content).unwrap();

        assert_eq!(def.name, "scout");
        assert_eq!(def.description, "A scout agent");
        assert_eq!(def.model.as_deref(), Some("haiku"));
        assert!(!def.share_context);
        assert_eq!(def.system_prompt, "You are a scout agent. Explore quickly.");
        assert!(def.tools.is_empty());
    }

    #[test]
    fn parse_frontmatter_with_tools() {
        let content = r#"---
description: Read-only explorer
tools:
  - read
  - grep
  - find
  - ls
---
You are an explorer."#;
        let path = Path::new("explorer.md");
        let def = SubAgentDefinition::from_markdown(path, content).unwrap();

        assert_eq!(def.name, "explorer");
        assert_eq!(def.description, "Read-only explorer");
        assert_eq!(def.tools.len(), 4);
        assert!(def.tools.contains(&"read".to_string()));
        assert!(def.tools.contains(&"grep".to_string()));
    }

    #[test]
    fn parse_no_frontmatter() {
        let content = "Just a system prompt with no frontmatter.";
        let path = Path::new("simple.md");
        let def = SubAgentDefinition::from_markdown(path, content).unwrap();

        assert_eq!(def.name, "simple");
        assert!(def.description.is_empty());
        assert!(def.model.is_none());
        assert!(def.tools.is_empty());
        assert_eq!(def.system_prompt, "Just a system prompt with no frontmatter.");
    }

    #[test]
    fn tool_restriction() {
        let def = SubAgentDefinition {
            name: "restricted".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            tools: vec!["read".to_string(), "grep".to_string()],
            share_context: false,
            source: PathBuf::new(),
        };

        assert!(def.is_tool_allowed("read"));
        assert!(def.is_tool_allowed("grep"));
        assert!(!def.is_tool_allowed("bash"));
        assert!(!def.is_tool_allowed("write"));
    }

    #[test]
    fn no_tool_restriction() {
        let def = SubAgentDefinition {
            name: "unrestricted".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            model: None,
            tools: vec![],
            share_context: false,
            source: PathBuf::new(),
        };

        // No restriction = all tools allowed
        assert!(def.is_tool_allowed("read"));
        assert!(def.is_tool_allowed("bash"));
        assert!(def.is_tool_allowed("write"));
    }

    #[test]
    fn description_for_llm() {
        let def = SubAgentDefinition {
            name: "scout".to_string(),
            description: "Fast reconnaissance".to_string(),
            system_prompt: String::new(),
            model: Some("haiku".to_string()),
            tools: vec!["read".to_string(), "grep".to_string()],
            share_context: false,
            source: PathBuf::new(),
        };

        let desc = def.description_for_llm();
        assert!(desc.contains("scout"));
        assert!(desc.contains("Fast reconnaissance"));
        assert!(desc.contains("read, grep"));
        assert!(desc.contains("haiku"));
    }

    #[test]
    fn split_frontmatter_edge_cases() {
        // Empty content
        let (fm, body) = split_frontmatter("");
        assert!(fm.is_empty());

        // No frontmatter
        let (fm, body) = split_frontmatter("just body text");
        assert!(fm.is_empty());
        assert_eq!(body, "just body text");
    }

    #[test]
    fn parse_yaml_simple() {
        let yaml = "key1: value1\nkey2: value2";
        let map = parse_simple_yaml(yaml);
        assert_eq!(map.get("key1").map(|s| s.as_str()), Some("value1"));
        assert_eq!(map.get("key2").map(|s| s.as_str()), Some("value2"));
    }

    #[test]
    fn parse_yaml_boolean() {
        let yaml = "share_context: true";
        let map = parse_simple_yaml(yaml);
        assert_eq!(map.get("share_context").map(|s| s.as_str()), Some("true"));
    }
}
