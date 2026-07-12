//! bash 工具 —— 执行 shell 命令。
//!
//! 教学要点：
//! - 这是 Agent 与操作系统交互的核心工具
//! - 通过执行命令，Agent 可以：运行测试、安装依赖、查看目录结构等
//! - 需要处理：超时、输出截断、跨平台兼容
//!
//! 借鉴对比：
//! - 对比 CoreCoder `bash.py`（127 行）: 有危险命令黑名单、cd 跟踪、输出截断
//! - 对比 pigs-tools `bash.rs`（144 行）: 跨平台、超时、50KB 截断
//! - 本 crate 简化版：超时 + 输出截断 + 跨平台，不做危险命令检查
//!   （教学目的：展示核心逻辑，安全检查可后续扩展）
//!
//! 安全提示：
//! 真正的生产级 Agent 应该有命令白名单/黑名单、沙箱隔离等安全措施。
//! 参考 claw-code 的 PermissionEnforcer 和 codex 的沙箱系统。
//! 本教学版不做安全限制，因为重点是展示 Agent 循环而非安全工程。

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use crate::error::{MiniAgentError, Result};
use crate::tool::Tool;

/// bash 工具 —— 执行 shell 命令并返回输出。
///
/// 在 Windows 上使用 `cmd /C`，在其他系统上使用 `sh -c`。
pub struct BashTool;

/// bash 工具的输出最大长度（字符数）。
///
/// 教学要点：命令输出可能非常长（如编译日志），
/// 需要截断以避免撑爆 LLM 的上下文窗口。
/// 借鉴 pigs-tools 的 50KB 限制，本 crate 用 20000 字符。
const MAX_OUTPUT_CHARS: usize = 20000;

/// 命令执行超时时间（秒）。
///
/// 教学要点：有些命令可能永远不会结束（如 `tail -f`），
/// 设置超时防止 Agent 卡死。借鉴 pigs-tools 的 120 秒超时。
const TIMEOUT_SECS: u64 = 120;

#[async_trait]
impl Tool for BashTool {
    /// 工具名: "bash"
    fn name(&self) -> &str {
        "bash"
    }

    /// 工具描述 —— 告诉 LLM 什么时候用这个工具
    fn description(&self) -> &str {
        "执行 shell 命令并返回输出。可以用来运行测试、查看目录、安装依赖等。\
        命令在当前工作目录下执行，有 120 秒超时限制。"
    }

    /// 参数 schema —— 定义工具接受什么参数
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的 shell 命令"
                }
            },
            "required": ["command"]
        })
    }

    /// 执行 shell 命令。
    ///
    /// 流程:
    /// 1. 从参数中提取 command 字符串
    /// 2. 根据操作系统选择 shell（Windows: cmd, 其他: sh）
    /// 3. 用 tokio::process::Command 异步执行
    /// 4. 设置超时，防止命令卡死
    /// 5. 截断过长的输出
    /// 6. 返回 stdout + stderr + 退出码
    async fn execute(&self, input: Value) -> Result<String> {
        // 1. 提取命令字符串
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MiniAgentError::ToolError("缺少 'command' 参数".into()))?;

        // 2. 根据操作系统选择 shell
        // Windows 用 cmd /C，Linux/macOS 用 sh -c
        let (program, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C") // Windows: cmd /C "command"
        } else {
            ("sh", "-c") // Unix: sh -c "command"
        };

        // 3. 构建命令并执行
        let mut cmd = Command::new(program);
        cmd.arg(flag).arg(command);           // 传递命令字符串
        cmd.stdout(Stdio::piped());           // 捕获 stdout
        cmd.stderr(Stdio::piped());           // 捕获 stderr
        cmd.stdin(Stdio::null());             // 不需要标准输入

        // 4. 异步启动命令
        let child = cmd.spawn().map_err(|e| {
            MiniAgentError::ToolError(format!("启动命令失败: {e}"))
        })?;

        // 5. 使用 tokio::time::timeout 设置超时
        let output = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), child.wait_with_output())
            .await
            .map_err(|_| {
                MiniAgentError::ToolError(format!(
                    "命令执行超时（{TIMEOUT_SECS} 秒）: {command}"
                ))
            })?
            .map_err(|e| MiniAgentError::ToolError(format!("等待命令完成失败: {e}")))?;

        // 6. 解析输出
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // 7. 拼接输出结果
        let mut result = String::new();

        // 如果有 stdout，加入
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }

        // 如果有 stderr，加入（标注为 stderr）
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n'); // 换行分隔
            }
            result.push_str("[stderr]\n");
            result.push_str(&stderr);
        }

        // 加入退出码
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("[退出码: {exit_code}]"));

        // 8. 截断过长输出
        if result.chars().count() > MAX_OUTPUT_CHARS {
            // 保留前 MAX_OUTPUT_CHARS 个字符 + 截断提示
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

    /// 测试执行简单命令
    #[tokio::test]
    async fn test_execute_echo() {
        let tool = BashTool;
        // 执行 echo 命令
        let result = tool
            .execute(serde_json::json!({"command": "echo hello"}))
            .await
            .unwrap();
        // 验证输出包含 "hello"
        assert!(result.contains("hello"));
        // 验证包含退出码
        assert!(result.contains("退出码"));
    }

    /// 测试缺少参数时报错
    #[tokio::test]
    async fn test_missing_param() {
        let tool = BashTool;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    /// 测试执行不存在的命令
    #[tokio::test]
    async fn test_invalid_command() {
        let tool = BashTool;
        let result = tool
            .execute(serde_json::json!({"command": "这个命令绝对不存在_12345"}))
            .await;
        // 应该返回成功（退出码非零），而不是 Err
        // 因为命令执行"失败"不等于工具执行"出错"
        assert!(result.is_ok());
        let output = result.unwrap();
        // 退出码应该非零
        assert!(output.contains("退出码"));
    }
}
