//! Project rules loader — inject `.pig/rules/**/*.md` into the system prompt.

use std::path::{Path, PathBuf};

/// A project rule document.
#[derive(Debug, Clone)]
pub struct RuleDoc {
    pub name: String,
    pub body: String,
    pub path: PathBuf,
}

/// Load rules from `{workspace}/.pig/rules/` (markdown files, recursive one level).
pub fn load_rules(workspace: &Path) -> Vec<RuleDoc> {
    let dir = workspace.join(".pigs").join("rules");
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut rules = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("md"))
                        .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(body) = std::fs::read_to_string(&path) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("rule")
                    .to_string();
                if !body.trim().is_empty() {
                    rules.push(RuleDoc {
                        name,
                        body: body.trim().to_string(),
                        path,
                    });
                }
            }
        }
    }
    rules
}

/// Format rules for system prompt injection.
pub fn format_rules_for_prompt(rules: &[RuleDoc]) -> String {
    if rules.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n--- Project Rules ---\n\n");
    out.push_str(
        "Follow these project-specific rules unless the user explicitly overrides them.\n\n",
    );
    for rule in rules {
        out.push_str(&format!("### Rule: {}\n\n{}\n\n", rule.name, rule.body));
    }
    out.push_str("--- End Project Rules ---\n");
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_and_format_rules() {
        let temp = std::env::temp_dir().join("pigs_rules_test");
        let rules_dir = temp.join(".pigs").join("rules");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&rules_dir).unwrap();
        let mut f = std::fs::File::create(rules_dir.join("style.md")).unwrap();
        writeln!(f, "Prefer explicit error handling.").unwrap();

        let rules = load_rules(&temp);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "style");
        let prompt = format_rules_for_prompt(&rules);
        assert!(prompt.contains("Project Rules"));
        assert!(prompt.contains("explicit error handling"));
        let _ = std::fs::remove_dir_all(&temp);
    }
}
