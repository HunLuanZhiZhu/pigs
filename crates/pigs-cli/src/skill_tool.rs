//! Skill tool — load a skill body on demand (catalog-only system prompt).
//!
//! Pattern matches claw-code's `Skill` tool: the system prompt lists skill
//! names/descriptions; full `SKILL.md` content is fetched only when needed.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use pigs_config::Skill;
use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Shared skill catalog visible to the `skill` tool.
pub type SkillCatalog = Arc<Mutex<Vec<Skill>>>;

/// Tool that loads one skill's full instructions by name.
pub struct SkillTool {
    catalog: SkillCatalog,
}

impl SkillTool {
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
}

impl ToolHandler for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "skill",
            "Load the full instructions for a named skill from the skill catalog. \
             The system prompt only lists skill names and short descriptions; call this \
             tool when you need the complete skill body before following it. \
             Use the exact skill name from the catalog.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name as listed in the Available Skills catalog"
                    },
                    "skill": {
                        "type": "string",
                        "description": "Alias of `name` (claw-code style)"
                    }
                },
                "required": []
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let name = input
                .get("name")
                .or_else(|| input.get("skill"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ToolError::InvalidInput(
                        "missing skill name: provide 'name' or 'skill'".into(),
                    )
                })?;

            let catalog = self.catalog.lock().map_err(|_| {
                ToolError::ExecutionFailed("skill catalog lock poisoned".into())
            })?;

            match pigs_config::find_skill(&catalog, name) {
                Some(skill) => {
                    let body = pigs_config::format_skill_body(skill);
                    Ok(ToolResult::success(body))
                }
                None => {
                    let available: Vec<&str> = catalog.iter().map(|s| s.name.as_str()).collect();
                    let list = if available.is_empty() {
                        "(no skills loaded)".to_string()
                    } else {
                        available.join(", ")
                    };
                    Ok(ToolResult::error(format!(
                        "Unknown skill '{name}'. Available: {list}"
                    )))
                }
            }
        })
    }
}
