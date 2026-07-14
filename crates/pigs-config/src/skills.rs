//! Skills loader — load skill definitions from directories and inject into the system prompt.
//!
//! A skill is a markdown file (SKILL.md) or any `*.md` file in a skills directory.
//! Supported locations (priority order; first match of a skill name wins):
//! - `~/.pigs/skills/`           — pigs user skills
//! - `~/.agents/skills/`         — common/user agent skills (shared ecosystem)
//! - `{workspace}/.pigs/skills/` — pigs project skills
//! - `{workspace}/.agents/skills/` — common project skills (e.g. ARIS junctions)
//! - `{workspace}/skills/`       — workspace root skills
//!
//! Optional YAML frontmatter:
//! ```markdown
//! ---
//! name: code-review
//! description: Review code for bugs and style issues
//! ---
//! When reviewing code, check for...
//! ```

use std::path::{Path, PathBuf};

/// A loaded skill definition.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
}

/// Ordered skill search roots for a workspace (and optional home).
///
/// Earlier entries take precedence when skill names collide.
pub fn skill_search_dirs(workspace: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".pigs").join("skills"));
        // Common agent skill root used by multiple tools/ecosystems.
        dirs.push(home.join(".agents").join("skills"));
    }
    dirs.push(workspace.join(".pigs").join("skills"));
    // Project-local common skills (ARIS / Codex-style `.agents/skills/<name>/SKILL.md`).
    dirs.push(workspace.join(".agents").join("skills"));
    dirs.push(workspace.join("skills"));
    dirs
}

fn env_flag_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Load skills from the standard locations.
///
/// Env overrides:
/// - `PIGS_DISABLE_SKILLS=1` — load no skills (useful for small local models / tests)
/// - `PIGS_DISABLE_COMMON_AGENT_SKILLS=1` — skip `~/.agents/skills` and `.agents/skills`
pub fn load_skills(workspace: &Path) -> Vec<Skill> {
    if env_flag_truthy("PIGS_DISABLE_SKILLS") {
        return Vec::new();
    }

    let mut skills = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let skip_common = env_flag_truthy("PIGS_DISABLE_COMMON_AGENT_SKILLS");

    for dir in skill_search_dirs(workspace) {
        if skip_common && is_agents_skills_dir(&dir) {
            continue;
        }
        if !dir.is_dir() {
            continue;
        }
        load_skills_from_dir(&dir, &mut skills, &mut seen);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// True when `dir` ends with `.agents/skills` (platform-independent path components).
fn is_agents_skills_dir(dir: &Path) -> bool {
    let mut comps = dir.components().rev();
    match (comps.next(), comps.next()) {
        (
            Some(std::path::Component::Normal(skills)),
            Some(std::path::Component::Normal(agents)),
        ) => skills == "skills" && agents == ".agents",
        _ => false,
    }
}

fn load_skills_from_dir(
    dir: &Path,
    skills: &mut Vec<Skill>,
    seen: &mut std::collections::HashSet<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Nested skill dir: skills/code-review/SKILL.md
            let skill_md = path.join("SKILL.md");
            if skill_md.is_file() {
                if let Some(skill) =
                    parse_skill_file(&skill_md, path.file_name().and_then(|n| n.to_str()))
                {
                    if seen.insert(skill.name.clone()) {
                        skills.push(skill);
                    }
                }
            }
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed");
        if let Some(skill) = parse_skill_file(&path, Some(stem)) {
            if seen.insert(skill.name.clone()) {
                skills.push(skill);
            }
        }
    }
}

fn parse_skill_file(path: &Path, default_name: Option<&str>) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (name, description, body) = parse_frontmatter(&content, default_name.unwrap_or("unnamed"));

    if body.trim().is_empty() && description.is_empty() {
        return None;
    }

    Some(Skill {
        name,
        description,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

/// Parse optional YAML-like frontmatter between --- fences.
fn parse_frontmatter(content: &str, default_name: &str) -> (String, String, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (default_name.to_string(), String::new(), content.to_string());
    }

    let rest = &trimmed[3..];
    let rest = rest.trim_start_matches(['\r', '\n']);
    if let Some(end) = rest.find("\n---") {
        let front = &rest[..end];
        let body = rest[end + 4..].trim_start_matches(['\r', '\n']);

        let mut name = default_name.to_string();
        let mut description = String::new();

        for line in front.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("name:") {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    name = v.to_string();
                }
            } else if let Some(v) = line.strip_prefix("description:") {
                description = v.trim().trim_matches('"').trim_matches('\'').to_string();
            }
        }

        (name, description, body.to_string())
    } else {
        (default_name.to_string(), String::new(), content.to_string())
    }
}

/// Format skills as a **catalog index** for the system prompt.
///
/// Only names and short descriptions are included. Full skill bodies are loaded
/// on demand via the `skill` tool (claw-code style), not dumped into every turn.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::from("\n\n--- Available Skills (catalog) ---\n\n");
    out.push_str(
        "Skills provide specialized guidance. The catalog below lists only names and short \
         descriptions to save context. When a skill is relevant, call the `skill` tool with \
         that skill name to load its full instructions before following them.\n\n",
    );

    for skill in skills {
        let desc = if skill.description.is_empty() {
            "(no description)"
        } else {
            skill.description.as_str()
        };
        out.push_str(&format!("- `{}`: {}\n", skill.name, desc));
    }

    out.push_str("\n--- End Skills Catalog ---\n");
    out
}

/// Look up a skill by name (case-sensitive exact match, then case-insensitive).
pub fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    let needle = name.trim();
    if needle.is_empty() {
        return None;
    }
    skills
        .iter()
        .find(|s| s.name == needle)
        .or_else(|| skills.iter().find(|s| s.name.eq_ignore_ascii_case(needle)))
}

/// Format the full body of a skill for tool output.
pub fn format_skill_body(skill: &Skill) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Skill: {}\n", skill.name));
    if !skill.description.is_empty() {
        out.push_str(&format!("Description: {}\n", skill.description));
    }
    out.push_str(&format!("Path: {}\n\n", skill.path.display()));
    if skill.body.is_empty() {
        out.push_str("(skill file has no body)\n");
    } else {
        out.push_str(&skill.body);
        if !skill.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_frontmatter() {
        let content =
            "---\nname: review\ndescription: Code review skill\n---\nDo a thorough review.\n";
        let (name, desc, body) = parse_frontmatter(content, "default");
        assert_eq!(name, "review");
        assert_eq!(desc, "Code review skill");
        assert!(body.contains("thorough review"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "Just a plain skill body.";
        let (name, desc, body) = parse_frontmatter(content, "plain");
        assert_eq!(name, "plain");
        assert!(desc.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_load_skills_from_workspace() {
        let temp = std::env::temp_dir().join("pigs_skills_test");
        let skills_dir = temp.join("skills");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&skills_dir).unwrap();

        let mut f = std::fs::File::create(skills_dir.join("demo.md")).unwrap();
        writeln!(
            f,
            "---\nname: demo\ndescription: Demo skill\n---\nAlways be helpful.\n"
        )
        .unwrap();

        let skills = load_skills(&temp);
        // May also load user-global skills from ~/.pigs or ~/.agents; assert workspace skill is present.
        let demo = skills
            .iter()
            .find(|s| s.name == "demo")
            .expect("workspace skills/demo.md should load");
        assert!(demo.body.contains("helpful"));
        assert!(demo.path.starts_with(&temp));

        let prompt = format_skills_for_prompt(&skills);
        assert!(prompt.contains("`demo`"));
        assert!(prompt.contains("Demo skill"));
        // Catalog must not embed the full skill body.
        assert!(!prompt.contains("Always be helpful"));
        assert!(prompt.contains("`skill` tool") || prompt.contains("skill` tool"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_find_skill_and_format_body() {
        let skill = Skill {
            name: "demo".into(),
            description: "Demo skill".into(),
            body: "Full instructions here.".into(),
            path: PathBuf::from("/tmp/demo.md"),
        };
        let skills = vec![skill];
        let found = find_skill(&skills, "DEMO").expect("case-insensitive find");
        let body = format_skill_body(found);
        assert!(body.contains("# Skill: demo"));
        assert!(body.contains("Full instructions here."));
        assert!(find_skill(&skills, "missing").is_none());
    }

    #[test]
    fn test_load_skills_from_agents_skills_dir() {
        let temp =
            std::env::temp_dir().join(format!("pigs_agents_skills_test_{}", std::process::id()));
        let agents_skill = temp.join(".agents").join("skills").join("aris-demo");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&agents_skill).unwrap();

        let mut f = std::fs::File::create(agents_skill.join("SKILL.md")).unwrap();
        writeln!(
            f,
            "---\nname: aris-demo\ndescription: From .agents/skills\n---\nUse ARIS workflow.\n"
        )
        .unwrap();

        let dirs = skill_search_dirs(&temp);
        assert!(dirs.iter().any(|d| is_agents_skills_dir(d)));

        let skills = load_skills(&temp);
        assert!(
            skills.iter().any(|s| s.name == "aris-demo"),
            "expected skill from .agents/skills, got: {:?}",
            skills.iter().map(|s| &s.name).collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_pigs_skills_outrank_agents_skills_on_name_collision() {
        let temp =
            std::env::temp_dir().join(format!("pigs_skills_priority_test_{}", std::process::id()));
        let pigs_dir = temp.join(".pigs").join("skills");
        let agents_dir = temp.join(".agents").join("skills").join("shared");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&pigs_dir).unwrap();
        std::fs::create_dir_all(&agents_dir).unwrap();

        let mut f = std::fs::File::create(pigs_dir.join("shared.md")).unwrap();
        writeln!(
            f,
            "---\nname: shared\ndescription: pigs wins\n---\nfrom pigs\n"
        )
        .unwrap();
        let mut f = std::fs::File::create(agents_dir.join("SKILL.md")).unwrap();
        writeln!(
            f,
            "---\nname: shared\ndescription: agents loses\n---\nfrom agents\n"
        )
        .unwrap();

        let skills = load_skills(&temp);
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        assert!(
            shared.body.contains("from pigs"),
            "pigs-local skill should win name collision, body={}",
            shared.body
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
}
