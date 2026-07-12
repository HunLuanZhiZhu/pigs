//! Cross-session memory notes.
//!
//! Memory is stored as markdown bullet notes in:
//! - global: `~/.pigs/memory.md`
//! - project: `{workspace}/.pigs/memory.md`
//!
//! Notes are injected into the system prompt so the agent can recall preferences
//! and durable facts across sessions.

use std::path::{Path, PathBuf};

use chrono::Utc;

/// A single memory note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryNote {
    pub text: String,
    pub source: MemorySource,
}

/// Where a memory note lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    Global,
    Project,
}

impl MemorySource {
    pub fn as_str(self) -> &'static str {
        match self {
            MemorySource::Global => "global",
            MemorySource::Project => "project",
        }
    }
}

/// Loaded memory set.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    pub global: Vec<String>,
    pub project: Vec<String>,
}

impl MemoryStore {
    pub fn all_notes(&self) -> Vec<MemoryNote> {
        let mut notes = Vec::new();
        for text in &self.global {
            notes.push(MemoryNote {
                text: text.clone(),
                source: MemorySource::Global,
            });
        }
        for text in &self.project {
            notes.push(MemoryNote {
                text: text.clone(),
                source: MemorySource::Project,
            });
        }
        notes
    }

    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.project.is_empty()
    }

    pub fn len(&self) -> usize {
        self.global.len() + self.project.len()
    }
}

/// Global memory file path: `~/.pigs/memory.md`.
pub fn global_memory_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".pigs").join("memory.md")
}

/// Project memory file path: `{workspace}/.pigs/memory.md`.
pub fn project_memory_path(workspace: &Path) -> PathBuf {
    workspace.join(".pigs").join("memory.md")
}

/// Load global + project memory notes.
pub fn load_memory(workspace: &Path) -> MemoryStore {
    MemoryStore {
        global: read_notes(&global_memory_path()),
        project: read_notes(&project_memory_path(workspace)),
    }
}

/// Append a note to global or project memory.
pub fn add_memory_note(
    workspace: &Path,
    source: MemorySource,
    text: &str,
) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("memory note cannot be empty".into());
    }
    let path = match source {
        MemorySource::Global => global_memory_path(),
        MemorySource::Project => project_memory_path(workspace),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut notes = read_notes(&path);
    // de-dupe exact notes
    if notes.iter().any(|n| n == text) {
        return Ok(path);
    }
    let stamp = Utc::now().format("%Y-%m-%d");
    notes.push(format!("[{stamp}] {text}"));
    write_notes(&path, &notes)?;
    Ok(path)
}

/// Remove notes containing the given substring (case-sensitive).
/// Returns number of removed notes and file path.
pub fn remove_memory_notes(
    workspace: &Path,
    source: MemorySource,
    needle: &str,
) -> Result<(usize, PathBuf), String> {
    let path = match source {
        MemorySource::Global => global_memory_path(),
        MemorySource::Project => project_memory_path(workspace),
    };
    let notes = read_notes(&path);
    let before = notes.len();
    let kept: Vec<String> = notes
        .into_iter()
        .filter(|n| !n.contains(needle))
        .collect();
    let removed = before - kept.len();
    write_notes(&path, &kept)?;
    Ok((removed, path))
}

/// Format memory for system prompt injection.
pub fn format_memory_for_prompt(store: &MemoryStore) -> String {
    if store.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n--- Memory Notes ---\n\n");
    out.push_str(
        "Durable notes from previous sessions. Use them as preferences/context, not hard constraints over explicit user instructions.\n\n",
    );
    if !store.global.is_empty() {
        out.push_str("### Global\n");
        for n in &store.global {
            out.push_str(&format!("- {n}\n"));
        }
        out.push('\n');
    }
    if !store.project.is_empty() {
        out.push_str("### Project\n");
        for n in &store.project {
            out.push_str(&format!("- {n}\n"));
        }
        out.push('\n');
    }
    out.push_str("--- End Memory Notes ---\n");
    out
}

fn read_notes(path: &Path) -> Vec<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.strip_prefix("- ").unwrap_or(l).to_string())
        .collect()
}

fn write_notes(path: &Path, notes: &[String]) -> Result<(), String> {
    let mut body = String::from("# Pigs Memory\n\n");
    body.push_str("Notes below are loaded into future sessions.\n\n");
    for n in notes {
        body.push_str(&format!("- {n}\n"));
    }
    std::fs::write(path, body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_add_and_format_memory() {
        let temp = std::env::temp_dir().join(format!(
            "pigs_memory_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join(".pigs")).unwrap();

        // Use temp workspace for project memory.
        let path = add_memory_note(&temp, MemorySource::Project, "Prefer Conventional Commits")
            .unwrap();
        assert!(path.exists());

        let store = load_memory(&temp);
        assert!(
            store.project.iter().any(|n| n.contains("Prefer Conventional Commits")),
            "project notes: {:?}",
            store.project
        );
        let prompt = format_memory_for_prompt(&store);
        assert!(prompt.contains("Prefer Conventional Commits"));

        let (removed, _) =
            remove_memory_notes(&temp, MemorySource::Project, "Conventional").unwrap();
        assert!(removed >= 1);
        let store = load_memory(&temp);
        assert!(
            store
                .project
                .iter()
                .all(|n| !n.contains("Prefer Conventional Commits"))
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
}
