# Pigs Mini Agent —— 教学版最简 Rust AI Agent

> "把 Claude Code 或 Cursor 剥到底，核心就是一个套着大模型的 `while` 循环，加七八个让它能动手的工具。"
> —— CoreCoder README

这个 crate 是一个**自包含的、教学导向的**通用 AI Agent 实现。不到 2000 行 Rust 代码，没有任何黑魔法。

## 这个 crate 是什么？

它是 AI Agent 的"nanoGPT"——用最少的代码展示 Agent 最核心的工作原理。如果你想知道 Claude Code、Cursor、Codex 这些工具"底下到底在干什么"，读这个 crate 就够了。

## 核心概念

Agent 的核心就是一个循环：

```
┌──────────────────────────────────────────────────┐
│                Agent 核心循环                      │
│                                                   │
│  1. 用户发消息 ──→ 加入对话历史                      │
│                      │                            │
│  2. 调用 LLM ◄───────┘                            │
│     （把历史 + 工具列表发给大模型）                    │
│                      │                            │
│  3. LLM 回复 ────────┘                            │
│     ├── 纯文本 ──→ 返回给用户 ✓ 完成                 │
│     └── 工具调用 ──→ 执行工具 ──→ 结果加入历史        │
│                      │                            │
│  4. 回到步骤 2 ↺ 循环                               │
└──────────────────────────────────────────────────┘
```

就这么简单。Agent 的"智能"来自 LLM（大语言模型），Agent 代码本身不做任何决策——它只是忠实地执行循环。

## 快速开始

```bash
# 1. 设置 API 密钥（任选一个供应商）
export OPENAI_API_KEY="sk-xxxx"

# 2. 运行示例
cargo run --example chat
```

支持任何 OpenAI 兼容供应商：

```bash
# OpenAI
export OPENAI_BASE_URL="https://api.openai.com/v1"
export OPENAI_MODEL="gpt-4o"

# DeepSeek
export OPENAI_BASE_URL="https://api.deepseek.com/v1"
export OPENAI_MODEL="deepseek-chat"

# Qwen (通义千问)
export OPENAI_BASE_URL="https://dashscope.aliyuncs.com/compatible-mode/v1"
export OPENAI_MODEL="qwen-plus"

# Ollama (本地)
export OPENAI_BASE_URL="http://localhost:11434/v1"
export OPENAI_MODEL="llama3"
```

## 代码示例

```rust
use pigs_mini_agent::{Agent, LlmClient, create_default_tools};

#[tokio::main]
async fn main() -> pigs_mini_agent::Result<()> {
    // 1. 创建 LLM 客户端
    let llm = LlmClient::from_env()?;
    // 2. 创建工具集（bash, read_file, write_file, edit_file）
    let tools = create_default_tools();
    // 3. 创建 Agent
    let mut agent = Agent::new(llm, tools);
    // 4. 聊天！
    let response = agent.chat("帮我创建一个 hello.txt 文件").await?;
    println!("{response}");
    Ok(())
}
```

## 教学文档

本 crate 有三层文档：

| 层级 | 路径 | 说明 |
|---|---|---|
| **HTML 站点**（推荐） | [`docs/html/index.html`](docs/html/index.html) | 深色「Terminal Codex」风格；每章末尾有源码拆解 |
| Markdown 章节 | [`docs/`](docs/) | `00`…`07`；每章含概念说明 + **源码拆解** |
| 拆解片段源 | [`docs/_walkthroughs/`](docs/_walkthroughs/) | 拼进 MD/HTML 的详细代码导读 |
| API 文档 | `cargo doc -p pigs-mini-agent --open` | 由源码 `///` 注释生成 |

在浏览器中打开 HTML 总览：

```bash
# Windows
start crates/pigs-mini-agent/docs/html/index.html

# macOS
open crates/pigs-mini-agent/docs/html/index.html

# Linux
xdg-open crates/pigs-mini-agent/docs/html/index.html
```

快捷键：`←` / `→` 在章节间翻页。

## 文件结构与阅读顺序

建议按以下顺序阅读，从简单到核心：

| 顺序 | 文件 | 内容 | 行数 |
|---|---|---|---|
| 1 | `src/message.rs` | 消息模型——对话历史的数据结构 | ~230 |
| 2 | `src/tool.rs` | 工具系统——`Tool` trait 和 `ToolRegistry` | ~250 |
| 3 | `src/prompt.rs` | 系统提示词——如何"设定" Agent 的行为 | ~130 |
| 4 | `src/llm.rs` | LLM 客户端——如何与大模型 API 交互 | ~310 |
| 5 | `src/tools/` | 4 个内置工具的具体实现 | ~480 |
| 6 | `src/agent.rs` | **Agent 循环核心——这是心脏** | ~260 |
| 7 | `src/error.rs` | 错误处理——Rust 的错误处理模式 | ~130 |
| 8 | `src/lib.rs` | 模块声明和 crate 级文档 | ~100 |

## 内置工具

| 工具 | 功能 | 借鉴来源 |
|---|---|---|
| `bash` | 执行 shell 命令 | CoreCoder `bash.py` + pigs-tools `bash.rs` |
| `read_file` | 读取文件内容（带行号） | CoreCoder `read.py` |
| `write_file` | 写入文件（自动创建父目录） | CoreCoder `write.py` |
| `edit_file` | 搜索替换编辑（唯一匹配） | CoreCoder `edit.py`（核心创新） |

### 为什么 edit_file 用唯一匹配而非行号？

来自 CoreCoder 的设计哲学：

> "行号是陷阱，模型数错一行就悄悄改错地方。用唯一片段锚定，失败可恢复、成功可验证。"

`edit_file` 要求 LLM 提供 `old_string`（要替换的文本）和 `new_string`（替换后的文本）。如果 `old_string` 在文件中不唯一，LLM 需要加入更多上下文使其唯一。比行号定位更可靠——LLM 经常数错行号。

## 设计决策

### 为什么自包含？

不依赖 `pigs-core`、`pigs-llm` 等其他 crate。读者只看这一个 crate 就能理解完整 Agent 循环，不需要在多个 crate 之间跳转。

### 为什么用中文注释？

教学目的。中文注释让中文读者更容易理解"为什么"这样设计，而不只是"做了什么"。

### 为什么用非流式而非流式？

流式响应需要处理 SSE 事件流和工具参数碎片重组，增加约 200 行复杂代码。教学版优先展示核心逻辑。流式实现可参考 `pigs-llm` crate 的 `openai.rs`。

## 与参考项目的关系

| 参考项目 | 语言 | 本 crate 借鉴了什么 |
|---|---|---|
| CoreCoder | Python | 极简循环骨架、工具设计、edit_file 唯一匹配、系统提示词 |
| claw-code/claw-analog | Rust | Rust Agent 循环模式、工具 match 分发 |
| codex | Rust | 分层架构理念、消息类型设计 |
| pigs-core/cli | Rust | 类型安全 enum、thiserror 错误模式 |

## 许可证

MIT
