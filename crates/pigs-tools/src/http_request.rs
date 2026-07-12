//! Generic HTTP request tool for APIs and debugging endpoints.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};
use serde_json::Value;

/// Tool for making arbitrary HTTP requests.
pub struct HttpRequestTool {
    client: reqwest::Client,
}

impl HttpRequestTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("pigs-agent/0.1 (+https://github.com/local/pigs)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for HttpRequestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "http_request",
            "Send an HTTP request and return status/headers/body. \
             Useful for calling APIs, checking endpoints, and inspecting responses. \
             Supports GET/POST/PUT/PATCH/DELETE/HEAD.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute URL, e.g. https://httpbin.org/get"
                    },
                    "method": {
                        "type": "string",
                        "description": "HTTP method. Default: GET",
                        "default": "GET"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional request headers as string-to-string map"
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional raw request body"
                    },
                    "json": {
                        "description": "Optional JSON body (takes precedence over body when both provided)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout seconds (1-120). Default: 30",
                        "default": 30
                    },
                    "max_body_chars": {
                        "type": "integer",
                        "description": "Max response body characters to return. Default: 20000",
                        "default": 20000
                    }
                },
                "required": ["url"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let url = input
                .get("url")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::InvalidInput("missing non-empty 'url'".into()))?;

            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(ToolError::InvalidInput(
                    "url must start with http:// or https://".into(),
                ));
            }

            let method = input
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .trim()
                .to_uppercase();
            let timeout_secs = input
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .clamp(1, 120);
            let max_body_chars = input
                .get("max_body_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(20_000)
                .clamp(100, 200_000) as usize;

            let method_enum = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| {
                ToolError::InvalidInput(format!("unsupported HTTP method: {method}"))
            })?;

            let mut req = self
                .client
                .request(method_enum, url)
                .timeout(Duration::from_secs(timeout_secs));

            if let Some(headers_val) = input.get("headers") {
                let headers: HashMap<String, String> = serde_json::from_value(headers_val.clone())
                    .map_err(|e| {
                        ToolError::InvalidInput(format!(
                            "headers must be an object of string values: {e}"
                        ))
                    })?;
                for (k, v) in headers {
                    req = req.header(k, v);
                }
            }

            if let Some(json_body) = input.get("json") {
                req = req.json(json_body);
            } else if let Some(body) = input.get("body").and_then(|v| v.as_str()) {
                req = req.body(body.to_string());
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("http_request failed: {e}")))?;

            let status = resp.status();
            let version = format!("{:?}", resp.version());
            let headers = resp.headers().clone();
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("failed reading body: {e}")))?;

            let body_text = String::from_utf8_lossy(&bytes).to_string();
            let truncated = if body_text.chars().count() > max_body_chars {
                let prefix: String = body_text.chars().take(max_body_chars).collect();
                format!(
                    "{prefix}\n...[truncated {} of {} chars]",
                    max_body_chars,
                    body_text.chars().count()
                )
            } else {
                body_text
            };

            let mut out = String::new();
            out.push_str(&format!("HTTP {} {}\n", status.as_u16(), status.canonical_reason().unwrap_or("")));
            out.push_str(&format!("URL: {url}\n"));
            out.push_str(&format!("Method: {method}\n"));
            out.push_str(&format!("Version: {version}\n\n"));
            out.push_str("Headers:\n");
            for (k, v) in headers.iter() {
                out.push_str(&format!("  {}: {}\n", k, v.to_str().unwrap_or("<binary>")));
            }
            out.push_str("\nBody:\n");
            out.push_str(&truncated);

            if status.is_success() {
                Ok(ToolResult::success(out))
            } else {
                Ok(ToolResult::error(out))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_tool_name_and_spec() {
        let tool = HttpRequestTool::new();
        assert_eq!(tool.name(), "http_request");
        let spec = tool.spec();
        assert_eq!(spec.name, "http_request");
        assert!(spec.input_schema.pointer("/properties/url").is_some());
    }

    #[tokio::test]
    async fn test_invalid_url_scheme() {
        let tool = HttpRequestTool::new();
        let err = tool
            .execute(serde_json::json!({"url": "ftp://example.com"}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("http://") || msg.contains("https://"));
    }

    #[tokio::test]
    async fn test_missing_url() {
        let tool = HttpRequestTool::new();
        let err = tool.execute(serde_json::json!({})).await.unwrap_err();
        assert!(err.to_string().contains("url"));
    }
}
