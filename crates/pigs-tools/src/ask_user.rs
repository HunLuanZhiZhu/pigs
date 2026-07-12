//! AskUser tool — ask the user a structured question with options.

use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};

/// Tool for asking the user a question.
pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        AskUserTool
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolHandler for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "ask_user",
            "Ask the user a question when you need clarification or a decision. \
             The user can select from provided options or type a custom answer.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user"
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Suggested options the user can choose from"
                    }
                },
                "required": ["question"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let question = input
                .get("question")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing 'question' field".into()))?;

            let options: Vec<String> = input
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Display the question
            println!("\n┌─ Question ──────────────────────────────");
            println!("│ {question}");
            println!("└──────────────────────────────────────────");

            if !options.is_empty() {
                println!("Options:");
                for (i, opt) in options.iter().enumerate() {
                    println!("  {}. {opt}", i + 1);
                }
                println!("  (or type your own answer)");
            }

            print!("Your answer: ");
            let _ = io::stdout().flush();

            let mut answer = String::new();
            io::stdin()
                .read_line(&mut answer)
                .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read input: {e}")))?;

            let answer = answer.trim();

            // If the user entered a number, map it to the option
            let final_answer = if !options.is_empty() {
                if let Ok(num) = answer.parse::<usize>() {
                    if num >= 1 && num <= options.len() {
                        return Ok(ToolResult::success(options[num - 1].clone()));
                    }
                }
                answer.to_string()
            } else {
                answer.to_string()
            };

            if final_answer.is_empty() {
                Ok(ToolResult::success("(no answer provided)".to_string()))
            } else {
                Ok(ToolResult::success(final_answer))
            }
        })
    }
}
