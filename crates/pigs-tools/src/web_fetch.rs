//! Web fetch tool — HTTP GET to fetch web page content.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for fetching web page content via HTTP GET.
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        WebFetchTool
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "web_fetch",
            "Fetch the content of a web page via HTTP GET. \
             Returns the raw response body text. Useful for reading documentation, \
             checking API responses, or downloading public content.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (must include http:// or https://)"
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum response length in characters. Default: 10000.",
                        "default": 10000
                    }
                },
                "required": ["url"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let url = input
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'url' field".into()))?;

            let max_length = input
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(10_000) as usize;

            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error(
                    "URL must start with http:// or https://".to_string(),
                ));
            }

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("PigsAgent/0.1")
                .build()
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to create HTTP client: {e}")))?;

            let response = client
                .get(url)
                .send()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("HTTP request failed: {e}")))?;

            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            if !status.is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP {status} — content type: {content_type}"
                )));
            }

            let body = response
                .text()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read response body: {e}")))?;

            let truncated = if body.len() > max_length {
                format!("{}... (truncated, {} total chars)", &body[..max_length], body.len())
            } else {
                body
            };

            Ok(ToolResult::success(format!(
                "Status: {status}\nContent-Type: {content_type}\nLength: {} chars\n\n{truncated}",
                truncated.len()
            )))
        })
    }
}
