//! Read file tool — reads file contents with optional line ranges.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for reading file contents.
pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        ReadFileTool
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "read_file",
            "Read the contents of a file. Returns the file content with line numbers. \
             Supports reading a range of lines by specifying offset and limit.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Starting line number (1-based). If omitted, starts from line 1."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read. If omitted, reads all lines."
                    }
                },
                "required": ["path"]
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

            let offset: usize = input
                .get("offset")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(1);

            let limit: Option<usize> = input
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            let path = Path::new(path_str);

            if !path.exists() {
                return Ok(ToolResult::error(format!("File not found: {path_str}")));
            }

            if !path.is_file() {
                return Ok(ToolResult::error(format!("Not a file: {path_str}")));
            }

            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {e}")))?;

            let lines: Vec<&str> = content.lines().collect();
            let start = if offset > 0 { offset - 1 } else { 0 };
            let end = match limit {
                Some(l) => (start + l).min(lines.len()),
                None => lines.len(),
            };

            if start >= lines.len() {
                return Ok(ToolResult::success(
                    "(empty - offset beyond file end)".to_string(),
                ));
            }

            let mut result = String::new();
            for (i, line) in lines[start..end].iter().enumerate() {
                let line_num = start + i + 1;
                result.push_str(&format!("{line_num:>6}\t{line}\n"));
            }

            // Truncate very large outputs
            const MAX_OUTPUT: usize = 100_000;
            if result.len() > MAX_OUTPUT {
                result.truncate(MAX_OUTPUT);
                result.push_str("\n... (truncated)\n");
            }

            Ok(ToolResult::success(result))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_read_file_success() {
        // Create a temp file
        let mut temp_path = std::env::temp_dir();
        temp_path.push("pigs_test_read.txt");
        let mut file = std::fs::File::create(&temp_path).unwrap();
        writeln!(file, "Hello world").unwrap();
        writeln!(file, "Second line").unwrap();

        let tool = ReadFileTool::new();
        let result = tool
            .execute(serde_json::json!({"path": temp_path.to_string_lossy()}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("Hello world"));
        assert!(result.output.contains("Second line"));

        let _ = std::fs::remove_file(&temp_path);
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let tool = ReadFileTool::new();
        let result = tool
            .execute(serde_json::json!({"path": "/nonexistent/file.txt"}))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("File not found"));
    }

    #[tokio::test]
    async fn test_read_file_missing_path() {
        let tool = ReadFileTool::new();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
