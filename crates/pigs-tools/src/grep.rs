//! Grep tool — search file contents with regex patterns.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for searching file contents with regex.
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        GrepTool
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for GrepTool {
    fn name(&self) -> &str {
        "grep_search"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "grep_search",
            "Search file contents using a regex pattern. Returns matching lines with file paths and line numbers. \
             Searches recursively from the given directory (defaults to current directory).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory or file to search in. Defaults to current directory.",
                        "default": "."
                    },
                    "include": {
                        "type": "string",
                        "description": "File glob pattern to include (e.g. '*.rs'). If omitted, searches all files."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "If true, perform case-insensitive search. Default: false.",
                        "default": false
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of matching lines to return. Default: 100.",
                        "default": 100
                    }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'pattern' field".into()))?;

            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");

            let include = input.get("include").and_then(|v| v.as_str());
            let case_insensitive = input
                .get("case_insensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let max_results = input
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;

            // Build regex
            let mut regex_builder = regex::RegexBuilder::new(pattern);
            regex_builder.case_insensitive(case_insensitive);
            let regex = regex_builder
                .build()
                .map_err(|e| ToolError::InvalidInput(format!("Invalid regex pattern: {e}")))?;

            // Build glob pattern for file filtering
            let glob_pattern = include.and_then(|p| {
                if p.is_empty() {
                    None
                } else {
                    glob::Pattern::new(p).ok()
                }
            });

            let search_path = Path::new(path);
            let ignore_patterns = crate::ignore::IgnorePatterns::load(search_path);
            let mut results = Vec::new();
            let mut count = 0;

            search_directory(SearchArgs {
                base: search_path,
                dir: search_path,
                regex: &regex,
                glob_pattern: glob_pattern.as_ref(),
                ignore_patterns: &ignore_patterns,
                results: &mut results,
                count: &mut count,
                max_results,
            })
            .await;
            if results.is_empty() {
                Ok(ToolResult::success("No matches found.".to_string()))
            } else {
                let output = results.join("\n");
                Ok(ToolResult::success(output))
            }
        })
    }
}

/// Arguments for recursive directory search.
struct SearchArgs<'a> {
    base: &'a Path,
    dir: &'a Path,
    regex: &'a regex::Regex,
    glob_pattern: Option<&'a glob::Pattern>,
    ignore_patterns: &'a crate::ignore::IgnorePatterns,
    results: &'a mut Vec<String>,
    count: &'a mut usize,
    max_results: usize,
}

/// Recursively search a directory for matching lines.
/// Uses an explicit stack to avoid async recursion issues.
async fn search_directory(args: SearchArgs<'_>) {
    let SearchArgs {
        base,
        dir,
        regex,
        glob_pattern,
        ignore_patterns,
        results,
        count,
        max_results,
    } = args;

    let mut stack: Vec<std::path::PathBuf> = vec![dir.to_path_buf()];

    while let Some(current_dir) = stack.pop() {
        if *count >= max_results {
            return;
        }

        let entries = match std::fs::read_dir(&current_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            if path.is_dir() {
                // Skip default-ignored directories
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if crate::ignore::IgnorePatterns::is_default_ignored(name) {
                        continue;
                    }
                }
                // Skip .pigsignore-matched directories
                if ignore_patterns.is_ignored(&path, base) {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                // Skip .pigsignore-matched files
                if ignore_patterns.is_ignored(&path, base) {
                    continue;
                }

                // Check glob filter
                if let Some(gp) = glob_pattern {
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !gp.matches(filename) {
                        continue;
                    }
                }

                // Skip binary files and large files
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(
                        ext,
                        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "pdf" | "zip" | "tar"
                            | "gz" | "exe" | "dll" | "so" | "dylib" | "bin" | "obj" | "o" | "a"
                    ) {
                        continue;
                    }
                }

                // Check file size (skip files > 1MB)
                if let Ok(metadata) = std::fs::metadata(&path) {
                    if metadata.len() > 1_000_000 {
                        continue;
                    }
                }

                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue, // Skip files that can't be read as UTF-8
                };

                for (line_num, line) in content.lines().enumerate() {
                    if *count >= max_results {
                        return;
                    }

                    if regex.is_match(line) {
                        let rel_path = path.to_string_lossy();
                        results.push(format!("{rel_path}:{}:    {line}", line_num + 1));
                        *count += 1;
                    }
                }
            }
        }
    }
}
