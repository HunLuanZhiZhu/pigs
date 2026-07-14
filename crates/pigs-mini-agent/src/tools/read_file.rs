//! read_file 工具 —— 读取文件内容。
//!
//! 教学要点：
//! - 这是 Agent 获取信息的基本工具——读代码、读配置、读文档
//! - 需要处理：行号显示、范围读取、大文件截断
//!
//! 借鉴对比：
//! - 对比 CoreCoder `read.py`（53 行）: 行号格式 `{n}\t{line}`，offset/limit
//! - 对比 pigs-tools `read_file.rs`（164 行）: 100KB 截断
//! - 本 crate 与两者一致：行号 + offset/limit + 10KB 截断
//!
//! 设计决策：为什么用行号格式？
//! 来自 CoreCoder 的设计——`{行号}\t{行内容}`。
//! 好处：LLM 可以通过行号定位（虽然 edit_file 不用行号，但行号帮助 LLM 理解文件结构）。
//! 也方便用户在终端中查看。

use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use crate::error::{MiniAgentError, Result};
use crate::tool::Tool;

/// read_file 工具 —— 读取文件内容并带行号返回。
pub struct ReadFileTool;

/// 输出最大长度（字符数）—— 防止大文件撑爆上下文。
const MAX_OUTPUT_CHARS: usize = 10000;

#[async_trait]
impl Tool for ReadFileTool {
    /// 工具名: "read_file"
    fn name(&self) -> &str {
        "read_file"
    }

    /// 工具描述
    fn description(&self) -> &str {
        "读取指定路径的文件内容。返回带行号的文本。\
        可以通过 offset 和 limit 参数读取文件的指定范围。\
        建议在修改文件前先读取文件内容。"
    }

    /// 参数 schema
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要读取的文件路径"
                },
                "offset": {
                    "type": "integer",
                    "description": "起始行号（从 1 开始），默认 1"
                },
                "limit": {
                    "type": "integer",
                    "description": "读取的行数，默认 2000"
                }
            },
            "required": ["path"]
        })
    }

    /// 执行文件读取。
    ///
    /// 流程:
    /// 1. 提取 path、offset、limit 参数
    /// 2. 异步读取文件内容
    /// 3. 按行号格式化输出
    /// 4. 应用 offset/limit 范围
    /// 5. 截断过长输出
    async fn execute(&self, input: Value) -> Result<String> {
        // 1. 提取路径参数（必需）
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'path' 参数".into()))?;

        // 2. 提取可选参数
        let offset = input.get("offset").and_then(|v| v.as_i64()).unwrap_or(1) as usize; // 默认从第 1 行开始
        let limit = input.get("limit").and_then(|v| v.as_i64()).unwrap_or(2000) as usize; // 默认读取 2000 行

        // 3. 异步读取文件
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| MiniAgentError::ToolError(format!("读取文件失败 '{path}': {e}")))?;

        // 4. 按行分割
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // 5. 应用 offset（起始行，从 1 开始计数）
        let start = if offset > 0 { offset - 1 } else { 0 }; // 转为 0-based 索引
        let start = start.min(total_lines); // 防止越界

        // 6. 应用 limit（读取行数）
        let end = (start + limit).min(total_lines); // 防止越界

        // 7. 格式化输出：行号 + 行内容
        let mut result = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            // 行号从 offset 开始计数
            let line_num = start + i + 1; // 1-based 行号
            result.push_str(&format!("{line_num}\t{line}\n"));
        }

        // 8. 如果不是从第 1 行开始，标注范围
        if start > 0 {
            result = format!(
                "（显示第 {offset} ~ {} 行，共 {total_lines} 行）\n\n{result}",
                start + (end - start)
            );
        } else if end < total_lines {
            // 如果没读完，标注总行数
            result = format!("{result}（共 {total_lines} 行，只显示前 {end} 行）");
        }

        // 9. 截断过长输出
        if result.chars().count() > MAX_OUTPUT_CHARS {
            let truncated: String = result.chars().take(MAX_OUTPUT_CHARS).collect();
            let total = result.chars().count();
            result = format!(
                "{truncated}\n\n... (输出已截断，共 {total} 字符，只显示前 {MAX_OUTPUT_CHARS} 字符)"
            );
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    /// 测试读取文件
    #[tokio::test]
    async fn test_read_file() {
        // 创建临时文件（NamedTempFile 有 .path() 方法）
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "第一行").unwrap();
        writeln!(tmp, "第二行").unwrap();
        writeln!(tmp, "第三行").unwrap();
        // 刷新确保内容写入磁盘
        tmp.flush().unwrap();

        let tool = ReadFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap()
            }))
            .await
            .unwrap();

        // 验证包含行号和内容
        assert!(result.contains("1\t第一行"));
        assert!(result.contains("2\t第二行"));
        assert!(result.contains("3\t第三行"));
    }

    /// 测试 offset 参数
    #[tokio::test]
    async fn test_read_file_with_offset() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "第一行").unwrap();
        writeln!(tmp, "第二行").unwrap();
        writeln!(tmp, "第三行").unwrap();
        tmp.flush().unwrap();

        let tool = ReadFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap(),
                "offset": 2  // 从第 2 行开始
            }))
            .await
            .unwrap();

        // 应该只包含第 2 行和第 3 行
        assert!(!result.contains("第一行"));
        assert!(result.contains("2\t第二行"));
        assert!(result.contains("3\t第三行"));
    }

    /// 测试读取不存在的文件
    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadFileTool;
        let result = tool
            .execute(serde_json::json!({
                "path": "/这个路径/绝对/不存在/文件.txt"
            }))
            .await;
        assert!(result.is_err());
    }

    /// 测试缺少参数
    #[tokio::test]
    async fn test_missing_param() {
        let tool = ReadFileTool;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
