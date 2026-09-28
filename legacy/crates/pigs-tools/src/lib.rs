//! Built-in tools for the Pigs Agent.
//!
//! Each tool implements the `ToolHandler` trait from `pigs-core` and provides
//! a specific capability (file reading, shell execution, web fetching, etc.).

pub mod apply_patch;
pub mod ask_user;
pub mod bash;
pub mod edit_file;
pub mod git_diff;
pub mod glob;
pub mod grep;
pub mod http_request;
pub mod ignore;
pub mod list_files;
pub mod read_file;
pub mod sleep;
pub mod todo_write;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;

use std::sync::{Arc, Mutex};

use pigs_core::ToolRegistry;
use pigs_permissions::PermissionMode;

use todo_write::{TodoList, TodoWriteTool};

fn register_common_tools(registry: &mut ToolRegistry) {
    // Read-only tools
    registry.register(Box::new(read_file::ReadFileTool::new()));
    registry.register(Box::new(grep::GrepTool::new()));
    registry.register(Box::new(glob::GlobTool::new()));
    registry.register(Box::new(list_files::ListFilesTool::new()));
    registry.register(Box::new(web_fetch::WebFetchTool::new()));
    registry.register(Box::new(web_search::WebSearchTool::new()));
    registry.register(Box::new(git_diff::GitDiffTool::new()));
    registry.register(Box::new(http_request::HttpRequestTool::new()));

    // Write tools
    registry.register(Box::new(write_file::WriteFileTool::new()));
    registry.register(Box::new(edit_file::EditFileTool::new()));
    registry.register(Box::new(apply_patch::ApplyPatchTool::new()));

    // Interactive / utility tools
    registry.register(Box::new(ask_user::AskUserTool::new()));
    registry.register(Box::new(sleep::SleepTool::new()));
}

/// Create a registry with all built-in tools registered.
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_common_tools(&mut registry);

    // Stateful tools (with shared todo list)
    let todos: TodoList = Arc::new(Mutex::new(Vec::new()));
    registry.register(Box::new(TodoWriteTool::new(todos)));

    // Dangerous tools
    registry.register(Box::new(bash::BashTool::new()));

    registry
}

/// Create a registry with a shared todo list (for CLI display).
pub fn create_default_registry_with_todos() -> (ToolRegistry, TodoList) {
    let mut registry = ToolRegistry::new();
    register_common_tools(&mut registry);

    // Stateful tools (with shared todo list)
    let todos: TodoList = Arc::new(Mutex::new(Vec::new()));
    registry.register(Box::new(TodoWriteTool::new(Arc::clone(&todos))));

    // Dangerous tools
    registry.register(Box::new(bash::BashTool::new()));

    (registry, todos)
}

/// Get the permission mode required for each built-in tool.
pub fn tool_permission_modes() -> Vec<(&'static str, PermissionMode)> {
    vec![
        ("read", PermissionMode::ReadOnly),
        ("grep", PermissionMode::ReadOnly),
        ("find", PermissionMode::ReadOnly),
        ("ls", PermissionMode::ReadOnly),
        ("web_fetch", PermissionMode::ReadOnly),
        ("web_search", PermissionMode::ReadOnly),
        ("git_diff", PermissionMode::ReadOnly),
        ("http_request", PermissionMode::ReadOnly),
        ("write", PermissionMode::WorkspaceWrite),
        ("edit", PermissionMode::WorkspaceWrite),
        ("patch", PermissionMode::WorkspaceWrite),
        ("ask_user", PermissionMode::ReadOnly),
        ("todo_write", PermissionMode::ReadOnly),
        ("sleep", PermissionMode::ReadOnly),
        ("agent", PermissionMode::ReadOnly),
        ("skill", PermissionMode::ReadOnly),
        ("bash", PermissionMode::DangerFullAccess),
    ]
}
