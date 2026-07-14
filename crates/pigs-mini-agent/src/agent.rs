//! Agent 循环核心 —— 整个 crate 的心脏。
//!
//! 教学要点：
//! - Agent 的核心就是一个 `while`/`for` 循环
//! - 循环内容：调 LLM → 有工具调用？执行工具，继续循环 : 返回文本，结束
//! - 有界循环防止 Agent 无限循环
//!
//! 借鉴对比：
//! - 对比 CoreCoder `agent.py` 的 12 行骨架:
//!   ```python
//!   for _ in range(self.max_rounds):
//!       reply = self.llm.chat(self.messages, self.tools)
//!       if not reply.tool_calls:
//!           return reply.text
//!       results = run_parallel(reply.tool_calls)
//!       self.messages += results
//!   return "(hit the round limit)"
//!   ```
//!   本 crate 是这个骨架的 Rust 翻译。
//!
//! - 对比 claw-analog `lib.rs:run()`:
//!   那里是 async fn run()，循环结构相同，但多了流式处理和权限检查。
//!   本 crate 简化版：无流式、无权限，纯循环。
//!
//! - 对比 codex `turn.rs:run_turn()`:
//!   那里有 300+ 行，包含 pending input、auto-compact、hooks、cancellation。
//!   本 crate 只保留最核心的循环逻辑。
//!
//! - 对比 pigs `agent.rs:run_turn()`（439 行）:
//!   那里有流式打印、权限检查、会话持久化、子 Agent。
//!   本 crate 只保留纯 Agent 循环。
//!
//! 核心循环图示：
//! ```text
//! 用户输入 "帮我创建一个 hello.rs 文件"
//!     │
//!     ▼
//! [1] 用户消息加入历史
//!     │
//!     ▼
//! [2] 调用 LLM（历史 + 工具定义）
//!     │
//!     ▼
//! [3] LLM 回复了什么？
//!     │
//!     ├── 纯文本（无工具调用）──→ [4] 返回文本给用户 ✓ 结束
//!     │
//!     └── 工具调用（如 write_file）
//!             │
//!             ▼
//!         [5] 把 assistant 消息（含工具调用）加入历史
//!             │
//!             ▼
//!         [6] 执行工具，把结果加入历史
//!             │
//!             ▼
//!         [7] 回到 [2]，带着工具结果再次调用 LLM ↺ 循环
//! ```

use crate::error::{MiniAgentError, Result};
use crate::llm::LlmClient;
use crate::message::Message;
use crate::prompt::build_system_prompt;
use crate::tool::ToolRegistry;

/// 通用 AI Agent —— 核心循环。
///
/// 这是整个 crate 的核心结构体。它持有一个 LLM 客户端、一组工具、
/// 一段对话历史，以及一个系统提示词。
///
/// 教学要点：Agent 的"智能"来自 LLM（大语言模型）。
/// Agent 代码本身不做任何"思考"——它只是忠实地执行循环：
/// 把历史发给 LLM → 看 LLM 要干什么 → 帮 LLM 执行 → 把结果告诉 LLM。
/// 所有"决策"都是 LLM 做的，Agent 只是 LLM 的"手脚"。
pub struct Agent {
    /// LLM 客户端 —— 连接大语言模型 API
    ///
    /// 教学要点：LLM 是 Agent 的"大脑"。所有的决策、推理、
    /// "知道该用什么工具"都来自 LLM。Agent 代码不做决策。
    pub llm: LlmClient,

    /// 工具注册表 —— Agent 可用的工具集合
    ///
    /// 教学要点：工具是 Agent 的"手脚"。LLM 决定调用什么工具，
    /// 但真正执行工具的是这段 Rust 代码。
    pub tools: ToolRegistry,

    /// 对话历史 —— 所有已发送/接收的消息
    ///
    /// 教学要点：每次调用 LLM 都会发送**完整的**对话历史。
    /// LLM 没有记忆——它靠历史消息"回忆"之前发生了什么。
    /// 历史越长，请求越慢越贵（token 更多）。
    pub messages: Vec<Message>,

    /// 系统提示词 —— 定义 Agent 的行为规则
    ///
    /// 在对话的最前面，通常只有一条。
    /// 每次调用 LLM 时都会把它放在历史消息前面。
    pub system_prompt: String,

    /// 最大循环轮次 —— 防止 Agent 无限循环
    ///
    /// 来自 CoreCoder 的 max_rounds=50。
    /// Agent 可能在某些场景下陷入循环：
    /// "调用工具 → LLM 再次要求调用工具 → 调用工具 → ..."
    /// 设置上限防止卡死。
    pub max_rounds: u32,
}

impl Agent {
    /// 创建一个新的 Agent。
    ///
    /// 参数:
    /// - `llm`: LLM 客户端实例
    /// - `tools`: 工具注册表（包含可用工具）
    ///
    /// 系统提示词会根据工具列表自动生成。
    /// 最大轮次默认为 50（借鉴 CoreCoder）。
    pub fn new(llm: LlmClient, tools: ToolRegistry) -> Self {
        // 动态生成系统提示词——工具列表变化时提示词自动变
        let system_prompt = build_system_prompt(&tools);
        Agent {
            llm,                  // LLM 客户端
            tools,                // 工具注册表
            messages: Vec::new(), // 空对话历史
            system_prompt,        // 自动生成的系统提示词
            max_rounds: 50,       // 默认最大 50 轮
        }
    }

    /// 设置最大循环轮次。
    ///
    /// 教学要点：这是 builder 模式——方法返回 `&mut Self`，
    /// 允许链式调用：`agent.with_max_rounds(30).chat("...")`
    pub fn with_max_rounds(mut self, rounds: u32) -> Self {
        self.max_rounds = rounds;
        self
    }

    /// 设置自定义系统提示词。
    ///
    /// 不使用自动生成的提示词，用自定义的。
    /// 教学要点：系统提示词是 Agent "性格"的来源。
    /// 改提示词就能改变 Agent 的行为方式。
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// 获取完整的消息列表（含系统提示词）。
    ///
    /// 系统提示词不在 `messages` 里，而是在每次调用 LLM 时
    /// 放在消息列表的最前面。这个方法把系统提示词作为
    /// 第一条消息拼接到历史前面。
    fn full_messages(&self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        // 系统提示词作为第一条消息
        messages.push(Message::system(&self.system_prompt));
        // 加上所有对话历史
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// 获取所有工具的 schema 列表。
    ///
    /// 这些 schema 会发给 LLM，告诉它有哪些工具可用。
    fn tool_schemas(&self) -> Vec<serde_json::Value> {
        self.tools.schemas()
    }

    /// 聊天 —— 处理一条用户消息，可能经历多轮 LLM+工具循环。
    ///
    /// 这是整个 crate 最重要的方法。它实现了 Agent 的核心循环。
    ///
    /// 参数:
    /// - `user_input`: 用户输入的文本
    ///
    /// 返回: Agent 的最终文本回复
    ///
    /// 教学要点：看这个方法时，记住循环的本质：
    /// ```text
    /// loop { LLM 回复 → 有工具调用? 执行,继续 : 返回 }
    /// ```
    /// 其他所有代码（错误处理、日志打印）都是辅助。
    pub async fn chat(&mut self, user_input: &str) -> Result<String> {
        // === 步骤 1: 用户消息加入历史 ===
        // 每次用户说话，都把消息加入对话历史
        self.messages.push(Message::user(user_input));

        // === 步骤 2: 有界循环 —— Agent 核心循环 ===
        // 用 for 循环而非 loop，保证不会无限跑
        // 借鉴 CoreCoder: `for _ in range(self.max_rounds)`
        for round in 0..self.max_rounds {
            // 打印当前轮次（帮助理解 Agent 在做什么）
            eprintln!("[agent] 第 {} 轮", round + 1);

            // === 步骤 2.1: 调用 LLM ===
            // 把完整历史（含系统提示词）+ 工具定义发给 LLM
            let messages = self.full_messages();
            let tool_schemas = self.tool_schemas();

            let response = self.llm.chat(&messages, &tool_schemas).await?;

            // === 步骤 2.2: 判断 LLM 是否请求了工具调用 ===
            if response.tool_calls.is_empty() {
                // 没有工具调用 —— LLM 完成了任务，回复了纯文本
                // 把 assistant 消息加入历史
                self.messages
                    .push(Message::assistant_text(&response.content));
                // 返回文本给用户
                return Ok(response.content);
            }

            // === 步骤 2.3: 有工具调用 —— 先把 assistant 消息加入历史 ===
            // 教学要点：必须先 push assistant 消息，再 push tool 结果。
            // 原因（来自 CoreCoder `_answer_pending_tool_calls` 的教训）：
            // OpenAI API 要求 assistant 的 tool_calls 必须有对应的 tool 回复。
            // 如果顺序搞反了，API 会拒绝请求。
            self.messages.push(Message::assistant_with_tools(
                &response.content,
                response.tool_calls.clone(),
            ));

            // 打印 LLM 请求了哪些工具
            for tc in &response.tool_calls {
                eprintln!("[agent] 工具调用: {}({})", tc.name, tc.arguments);
            }

            // === 步骤 2.4: 逐个执行工具 ===
            // 教学要点：这里用 for 循环逐个执行，不并行。
            // CoreCoder 在多个工具时用 ThreadPoolExecutor 并行执行，
            // 但教学版用顺序执行更简单。
            // 并行执行可参考 CoreCoder 的 `_exec_tools_parallel` 或
            // codex 的 `FuturesOrdered` 并发模式。
            for tc in &response.tool_calls {
                // 执行工具
                let result = self.tools.execute(&tc.name, tc.arguments.clone()).await;

                // 根据执行结果构造工具消息
                let (output, is_error) = match result {
                    Ok(output) => {
                        eprintln!(
                            "[agent] 工具 {} 完成: {} 字符",
                            tc.name,
                            output.chars().count()
                        );
                        (output, false) // 成功
                    }
                    Err(e) => {
                        // 教学要点：工具失败时不是 panic，而是把错误信息作为
                        // 工具结果返回给 LLM。这样 LLM 能看到错误并决定
                        // 下一步（比如换一种方式重试）。
                        // 这与 CoreCoder 的 `_exec_tool` 设计一致：
                        // `return f"Error executing {tc.name}: {e}"`
                        let error_msg = e.to_string();
                        eprintln!("[agent] 工具 {} 失败: {}", tc.name, error_msg);
                        (error_msg, true) // 错误
                    }
                };

                // 把工具结果加入历史
                // 注意 tool_call_id 必须与 assistant 消息中的 ToolCall.id 对应
                self.messages
                    .push(Message::tool_result(&tc.id, &output, is_error));
            }

            // === 步骤 2.5: 循环回到步骤 2.1 ===
            // 带着工具执行结果，再次调用 LLM。
            // LLM 会看到工具结果，决定：
            // - 继续调用其他工具（如读了文件之后要编辑）
            // - 完成任务，回复纯文本
            // 这就是 Agent 循环的核心——LLM 和工具交替执行。
        }

        // === 步骤 3: 达到最大轮次，强制停止 ===
        // Agent 循环了 max_rounds 次还没完成，可能是陷入了循环。
        // 借鉴 CoreCoder: `return "(reached maximum tool-call rounds)"`
        Err(MiniAgentError::MaxRoundsReached(self.max_rounds))
    }

    /// 重置对话 —— 清空历史。
    ///
    /// 教学要点：清空历史相当于开始新对话。
    /// LLM 没有记忆——历史清空后它就"忘了"之前的事。
    pub fn reset(&mut self) {
        self.messages.clear();
    }

    /// 获取对话历史长度（消息数）。
    ///
    /// 返回 `messages` 字段中的消息条数。
    /// 注意：这个计数**不包含**系统提示词——系统提示词在每次调用 LLM
    /// 时才通过 `full_messages()` 临时拼接到历史前面，不算在 `messages` 里。
    ///
    /// 教学要点：历史越长，发送给 LLM 的请求越大（token 更多），
    /// 请求越慢越贵。可以用这个方法监控历史长度，在必要时调用
    /// `reset()` 清空历史或实现自己的上下文压缩逻辑。
    pub fn history_len(&self) -> usize {
        self.messages.len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 验证 Agent 的基本功能
    //
    // 注意：完整的 chat() 测试需要 mock LLM 服务器，
    // 这里只测试不需要网络的功能。

    use super::*;
    use crate::llm::LlmClient;
    use crate::tools::create_empty_tools;

    /// 测试 Agent 创建
    #[test]
    fn test_agent_creation() {
        let llm = LlmClient::new("test-key", "https://api.test.com/v1", "test-model");
        let tools = create_empty_tools();
        let agent = Agent::new(llm, tools);

        // 验证初始状态
        assert_eq!(agent.max_rounds, 50); // 默认值
        assert_eq!(agent.history_len(), 0); // 空历史
                                            // 系统提示词应该非空
        assert!(!agent.system_prompt.is_empty());
    }

    /// 测试 builder 模式
    #[test]
    fn test_builder_pattern() {
        let llm = LlmClient::new("test-key", "https://api.test.com/v1", "test-model");
        let tools = create_empty_tools();
        let agent = Agent::new(llm, tools)
            .with_max_rounds(10)
            .with_system_prompt("你是一个测试 Agent");

        assert_eq!(agent.max_rounds, 10);
        assert_eq!(agent.system_prompt, "你是一个测试 Agent");
    }

    /// 测试重置历史
    #[test]
    fn test_reset() {
        let llm = LlmClient::new("test-key", "https://api.test.com/v1", "test-model");
        let tools = create_empty_tools();
        let mut agent = Agent::new(llm, tools);

        // 添加一些消息
        agent.messages.push(Message::user("你好"));
        assert_eq!(agent.history_len(), 1);

        // 重置
        agent.reset();
        assert_eq!(agent.history_len(), 0);
    }

    /// 测试 full_messages 包含系统提示词
    #[test]
    fn test_full_messages() {
        let llm = LlmClient::new("test-key", "https://api.test.com/v1", "test-model");
        let tools = create_empty_tools();
        let mut agent = Agent::new(llm, tools);

        agent.messages.push(Message::user("测试"));

        let full = agent.full_messages();
        // 第一条应该是系统消息
        assert_eq!(full[0].role, crate::message::Role::System);
        // 第二条应该是用户消息
        assert_eq!(full[1].role, crate::message::Role::User);
        assert_eq!(full.len(), 2);
    }
}
