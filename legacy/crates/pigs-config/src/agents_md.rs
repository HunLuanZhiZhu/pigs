//! Project memory file loading — injected into the system prompt.
//!
//! Supported filenames in the workspace root (priority order):
//! 1. `CLAUDE.md` (preferred when present)
//! 2. `AGENTS.md` (fallback)

use std::path::{Path, PathBuf};

/// Preferred project-context filenames. First existing non-empty file wins.
pub const PROJECT_CONTEXT_FILENAMES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// A loaded project context file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    /// Basename that was loaded (`CLAUDE.md` or `AGENTS.md`).
    pub source: String,
    /// File contents.
    pub content: String,
    /// Absolute or workspace-relative path that was read.
    pub path: PathBuf,
}

/// Load project memory from `dir`, preferring `CLAUDE.md` over `AGENTS.md`.
///
/// Returns `None` if neither file exists or both are empty/unreadable.
pub fn load_agents_md(dir: &Path) -> Option<String> {
    load_project_context(dir).map(|ctx| ctx.content)
}

/// Load project context with source metadata (`CLAUDE.md` preferred).
pub fn load_project_context(dir: &Path) -> Option<ProjectContext> {
    for name in PROJECT_CONTEXT_FILENAMES {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        return Some(ProjectContext {
            source: (*name).to_string(),
            content,
            path,
        });
    }
    None
}

/// Build the full system prompt, optionally including project context content.
///
/// `source_label` is the filename shown in the section header (defaults to `AGENTS.md`
/// when only the content string is available via the older API).
pub fn build_system_prompt(base_prompt: &str, agents_md: Option<&str>) -> String {
    build_system_prompt_with_source(base_prompt, agents_md, "AGENTS.md")
}

/// Like [`build_system_prompt`], but labels the injected section with `source_label`.
pub fn build_system_prompt_with_source(
    base_prompt: &str,
    project_md: Option<&str>,
    source_label: &str,
) -> String {
    match project_md {
        Some(md) if !md.trim().is_empty() => {
            let label = if source_label.trim().is_empty() {
                "AGENTS.md"
            } else {
                source_label
            };
            format!("{base_prompt}\n\n--- Project Context ({label}) ---\n\n{md}")
        }
        _ => base_prompt.to_string(),
    }
}

/// Convenience: load from `dir` and append to `base_prompt` with the correct label.
pub fn build_system_prompt_from_dir(base_prompt: &str, dir: &Path) -> String {
    match load_project_context(dir) {
        Some(ctx) => build_system_prompt_with_source(base_prompt, Some(&ctx.content), &ctx.source),
        None => base_prompt.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    #[test]
    fn test_build_system_prompt_with_agents_md() {
        let base = "You are a helpful agent.";
        let agents = "# Project\n\nThis is a test project.";
        let result = build_system_prompt(base, Some(agents));
        assert!(result.contains("You are a helpful agent."));
        assert!(result.contains("--- Project Context (AGENTS.md) ---"));
        assert!(result.contains("# Project"));
    }

    #[test]
    fn test_build_system_prompt_without_agents_md() {
        let base = "You are a helpful agent.";
        let result = build_system_prompt(base, None);
        assert_eq!(result, base);
    }

    #[test]
    fn test_build_system_prompt_empty_agents_md() {
        let base = "You are a helpful agent.";
        let result = build_system_prompt(base, Some("   \n  "));
        assert_eq!(result, base);
    }

    #[test]
    fn test_claude_md_preferred_over_agents_md() {
        let temp =
            std::env::temp_dir().join(format!("pigs_project_context_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let mut agents = std::fs::File::create(temp.join("AGENTS.md")).unwrap();
        writeln!(agents, "# Agents file\nfrom agents").unwrap();
        let mut claude = std::fs::File::create(temp.join("CLAUDE.md")).unwrap();
        writeln!(claude, "# Claude file\nfrom claude").unwrap();

        let ctx = load_project_context(&temp).expect("should load");
        assert_eq!(ctx.source, "CLAUDE.md");
        assert!(ctx.content.contains("from claude"));
        assert!(!ctx.content.contains("from agents"));

        let content = load_agents_md(&temp).unwrap();
        assert!(content.contains("from claude"));

        let prompt = build_system_prompt_from_dir("base", &temp);
        assert!(prompt.contains("--- Project Context (CLAUDE.md) ---"));
        assert!(prompt.contains("from claude"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_agents_md_used_when_no_claude_md() {
        let temp = std::env::temp_dir().join(format!(
            "pigs_project_context_agents_only_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let mut agents = std::fs::File::create(temp.join("AGENTS.md")).unwrap();
        writeln!(agents, "agents only").unwrap();

        let ctx = load_project_context(&temp).unwrap();
        assert_eq!(ctx.source, "AGENTS.md");
        assert!(ctx.content.contains("agents only"));

        let prompt = build_system_prompt_from_dir("base", &temp);
        assert!(prompt.contains("--- Project Context (AGENTS.md) ---"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_load_project_context_missing() {
        let temp = std::env::temp_dir().join(format!(
            "pigs_project_context_missing_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        assert!(load_project_context(&temp).is_none());
        assert!(load_agents_md(&temp).is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
