//! Tool system — tool specifications, the ToolHandler trait, and a registry.
//!
//! Tools are the primary way the agent interacts with the world. Each tool
//! implements the `ToolHandler` trait and is registered in a `ToolRegistry`.
//! The registry provides tool definitions (for the LLM) and dispatches
//! execution.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Error type for tool operations.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Timeout after {0}s")]
    Timeout(u64),
}

/// The result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful tool result.
    pub fn success(output: impl Into<String>) -> Self {
        ToolResult { output: output.into(), is_error: false }
    }

    /// Create an error tool result.
    pub fn error(output: impl Into<String>) -> Self {
        ToolResult { output: output.into(), is_error: true }
    }
}

/// The specification of a tool, sent to the LLM so it knows what tools are available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolSpec {
    /// Create a new tool spec.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        ToolSpec { name: name.into(), description: description.into(), input_schema }
    }
}

/// Type alias for the boxed future returned by `ToolHandler::execute`.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>>;

/// Trait that every tool must implement. Object-safe for dynamic dispatch.
///
/// Implementations receive a JSON value as input (as specified by their `input_schema`)
/// and return a `ToolResult` containing the output text.
pub trait ToolHandler: Send + Sync {
    /// Get the tool's name.
    fn name(&self) -> &str;

    /// Get the tool's specification (name, description, input schema).
    fn spec(&self) -> ToolSpec;

    /// Execute the tool with the given JSON input.
    fn execute<'a>(&'a self, input: serde_json::Value) -> ToolFuture<'a>;
}

/// Registry of available tools. Manages tool dispatch by name.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        ToolRegistry { tools: HashMap::new() }
    }

    /// Register a tool handler.
    pub fn register(&mut self, handler: Box<dyn ToolHandler>) {
        let name = handler.name().to_string();
        self.tools.insert(name, handler);
    }

    /// Check if a tool is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get all tool specifications (for sending to the LLM).
    pub fn definitions(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|h| h.spec()).collect()
    }

    /// Get the names of all registered tools.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolResult, ToolError> {
        let handler = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        handler.execute(input).await
    }

    /// Get the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    struct EchoTool;

    impl ToolHandler for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn spec(&self) -> ToolSpec {
            ToolSpec::new(
                "echo",
                "Echoes back the input text",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    },
                    "required": ["text"]
                }),
            )
        }

        fn execute<'a>(&'a self, input: serde_json::Value) -> ToolFuture<'a> {
            Box::pin(async move {
                let text = input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidInput("missing 'text' field".into()))?;
                Ok(ToolResult::success(text.to_string()))
            })
        }
    }

    #[tokio::test]
    async fn test_registry_execute() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        assert!(registry.has("echo"));
        assert_eq!(registry.len(), 1);

        let result = registry
            .execute("echo", serde_json::json!({"text": "hello"}))
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn test_registry_definitions() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", serde_json::json!({})).await;
        assert!(result.is_err());
        match result {
            Err(ToolError::NotFound(name)) => assert_eq!(name, "nonexistent"),
            _ => panic!("Expected NotFound error"),
        }
    }
}
