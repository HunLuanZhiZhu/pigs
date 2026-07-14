//! TodoWrite tool — track tasks during agent execution.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Shared todo list state that can be accessed by the tool and displayed by the CLI.
pub type TodoList = Arc<Mutex<Vec<TodoItem>>>;

/// A single todo item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

/// Status of a todo item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

/// Priority of a todo item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl std::fmt::Display for TodoPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoPriority::High => write!(f, "high"),
            TodoPriority::Medium => write!(f, "medium"),
            TodoPriority::Low => write!(f, "low"),
        }
    }
}

/// Tool for managing a todo list.
pub struct TodoWriteTool {
    todos: TodoList,
}

impl TodoWriteTool {
    /// Create a new TodoWrite tool with a shared todo list.
    pub fn new(todos: TodoList) -> Self {
        TodoWriteTool { todos }
    }
}

impl ToolHandler for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "todo_write",
            "Update the todo list. Use this to track tasks during work. \
             Provide the complete list of todos (not just changes). \
             Each call replaces the entire list.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The complete list of todo items",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "The task description"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Current status of the task"
                                },
                                "priority": {
                                    "type": "string",
                                    "enum": ["high", "medium", "low"],
                                    "description": "Priority level"
                                }
                            },
                            "required": ["content", "status", "priority"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let todos_arr = input
                .get("todos")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ToolError::InvalidInput("missing 'todos' array".into()))?;

            let mut new_todos = Vec::new();

            for item in todos_arr {
                let content = item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidInput("todo item missing 'content'".into()))?
                    .to_string();

                let status_str = item
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pending");
                let status = match status_str {
                    "in_progress" => TodoStatus::InProgress,
                    "completed" => TodoStatus::Completed,
                    _ => TodoStatus::Pending,
                };

                let priority_str = item
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("medium");
                let priority = match priority_str {
                    "high" => TodoPriority::High,
                    "low" => TodoPriority::Low,
                    _ => TodoPriority::Medium,
                };

                new_todos.push(TodoItem {
                    content,
                    status,
                    priority,
                });
            }

            let summary = {
                let total = new_todos.len();
                let completed = new_todos
                    .iter()
                    .filter(|t| matches!(t.status, TodoStatus::Completed))
                    .count();
                let in_progress = new_todos
                    .iter()
                    .filter(|t| matches!(t.status, TodoStatus::InProgress))
                    .count();
                format!(
                    "Todo list updated: {total} items ({completed} completed, {in_progress} in progress)"
                )
            };

            let mut todos = self.todos.lock().map_err(|e| {
                ToolError::ExecutionFailed(format!("Failed to lock todo list: {e}"))
            })?;
            *todos = new_todos;

            Ok(ToolResult::success(summary))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[tokio::test]
    async fn test_todo_write_basic() {
        let todos: TodoList = Arc::new(Mutex::new(Vec::new()));
        let tool = TodoWriteTool::new(Arc::clone(&todos));

        let result = tool
            .execute(serde_json::json!({
                "todos": [
                    {"content": "Task 1", "status": "pending", "priority": "high"},
                    {"content": "Task 2", "status": "in_progress", "priority": "medium"},
                    {"content": "Task 3", "status": "completed", "priority": "low"}
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("3 items"));
        assert!(result.output.contains("1 completed"));
        assert!(result.output.contains("1 in progress"));

        let locked = todos.lock().unwrap();
        assert_eq!(locked.len(), 3);
    }

    #[tokio::test]
    async fn test_todo_write_missing_todos() {
        let todos: TodoList = Arc::new(Mutex::new(Vec::new()));
        let tool = TodoWriteTool::new(todos);

        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_todo_write_replaces_list() {
        let todos: TodoList = Arc::new(Mutex::new(Vec::new()));

        // First write
        let tool = TodoWriteTool::new(Arc::clone(&todos));
        tool.execute(serde_json::json!({
            "todos": [
                {"content": "A", "status": "pending", "priority": "high"},
                {"content": "B", "status": "pending", "priority": "high"},
                {"content": "C", "status": "pending", "priority": "high"}
            ]
        }))
        .await
        .unwrap();

        assert_eq!(todos.lock().unwrap().len(), 3);

        // Second write replaces
        tool.execute(serde_json::json!({
            "todos": [
                {"content": "X", "status": "completed", "priority": "low"}
            ]
        }))
        .await
        .unwrap();

        assert_eq!(todos.lock().unwrap().len(), 1);
    }
}
