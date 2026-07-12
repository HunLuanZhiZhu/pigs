//! Write file tool — creates or overwrites files.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for writing file contents.
pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        WriteFileTool
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "write_file",
            "Write content to a file. Creates the file if it doesn't exist, \
             or overwrites it if it does. Creates parent directories if needed.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["path", "content"]
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

            let content = input
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'content' field".into()))?;

            let path = Path::new(path_str);

            // Create parent directories if needed
            if let Some(parent) = path.parent() {
                if !parent.exists() && !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| {
                            ToolError::ExecutionFailed(format!(
                                "Failed to create parent directories: {e}"
                            ))
                        })?;
                }
            }

            tokio::fs::write(path, content)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write file: {e}")))?;

            let lines = content.lines().count();
            Ok(ToolResult::success(format!(
                "Successfully wrote {lines} lines to {path_str}"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn test_write_file_success() {
        let mut temp_path = std::env::temp_dir();
        temp_path.push("pigs_test_write.txt");
        let _ = std::fs::remove_file(&temp_path);

        let tool = WriteFileTool::new();
        let result = tool
            .execute(serde_json::json!({
                "path": temp_path.to_string_lossy(),
                "content": "line 1\nline 2\nline 3"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("3 lines"));

        // Verify content was written
        let content = std::fs::read_to_string(&temp_path).unwrap();
        assert_eq!(content, "line 1\nline 2\nline 3");

        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_write_file_missing_content() {
        let tool = WriteFileTool::new();
        let result = tool
            .execute(serde_json::json!({"path": "/tmp/test.txt"}))
            .await;
        assert!(result.is_err());
    }
}
