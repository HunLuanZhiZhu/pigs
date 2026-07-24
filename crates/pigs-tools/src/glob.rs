//! Glob tool — find files matching a glob pattern.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for finding files by glob pattern.
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        GlobTool
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for GlobTool {
    fn name(&self) -> &str {
        "find"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "find",
            "Find files matching a glob pattern. Returns file paths relative to the search directory. \
             Supports patterns like '**/*.rs' for recursive matching.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The glob pattern to match (e.g. '**/*.rs', 'src/**/*.ts', '*.json')"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory to search in. Defaults to current directory.",
                        "default": "."
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

            let base_path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");

            let base = Path::new(base_path);
            let ignore_patterns = crate::ignore::IgnorePatterns::load(base);

            // Build the full glob pattern
            let full_pattern = if pattern.starts_with('/') || pattern.starts_with('\\') {
                pattern.to_string()
            } else {
                format!("{base_path}/{pattern}")
            };

            // Use the glob crate to find matching paths
            let glob_pattern = glob::glob(&full_pattern)
                .map_err(|e| ToolError::InvalidInput(format!("Invalid glob pattern: {e}")))?;

            let mut results = Vec::new();
            let mut count = 0;
            const MAX_RESULTS: usize = 500;

            for entry in glob_pattern {
                if count >= MAX_RESULTS {
                    break;
                }
                match entry {
                    Ok(path) => {
                        // Skip directories
                        if !path.is_file() {
                            continue;
                        }

                        // Skip paths under default-ignored directories
                        let skip = path.components().any(|c| {
                            c.as_os_str()
                                .to_str()
                                .map(crate::ignore::IgnorePatterns::is_default_ignored)
                                .unwrap_or(false)
                        });
                        if skip {
                            continue;
                        }

                        // Skip .pigignore-matched files
                        if ignore_patterns.is_ignored(&path, base) {
                            continue;
                        }

                        results.push(path.to_string_lossy().to_string());
                        count += 1;
                    }
                    Err(_) => continue,
                }
            }

            if results.is_empty() {
                Ok(ToolResult::success(
                    "No files found matching the pattern.".to_string(),
                ))
            } else {
                let output = format!("Found {} file(s):\n{}", results.len(), results.join("\n"));
                Ok(ToolResult::success(output))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn test_glob_search_finds_files() {
        // Search for Cargo.toml in the pigs workspace
        let tool = GlobTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "Cargo.toml",
                "path": "."
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Cargo.toml"));
    }

    #[tokio::test]
    async fn test_glob_search_no_matches() {
        let tool = GlobTool::new();
        let result = tool
            .execute(serde_json::json!({
                "pattern": "nonexistent_file_pattern_xyz123.*",
                "path": "."
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("No files found"));
    }

    #[tokio::test]
    async fn test_glob_search_missing_pattern() {
        let tool = GlobTool::new();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
