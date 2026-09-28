//! MCP client errors.

/// Error type for MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Failed to spawn MCP server process: {0}")]
    Spawn(String),
    #[error("MCP server I/O error: {0}")]
    Io(String),
    #[error("JSON-RPC error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("Invalid MCP response: {0}")]
    InvalidResponse(String),
    #[error("MCP server not connected")]
    NotConnected,
    #[error("Timeout waiting for MCP response")]
    Timeout,
    #[error("Server '{0}' already connected")]
    AlreadyConnected(String),
    #[error("Server '{0}' not found")]
    ServerNotFound(String),
}
