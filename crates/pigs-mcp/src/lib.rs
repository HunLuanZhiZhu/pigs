//! Minimal MCP (Model Context Protocol) client over stdio.
//!
//! Supports a practical subset of MCP:
//! - initialize
//! - tools/list
//! - tools/call
//!
//! Transport: JSON-RPC 2.0 messages framed by Content-Length headers
//! (the common MCP stdio framing used by many servers).

pub mod client;
pub mod error;
pub mod protocol;
pub mod tool_bridge;

pub use client::{McpClient, McpServerConfig, McpToolInfo};
pub use error::McpError;
pub use tool_bridge::McpToolHandler;
