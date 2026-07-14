//! 教学示例 —— 终端聊天 Agent。
//!
//! 这个示例展示如何用 `pigs-mini-agent` crate 创建一个可交互的终端 AI Agent。
//!
//! 运行方式：
//! ```bash
//! # 设置 API 密钥（任选一个供应商）
//! export OPENAI_API_KEY="sk-xxxx"
//!
//! # 可选：指定供应商和模型
//! export OPENAI_BASE_URL="https://api.openai.com/v1"
//! export OPENAI_MODEL="gpt-4o"
//!
//! # 运行示例
//! cargo run --example chat
//! ```
//!
//! 也可以用其他 OpenAI 兼容供应商：
//! ```bash
//! # DeepSeek
//! export OPENAI_API_KEY="sk-xxxx"
//! export OPENAI_BASE_URL="https://api.deepseek.com/v1"
//! export OPENAI_MODEL="deepseek-chat"
//!
//! # Qwen (通义千问)
//! export OPENAI_API_KEY="sk-xxxx"
//! export OPENAI_BASE_URL="https://dashscope.aliyuncs.com/compatible-mode/v1"
//! export OPENAI_MODEL="qwen-plus"
//! ```

use std::io::{self, Write};

use pigs_mini_agent::{create_default_tools, Agent, LlmClient};

/// 终端聊天 Agent 入口。
///
/// 教学要点：整个 `main` 函数只做了 4 件事：
/// 1. 创建 LLM 客户端
/// 2. 创建工具集
/// 3. 创建 Agent
/// 4. REPL 循环（读取输入 → 调用 Agent → 打印回复）
///
/// 就这么简单——这就是一个完整的 AI Agent。
#[tokio::main]
async fn main() -> pigs_mini_agent::Result<()> {
    // 打印欢迎信息
    println!("========================================");
    println!("  Pigs Mini Agent —— 教学版终端 AI Agent");
    println!("========================================");
    println!();

    // === 步骤 1: 创建 LLM 客户端 ===
    // 从环境变量读取配置（OPENAI_API_KEY / OPENAI_BASE_URL / OPENAI_MODEL）
    let llm = LlmClient::from_env()?;

    // === 步骤 2: 创建工具集 ===
    // 注册 4 个内置工具：bash、read_file、write_file、edit_file
    let tools = create_default_tools();

    // === 步骤 3: 创建 Agent ===
    let mut agent = Agent::new(llm, tools);

    // 打印 Agent 信息
    println!("模型: {}", agent.llm.model());
    println!("工具: {}", agent.tools.names().join(", "));
    println!();
    println!("输入消息开始对话，输入 /quit 退出");
    println!();

    // === 步骤 4: REPL 循环 ===
    // REPL = Read-Eval-Print Loop（读取-求值-打印 循环）
    // 这是终端交互程序的标准模式
    loop {
        // --- Read: 读取用户输入 ---
        print!("你> "); // 提示符
        io::stdout().flush().ok(); // 立即刷新，让提示符显示出来

        let mut input = String::new();
        io::stdin().read_line(&mut input)?; // 读取一行
        let input = input.trim(); // 去除首尾空白

        // 空输入跳过
        if input.is_empty() {
            continue;
        }

        // 退出命令
        if input == "/quit" || input == "/exit" {
            println!("再见！");
            break;
        }

        // --- Eval + Print: 调用 Agent 并打印回复 ---
        println!();
        match agent.chat(input).await {
            // 成功 —— 打印 Agent 的回复
            Ok(response) => {
                println!();
                println!("Agent> {response}");
            }
            // 失败 —— 打印错误信息
            Err(e) => {
                eprintln!();
                eprintln!("[错误] {e}");
                eprintln!("（对话历史保留，可以继续输入）");
            }
        }
        println!();
    }

    Ok(())
}
