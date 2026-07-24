//! Edit file tool — exact string replacement in files.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for editing files via exact string replacement.
pub struct EditFileTool;

impl EditFileTool {
    pub fn new() -> Self {
        EditFileTool
    }
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for EditFileTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "edit",
            "Edit a file by replacing an exact string match with new content. \
             The old_string must appear exactly once in the file unless replace_all is true.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact string to find in the file"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The string to replace it with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "If true, replace all occurrences. Default: false.",
                        "default": false
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let path_str = input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'path' field".into()))?;

            let old_string = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'old_string' field".into()))?;

            let new_string = input
                .get("new_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'new_string' field".into()))?;

            let replace_all = input
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let path = Path::new(path_str);

            if !path.exists() {
                return Ok(ToolResult::error(format!("File not found: {path_str}")));
            }

            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {e}")))?;

            // Count occurrences
            let count = content.matches(old_string).count();

            if count == 0 {
                return Ok(ToolResult::error("old_string not found in file. Make sure the string matches exactly, including whitespace.".to_string()));
            }

            if count > 1 && !replace_all {
                return Ok(ToolResult::error(format!(
                    "old_string found {count} times in file. Set replace_all=true to replace all occurrences, or make old_string more specific."
                )));
            }

            let new_content = if replace_all {
                content.replace(old_string, new_string)
            } else {
                content.replacen(old_string, new_string, 1)
            };

            tokio::fs::write(path, &new_content)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write file: {e}")))?;

            let replaced_count = if replace_all { count } else { 1 };
            Ok(ToolResult::success(format!(
                "Successfully replaced {replaced_count} occurrence(s) in {path_str}"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn test_edit_file_single_replace() {
        let mut temp_path = std::env::temp_dir();
        temp_path.push("pigs_test_edit.txt");
        std::fs::write(&temp_path, "hello world\nfoo bar").unwrap();

        let tool = EditFileTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": temp_path.to_string_lossy(),
                "old_string": "hello world",
                "new_string": "goodbye world"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        let content = std::fs::read_to_string(&temp_path).unwrap();
        assert_eq!(content, "goodbye world\nfoo bar");

        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_edit_file_not_found_string() {
        let mut temp_path = std::env::temp_dir();
        temp_path.push("pigs_test_edit2.txt");
        std::fs::write(&temp_path, "hello world").unwrap();

        let tool = EditFileTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": temp_path.to_string_lossy(),
                "old_string": "nonexistent string",
                "new_string": "replacement"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("not found"));

        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_edit_file_file_not_found() {
        let tool = EditFileTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": "/nonexistent/file.txt",
                "old_string": "a",
                "new_string": "b"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("File not found"));
    }
}
