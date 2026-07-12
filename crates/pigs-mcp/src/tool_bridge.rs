//! Bridge MCP tools into pigs-core ToolHandler.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};
use serde_json::Value;

use crate::client::{McpClient, McpToolInfo};

/// A ToolHandler that proxies execution to an MCP server tool.
pub struct McpToolHandler {
    client: Arc<McpClient>,
    info: McpToolInfo,
    /// Prefixed tool name exposed to the LLM, e.g. "mcp_myserver_search"
    exposed_name: String,
}

impl McpToolHandler {
    /// Create a new MCP tool handler.
    ///
    /// The exposed name is `mcp_{server}_{tool}` to avoid collisions.
    pub fn new(client: Arc<McpClient>, info: McpToolInfo) -> Self {
        let exposed_name = format!(
            "mcp_{}_{}",
            sanitize_name(&info.server_name),
            sanitize_name(&info.name)
        );
        Self {
            client,
            info,
            exposed_name,
        }
    }

    /// Get the exposed tool name.
    pub fn exposed_name(&self) -> &str {
        &self.exposed_name
    }
}

impl ToolHandler for McpToolHandler {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn spec(&self) -> ToolSpec {
        let description = if self.info.description.is_empty() {
            format!(
                "MCP tool '{}' from server '{}'",
                self.info.name, self.info.server_name
            )
        } else {
            format!(
                "[MCP:{}] {}",
                self.info.server_name, self.info.description
            )
        };

        ToolSpec::new(
            &self.exposed_name,
            description,
            self.info.input_schema.clone(),
        )
    }

    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let result = self
                .client
                .call_tool(&self.info.server_name, &self.info.name, input)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let text = result.text_content();
            if result.is_error.unwrap_or(false) {
                Ok(ToolResult::error(text))
            } else if text.is_empty() {
                Ok(ToolResult::success("(empty MCP tool result)".to_string()))
            } else {
                Ok(ToolResult::success(text))
            }
        })
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("my-server"), "my-server");
        assert_eq!(sanitize_name("my server!"), "my_server_");
    }
}
