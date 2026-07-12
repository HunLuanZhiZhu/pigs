//! # Pigs Mini Agent —— 教学版最简 Rust AI Agent
//!
//! 这是一个**自包含的、教学导向的**通用 AI Agent crate。
//!
//! ## 这个 crate 是什么？
//!
//! 如果把 Claude Code、Cursor、Codex 这样的编程 Agent 剥到底，
//! 核心就是一个**套着大模型的循环**，加几个让它能动手的工具。
//!
//! 这个 crate 就是那个"剥到底"的最小实现。它展示了 AI Agent
//! 最核心的工作原理——不到 2000 行 Rust 代码，没有任何黑魔法。
//!
//! ## 核心概念
//!
//! Agent 的工作模式可以用 4 步概括：
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │                  Agent 核心循环                        │
//! │                                                       │
//! │  1. 用户发消息 ──→ 加入对话历史                         │
//! │                      │                                │
//! │  2. 调用 LLM ◄───────┘                                │
//! │     （把历史 + 工具列表发给大模型）                      │
//! │                      │                                │
//! │  3. LLM 回复 ────────┘                                │
//! │     ├── 纯文本 ──→ 返回给用户 ✓ 完成                    │
//! │     └── 工具调用 ──→ 执行工具 ──→ 结果加入历史 ──→ 回到 2 │
//! │                                                       │
//! │  4. 循环直到 LLM 不再需要工具，或达到最大轮次             │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! ## 快速开始
//!
//! ```no_run
//! use pigs_mini_agent::{Agent, LlmClient, create_default_tools};
//!
//! #[tokio::main]
//! async fn main() -> pigs_mini_agent::Result<()> {
//!     // 1. 创建 LLM 客户端（需要设置 OPENAI_API_KEY 环境变量）
//!     let llm = LlmClient::from_env()?;
//!     // 2. 创建工具集（bash, read_file, write_file, edit_file）
//!     let tools = create_default_tools();
//!     // 3. 创建 Agent
//!     let mut agent = Agent::new(llm, tools);
//!     // 4. 聊天！
//!     let response = agent.chat("帮我创建一个 hello.txt 文件，内容是 Hello World").await?;
//!     println!("{response}");
//!     Ok(())
//! }
//! ```
//!
//! ## 阅读顺序建议
//!
//! 建议按以下顺序阅读源码，从简单到核心：
//!
//! 1. [`message`] —— 消息模型：理解对话历史的数据结构
//! 2. [`tool`] —— 工具系统：理解 Agent 如何调用工具
//! 3. [`prompt`] —— 系统提示词：理解如何"设定" Agent 的行为
//! 4. [`llm`] —— LLM 客户端：理解如何与大模型 API 交互
//! 5. [`tools`] —— 内置工具：看 4 个具体工具的实现
//! 6. [`agent`] —— Agent 循环：**这是核心**，看上面那张图
//! 7. [`error`] —— 错误处理：理解 Rust 的错误处理模式
//!
//! ## 设计决策
//!
//! ### 为什么自包含（不依赖 pigs-core 等）？
//! 教学目的——读者只看这一个 crate 就能理解完整 Agent 循环，
//! 不需要在多个 crate 之间跳转。所有类型都自己定义。
//!
//! ### 为什么用中文注释？
//! 这个 crate 的目标是教学。中文注释让中文读者更容易理解
//! "为什么"这样设计，而不只是"做了什么"。
//!
//! ### 为什么选 OpenAI 兼容格式？
//! 因为它是最通用的——OpenAI、DeepSeek、Qwen、Kimi、Ollama
//! 都实现了这个格式。一个客户端就能对接所有供应商。
//!
//! ### 为什么用非流式而非流式？
//! 流式响应需要处理 SSE 事件流和工具参数碎片重组，
//! 增加约 200 行复杂代码。教学版优先展示核心逻辑。
//! 流式实现可参考 `pigs-llm` crate 的 `openai.rs`。
//!
//! ### 为什么 edit_file 用唯一匹配而非行号？
//! 来自 CoreCoder 的设计哲学：
//! > "行号是陷阱，模型数错一行就悄悄改错地方。
//! > 用唯一片段锚定，失败可恢复、成功可验证。"
//!
//! ## 与参考项目的关系
//!
//! | 参考项目 | 语言 | 本 crate 借鉴了什么 |
//! |---|---|---|
//! | CoreCoder | Python | 极简循环骨架、工具设计、edit_file 唯一匹配、系统提示词 |
//! | claw-code/claw-analog | Rust | Rust Agent 循环模式、工具 match 分发 |
//! | codex | Rust | 分层架构理念、消息类型设计 |
//! | pigs-core/cli | Rust | 类型安全 enum、thiserror 错误模式 |

// === 模块声明 ===
// 教学要点：Rust 的模块系统——每个 .rs 文件是一个模块，
// 通过 `pub mod` 声明后可以在 crate 外部访问。

/// 错误类型模块 —— 定义统一的错误类型 `MiniAgentError`
pub mod error;

/// 消息模型模块 —— 定义 `Message`、`Role`、`ToolCall` 等
pub mod message;

/// 工具系统模块 —— 定义 `Tool` trait 和 `ToolRegistry`
pub mod tool;

/// LLM 客户端模块 —— 定义 `LlmClient` 和 `LlmResponse`
pub mod llm;

/// 系统提示词模块 —— 构建动态系统提示词
pub mod prompt;

/// Agent 循环模块 —— 定义 `Agent` 结构体和核心循环
pub mod agent;

/// 内置工具模块 —— 4 个具体工具的实现
pub mod tools;

// === 公开重导出 ===
// 教学要点：重导出让用户不需要写完整的模块路径，
// 例如 `use pigs_mini_agent::Agent` 而非 `use pigs_mini_agent::agent::Agent`。
// 重导出的类型本身在各自模块中已有完整文档，这里提供快捷导入路径。

/// Agent 核心结构体 —— 实现 [`Agent::chat`] 方法，是整个 crate 的入口。
pub use agent::Agent;

/// 统一错误类型和 `Result` 别名 —— crate 中所有公开函数返回的 `Result<T>`
/// 中的 `E` 类型就是 [`MiniAgentError`]。
pub use error::{MiniAgentError, Result};

/// LLM 客户端（[`LlmClient`]）和 LLM 响应（[`LlmResponse`]）。
pub use llm::{LlmClient, LlmResponse};

/// 消息相关类型：[`Message`]（对话消息）、[`Role`]（消息角色）、[`ToolCall`]（工具调用）。
pub use message::{Message, Role, ToolCall};

/// 系统提示词构造函数 —— 根据工具注册表动态生成提示词。
pub use prompt::build_system_prompt;

/// 工具 trait（[`Tool`]）和工具注册表（[`ToolRegistry`]）。
pub use tool::{Tool, ToolRegistry};

/// 默认工具集工厂函数 —— 创建包含 4 个内置工具的注册表。
pub use tools::create_default_tools;
