//! edit_file 工具 —— 搜索替换编辑。
//!
//! 教学要点：
//! - 这是 Agent 修改已有文件的工具——CoreCoder 的**核心创新**
//! - 与行号编辑不同，用**唯一字符串匹配**来定位修改位置
//!
//! 借鉴对比：
//! - 对比 CoreCoder `edit.py`（92 行）: 唯一匹配 search-and-replace
//! - 对比 pigs-tools `edit_file.rs`（190 行）: 精确字符串替换 + replace_all 选项
//! - 本 crate 采用 CoreCoder 的唯一匹配设计
//!
//! 为什么不用行号编辑？
//! （来自 CoreCoder README 第 130 行的设计哲学）
//! > "行号是陷阱，模型数错一行就悄悄改错地方。
//! > 用唯一片段锚定，失败可恢复、成功可验证。"
//!
//! edit_file 的工作方式:
//! 1. LLM 提供 `old_string`（要替换的文本）和 `new_string`（替换后的文本）
//! 2. 在文件中搜索 `old_string`
//! 3. 如果恰好匹配 1 次 → 执行替换
//! 4. 如果匹配 0 次 → 返回错误（提示 LLM 重新锚定）
//! 5. 如果匹配多次 → 返回错误（要求 LLM 加更多上下文保证唯一性）
//!
//! 这个设计让 LLM 自己负责"改对地方"——
//! 如果匹配不唯一，LLM 需要在 old_string 中加入更多上下文。
//! 比行号定位更可靠，因为 LLM 经常数错行号。

use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use crate::error::{MiniAgentError, Result};
use crate::tool::Tool;

/// edit_file 工具 —— 通过搜索替换编辑文件。
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    /// 工具名: "edit_file"
    fn name(&self) -> &str {
        "edit_file"
    }

    /// 工具描述 —— 告诉 LLM 如何使用这个工具
    fn description(&self) -> &str {
        "通过搜索替换编辑文件。提供 old_string（要替换的文本）和 new_string（替换后的文本）。\
        old_string 必须在文件中唯一匹配——如果不唯一，请加入更多上下文使其唯一。\
        建议先使用 read_file 查看文件内容，再使用 edit_file 进行修改。\
        适用于小范围修改，大范围重写请使用 write_file。"
    }

    /// 参数 schema
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要编辑的文件路径"
                },
                "old_string": {
                    "type": "string",
                    "description": "要被替换的文本（必须在文件中唯一匹配）"
                },
                "new_string": {
                    "type": "string",
                    "description": "替换后的文本"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    /// 执行文件编辑。
    ///
    /// 流程:
    /// 1. 提取 path、old_string、new_string 参数
    /// 2. 读取文件内容
    /// 3. 统计 old_string 在文件中的出现次数
    /// 4. 根据出现次数决定行为：
    ///    - 0 次 → 返回错误（文件开头片段供 LLM 重新锚定）
    ///    - 1 次 → 执行替换
    ///    - 多次 → 返回错误（要求 LLM 加更多上下文）
    /// 5. 写入修改后的文件
    /// 6. 返回修改信息
    async fn execute(&self, input: Value) -> Result<String> {
        // 1. 提取路径参数（必需）
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'path' 参数".into()))?;

        // 2. 提取 old_string（要替换的文本，必需）
        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'old_string' 参数".into()))?;

        // 3. 提取 new_string（替换后的文本，必需）
        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'new_string' 参数".into()))?;

        // 4. 如果 old_string 和 new_string 相同，没有意义
        if old_string == new_string {
            return Err(MiniAgentError::ToolError(
                "old_string 和 new_string 相同，无需替换".into(),
            ));
        }

        // 5. 读取文件内容
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| MiniAgentError::ToolError(format!("读取文件失败 '{path}': {e}")))?;

        // 6. 统计 old_string 出现次数
        let match_count = content.matches(old_string).count();

        // 7. 根据匹配次数决定行为
        match match_count {
            0 => {
                // 没有匹配 —— 返回文件开头内容供 LLM 重新锚定
                // 借鉴 CoreCoder: "0 匹配返回文件头 500 字符让模型重新锚定"
                let preview: String = content.chars().take(500).collect();
                Err(MiniAgentError::ToolError(format!(
                    "在文件 '{path}' 中未找到要替换的文本。\n\n\
                    文件开头 500 字符供参考:\n{preview}"
                )))
            }
            1 => {
                // 恰好 1 次匹配 —— 执行替换
                let new_content = content.replacen(old_string, new_string, 1);

                // 写入修改后的文件
                fs::write(path, &new_content).await.map_err(|e| {
                    MiniAgentError::ToolError(format!("写入文件失败 '{path}': {e}"))
                })?;

                // 返回成功信息
                Ok(format!(
                    "文件已修改: {path}\n\
                    替换: {old_chars} 字符 → {new_chars} 字符",
                    old_chars = old_string.chars().count(),
                    new_chars = new_string.chars().count(),
                ))
            }
            _ => {
                // 多次匹配 —— 要求 LLM 加更多上下文
                Err(MiniAgentError::ToolError(format!(
                    "在文件 '{path}' 中找到 {match_count} 处匹配 'old_string'。\n\
                    请在 old_string 中加入更多上下文使其唯一匹配。"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 测试唯一匹配替换
    #[tokio::test]
    async fn test_unique_replace() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "println!(\"hello\");",
                "new_string": "println!(\"world\");"
            }))
            .await;

        assert!(result.is_ok());

        // 验证文件内容已修改
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("world"));
        assert!(!content.contains("hello"));
    }

    /// 测试无匹配时返回错误
    #[tokio::test]
    async fn test_no_match() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world\n").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "这段文本不存在",
                "new_string": "新文本"
            }))
            .await;

        // 应该返回错误
        assert!(result.is_err());
        // 错误信息应该包含文件开头内容
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("未找到"));
        assert!(err_msg.contains("hello world"));
    }

    /// 测试多次匹配时返回错误
    #[tokio::test]
    async fn test_multiple_matches() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.txt");
        std::fs::write(&file_path, "重复文本\n重复文本\n重复文本\n").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "重复文本",
                "new_string": "新文本"
            }))
            .await;

        // 应该返回错误（多次匹配）
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("3 处匹配"));
    }

    /// 测试 old_string 和 new_string 相同时报错
    #[tokio::test]
    async fn test_same_old_new() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "hello"
            }))
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("相同"));
    }

    /// 测试多行替换
    #[tokio::test]
    async fn test_multiline_replace() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    // TODO: 实现功能\n}\n").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "    // TODO: 实现功能",
                "new_string": "    println!(\"已实现\");"
            }))
            .await;

        assert!(result.is_ok());

        // 验证多行替换成功
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("已实现"));
        assert!(!content.contains("TODO"));
    }
}
