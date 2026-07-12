//! write_file 工具 —— 写入文件。
//!
//! 教学要点：
//! - 这是 Agent 创建新文件的工具
//! - 需要处理：自动创建父目录（用户体验）
//! - 与 edit_file 的区别：write_file 是完全重写，edit_file 是局部修改
//!
//! 借鉴对比：
//! - 对比 CoreCoder `write.py`（38 行）: mkdir(parents=True) 自动建目录
//! - 对比 pigs-tools `write_file.rs`（130 行）: 同样自动建目录
//! - 本 crate 与两者一致：自动创建父目录
//!
//! 设计决策：什么时候用 write_file，什么时候用 edit_file？
//! 来自 CoreCoder 的行为规则：
//! - write_file: 新建文件 或 完全重写（内容大幅变化）
//! - edit_file: 小范围修改（改几行、改几个词）

use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use crate::error::{MiniAgentError, Result};
use crate::tool::Tool;

/// write_file 工具 —— 将内容写入文件（覆盖已有内容）。
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    /// 工具名: "write_file"
    fn name(&self) -> &str {
        "write_file"
    }

    /// 工具描述
    fn description(&self) -> &str {
        "将内容写入文件。如果文件已存在则覆盖。\
        会自动创建不存在的父目录。\
        适用于创建新文件或完整重写文件内容。\
        小范围修改请使用 edit_file。"
    }

    /// 参数 schema
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要写入的文件路径"
                },
                "content": {
                    "type": "string",
                    "description": "要写入的文件内容"
                }
            },
            "required": ["path", "content"]
        })
    }

    /// 执行文件写入。
    ///
    /// 流程:
    /// 1. 提取 path 和 content 参数
    /// 2. 如果父目录不存在，自动创建
    /// 3. 异步写入文件
    /// 4. 返回成功信息（包含写入的字符数）
    async fn execute(&self, input: Value) -> Result<String> {
        // 1. 提取路径参数（必需）
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'path' 参数".into()))?;

        // 2. 提取内容参数（必需）
        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'content' 参数".into()))?;

        // 3. 获取父目录路径
        let path_obj = std::path::Path::new(path);
        if let Some(parent) = path_obj.parent() {
            // 如果父目录不存在，自动创建
            // create_dir_all 会递归创建所有不存在的父目录
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|e| {
                        MiniAgentError::ToolError(format!(
                            "创建目录失败 '{}': {e}",
                            parent.display()
                        ))
                    })?;
            }
        }

        // 4. 写入文件
        fs::write(path, content)
            .await
            .map_err(|e| MiniAgentError::ToolError(format!("写入文件失败 '{path}': {e}")))?;

        // 5. 返回成功信息
        let char_count = content.chars().count();
        let line_count = content.lines().count();
        Ok(format!(
            "文件已写入: {path} ({char_count} 字符, {line_count} 行)"
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 测试写入新文件
    #[tokio::test]
    async fn test_write_new_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("test.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "hello world\n第二行"
            }))
            .await
            .unwrap();

        // 验证返回信息
        assert!(result.contains("文件已写入"));
        assert!(result.contains("test.txt"));

        // 验证文件内容
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello world\n第二行");
    }

    /// 测试自动创建父目录
    #[tokio::test]
    async fn test_create_parent_dirs() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path()
            .join("dir1")
            .join("dir2")
            .join("dir3")
            .join("test.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "嵌套目录测试"
            }))
            .await;

        // 应该成功，父目录被自动创建
        assert!(result.is_ok());

        // 验证文件确实存在
        assert!(file_path.exists());
    }

    /// 测试覆盖已有文件
    #[tokio::test]
    async fn test_overwrite_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("existing.txt");

        // 先写入初始内容
        std::fs::write(&file_path, "旧内容").unwrap();

        let tool = WriteFileTool;
        tool.execute(serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "新内容"
        }))
        .await
        .unwrap();

        // 验证内容被覆盖
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "新内容");
    }

    /// 测试缺少参数
    #[tokio::test]
    async fn test_missing_params() {
        let tool = WriteFileTool;

        // 缺少 path
        let result = tool.execute(serde_json::json!({"content": "test"})).await;
        assert!(result.is_err());

        // 缺少 content
        let result = tool.execute(serde_json::json!({"path": "/tmp/test.txt"})).await;
        assert!(result.is_err());
    }
}
