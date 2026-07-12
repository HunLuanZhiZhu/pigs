//! List files tool — list directory contents.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for listing directory contents.
pub struct ListFilesTool;

impl ListFilesTool {
    pub fn new() -> Self {
        ListFilesTool
    }
}

impl Default for ListFilesTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "list_files",
            "List the contents of a directory. Returns file and directory names with type indicators. \
             Non-recursive by default.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The directory to list. Defaults to current directory.",
                        "default": "."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "If true, list recursively. Default: false.",
                        "default": false
                    }
                }
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let recursive = input
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let dir = Path::new(path);

            if !dir.exists() {
                return Ok(ToolResult::error(format!("Directory not found: {path}")));
            }

            if !dir.is_dir() {
                return Ok(ToolResult::error(format!("Not a directory: {path}")));
            }

            let ignore_patterns = crate::ignore::IgnorePatterns::load(dir);
            let mut results = Vec::new();
            const MAX_ENTRIES: usize = 1000;

            if recursive {
                list_recursive(dir, dir, &ignore_patterns, &mut results, 0, MAX_ENTRIES);
            } else {
                list_flat(dir, &ignore_patterns, &mut results, MAX_ENTRIES);
            }

            if results.is_empty() {
                Ok(ToolResult::success("(empty directory)".to_string()))
            } else {
                let output = results.join("\n");
                Ok(ToolResult::success(output))
            }
        })
    }
}

fn list_flat(
    dir: &Path,
    ignore_patterns: &crate::ignore::IgnorePatterns,
    results: &mut Vec<String>,
    max: usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut items: Vec<(String, bool)> = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let is_dir = path.is_dir();

        // Skip .pigsignore-matched entries
        if ignore_patterns.is_ignored(&path, dir) {
            continue;
        }

        items.push((name, is_dir));
    }

    // Sort: directories first, then alphabetically
    items.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });

    for (name, is_dir) in items.into_iter().take(max) {
        if is_dir {
            results.push(format!("{name}/"));
        } else {
            results.push(name);
        }
    }
}

fn list_recursive(
    base: &Path,
    dir: &Path,
    ignore_patterns: &crate::ignore::IgnorePatterns,
    results: &mut Vec<String>,
    depth: usize,
    max: usize,
) {
    if results.len() >= max || depth > 10 {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut items: Vec<(String, std::path::PathBuf, bool)> = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let is_dir = path.is_dir();

        // Skip default-ignored directories
        if crate::ignore::IgnorePatterns::is_default_ignored(&name) {
            continue;
        }

        // Skip .pigsignore-matched entries
        if ignore_patterns.is_ignored(&path, base) {
            continue;
        }

        items.push((name, path, is_dir));
    }

    items.sort_by(|a, b| match (a.2, b.2) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });

    let indent = "  ".repeat(depth);

    for (name, path, is_dir) in items {
        if results.len() >= max {
            return;
        }

        let display = if is_dir {
            format!("{indent}{name}/")
        } else {
            format!("{indent}{name}")
        };
        results.push(display);

        if is_dir {
            list_recursive(base, &path, ignore_patterns, results, depth + 1, max);
        }
    }
}
