//! 相位运行时的数据面工具。
//! Data-plane tools for the phased runtime.
//!
//! 复用 `pigs-tools` 的完整内置工具注册表，使相位运行时拥有与
//! `pigs-cli` 相同的工具面。额外添加了进程级 `internal_notes` 暂存工具。
//! Reuses the full built-in tool registry from `pigs-tools` so that the
//! phased runtime has the same tool surface as `pigs-cli`. An extra
//! process-local `internal_notes` scratchpad is added on top.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use pigs_core::{ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

/// 构建一个包含**全部**内置工具（同 pigs-cli）+ internal_notes 的注册表。
/// Build a registry with **all** built-in tools (same as pigs-cli) + internal_notes.
pub fn info_tool_registry() -> ToolRegistry {
    let mut reg = pigs_tools::create_default_registry();
    reg.register(Box::new(InternalNotesTool));
    reg
}

/// 进程级暂存工具：记录或召回本轮内部的简短笔记。
/// Process-local scratchpad tool: record or recall short internal notes for this turn.
struct InternalNotesTool;

impl ToolHandler for InternalNotesTool {
    fn name(&self) -> &str {
        "internal_notes"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "internal_notes",
            "Record or recall short internal notes for this turn (scratchpad).              Use to stash findings. Not a full codebase search.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["write", "read", "list"],
                        "description": "write a note, read by id, or list all"
                    },
                    "id": { "type": "string", "description": "note id for write/read" },
                    "content": { "type": "string", "description": "note body for write" }
                },
                "required": ["action"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // 静态全局笔记存储（进程级，非请求级）
            // Static global notes storage (process-level, not per-request)
            static NOTES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
            let action = input
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list");
            let mut guard = NOTES
                .lock()
                .map_err(|_| ToolError::ExecutionFailed("notes lock poisoned".into()))?;
            if guard.is_none() {
                *guard = Some(HashMap::new());
            }
            let map = guard
                .as_mut()
                .ok_or_else(|| ToolError::ExecutionFailed("notes not initialized".into()))?;

            match action {
                // 写入笔记 / Write a note
                "write" => {
                    let id = input
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string();
                    let content = input
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let len = content.len();
                    map.insert(id.clone(), content);
                    Ok(ToolResult::success(format!(
                        "wrote note `{id}` ({len} chars)"
                    )))
                }
                // 读取笔记 / Read a note by id
                "read" => {
                    let id = input
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    match map.get(id) {
                        Some(c) => Ok(ToolResult::success(c.clone())),
                        None => Ok(ToolResult::error(format!("no note `{id}`"))),
                    }
                }
                // 列出全部笔记 / List all notes
                _ => {
                    if map.is_empty() {
                        Ok(ToolResult::success("(no internal notes)"))
                    } else {
                        let mut lines: Vec<String> = map
                            .iter()
                            .map(|(k, v)| {
                                // 每条笔记预览限制 120 字符 / Preview each note to 120 chars
                                let preview = if v.chars().count() > 120 {
                                    let t: String = v.chars().take(117).collect();
                                    format!("{t}...")
                                } else {
                                    v.clone()
                                };
                                format!("- {k}: {preview}")
                            })
                            .collect();
                        lines.sort();
                        Ok(ToolResult::success(lines.join("\n")))
                    }
                }
            }
        })
    }
}
