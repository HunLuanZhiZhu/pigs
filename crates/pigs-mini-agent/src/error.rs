//! 统一错误类型 —— 整个 crate 的错误处理基础。
//!
//! 教学要点：
//! - Rust 的错误处理不靠异常，靠 `Result<T, E>` 类型
//! - `thiserror` crate 可以自动为 enum 实现 `Display` 和 `Error` trait
//! - 一个统一的错误类型让 crate 内部的错误传递更简单（只需 `?` 操作符）
//!
//! 借鉴对比：
//! - 对比 pigs-core: 那里把错误分成了 `ApiError` 和 `ToolError` 两个类型
//! - 对比 CoreCoder: Python 里用 try/except + 字符串错误，没有类型安全
//! - 本 crate 选择一个统一的 `MiniAgentError`，简化教学复杂度

use thiserror::Error;

/// 统一错误类型 —— crate 中所有操作可能返回的错误。
///
/// 每个变体代表一类错误场景。用 `#[error("...")]` 属性定义
/// 错误的显示格式，`thiserror` 会据此自动实现 `Display` trait。
#[derive(Debug, Error)]
pub enum MiniAgentError {
    /// LLM API 调用失败 —— 例如模型返回了错误响应、认证失败等。
    ///
    /// 常见场景：API Key 无效、模型名称错误、请求格式不对
    #[error("LLM 调用失败: {0}")]
    LlmError(String),

    /// 工具执行失败 —— 例如文件不存在、命令执行超时等。
    ///
    /// 常见场景：read_file 找不到文件、bash 命令返回非零退出码
    /// 注意：工具执行的"软错误"（如命令返回非零）不一定会触发这个，
    /// 而是把错误信息作为工具结果返回给 LLM，让 LLM 决定下一步。
    #[error("工具执行失败: {0}")]
    ToolError(String),

    /// 网络请求失败 —— HTTP 连接超时、DNS 解析失败等。
    ///
    /// 与 `LlmError` 的区别：这是底层网络错误（reqwest 层），
    /// `LlmError` 是 API 层面的错误（HTTP 200 但返回了错误 JSON）
    #[error("网络错误: {0}")]
    NetworkError(String),

    /// JSON 解析失败 —— LLM 返回的数据不符合预期格式。
    ///
    /// 常见场景：LLM 返回的 tool_calls 参数不是合法 JSON
    /// （大语言模型有时会生成格式不正确的 JSON）
    #[error("解析错误: {0}")]
    ParseError(String),

    /// 文件 IO 失败 —— 读写文件时发生的操作系统级错误。
    ///
    /// 常见场景：权限不足、磁盘已满、路径包含非法字符
    #[error("IO 错误: {0}")]
    IoError(String),

    /// Agent 循环达到最大轮次 —— 防止 Agent 无限循环的安全机制。
    ///
    /// 设计原因（借鉴 CoreCoder 的 max_rounds=50）：
    /// Agent 可能在某些场景下陷入"调用工具 → LLM 再次要求调用工具"的
    /// 无限循环。设置最大轮次作为安全阀，超过后强制停止。
    /// 这不是"错误"而是"保护"——就像 while 循环要避免无限循环一样。
    #[error("Agent 达到最大轮次限制 ({0} 轮)，强制停止")]
    MaxRoundsReached(u32),

    /// 配置错误 —— 缺少必要的环境变量或配置项。
    ///
    /// 常见场景：没有设置 OPENAI_API_KEY 环境变量
    #[error("配置错误: {0}")]
    ConfigError(String),
}

/// 为 `std::io::Error` 实现 `From` trait，
/// 这样在代码里可以用 `?` 操作符自动将 IO 错误转换为本 crate 的错误类型。
///
/// 教学要点：`From` trait 是 Rust 错误转换的核心机制。
/// 实现了 `From<E>` 之后，`Result<T, E>` 就可以用 `?` 自动转为 `Result<T, MyError>`
impl From<std::io::Error> for MiniAgentError {
    /// 将标准库 IO 错误转换为 `MiniAgentError::IoError`
    fn from(err: std::io::Error) -> Self {
        MiniAgentError::IoError(err.to_string())
    }
}

/// 为 `reqwest::Error` 实现 `From` trait，
/// 这样 HTTP 请求的错误可以用 `?` 自动转换。
impl From<reqwest::Error> for MiniAgentError {
    /// 将 reqwest 网络错误转换为 `MiniAgentError::NetworkError`
    fn from(err: reqwest::Error) -> Self {
        MiniAgentError::NetworkError(err.to_string())
    }
}

/// 为 `serde_json::Error` 实现 `From` trait，
/// 这样 JSON 解析的错误可以用 `?` 自动转换。
impl From<serde_json::Error> for MiniAgentError {
    /// 将 JSON 序列化/反序列化错误转换为 `MiniAgentError::ParseError`
    fn from(err: serde_json::Error) -> Self {
        MiniAgentError::ParseError(err.to_string())
    }
}

/// crate 级的 Result 类型别名。
///
/// 教学要点：定义 `type Result<T> = std::result::Result<T, MyError>`
/// 是 Rust 的惯用模式，让函数签名更简洁。
/// 这样 `fn foo() -> Result<String>` 自动意味着 `Result<String, MiniAgentError>`
pub type Result<T> = std::result::Result<T, MiniAgentError>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 验证错误类型的 Display 实现和 From 转换

    use super::*;

    /// 测试 IO 错误可以自动转换
    #[test]
    fn test_io_error_conversion() {
        // 创建一个 IO 错误
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "文件不存在");
        // 用 From trait 转换
        let agent_err: MiniAgentError = io_err.into();
        // 验证转换后是 IoError 变体
        assert!(matches!(agent_err, MiniAgentError::IoError(_)));
    }

    /// 测试 JSON 错误可以自动转换
    #[test]
    fn test_json_error_conversion() {
        // 故意解析非法 JSON，产生 serde_json::Error
        let json_err = serde_json::from_str::<serde_json::Value>("非法 JSON").unwrap_err();
        // 用 From trait 转换
        let agent_err: MiniAgentError = json_err.into();
        // 验证转换后是 ParseError 变体
        assert!(matches!(agent_err, MiniAgentError::ParseError(_)));
    }

    /// 测试错误显示包含原始信息
    #[test]
    fn test_error_display() {
        let err = MiniAgentError::ToolError("文件不存在: /tmp/test.txt".to_string());
        let display = format!("{err}");
        // Display 应该包含我们定义的前缀和原始信息
        assert!(display.contains("工具执行失败"));
        assert!(display.contains("文件不存在"));
    }

    /// 测试 MaxRoundsReached 的轮次信息
    #[test]
    fn test_max_rounds_display() {
        let err = MiniAgentError::MaxRoundsReached(50);
        let display = format!("{err}");
        // Display 应该包含轮次数字
        assert!(display.contains("50"));
    }
}
