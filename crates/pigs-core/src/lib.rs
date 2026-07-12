//! Pigs Agent — Core types and traits.
//!
//! This crate defines the fundamental abstractions shared across all other crates:
//! - Message model (`Message`, `ContentBlock`, `MessageRole`)
//! - Tool system (`ToolSpec`, `ToolHandler`, `ToolResult`, `ToolRegistry`)
//! - LLM API abstraction (`ApiClient`, `ApiRequest`, `ApiResponse`)
//! - Token usage tracking (`TokenUsage`)

pub mod api;
pub mod message;
pub mod tool;
pub mod usage;

pub use api::{ApiError, ApiRequest, ApiResponse, ApiClient, ApiFuture, StreamCallback, StreamEvent};
pub use message::{ContentBlock, Message, MessageRole};
pub use tool::{ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};
pub use usage::TokenUsage;
