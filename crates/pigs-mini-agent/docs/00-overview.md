# Pigs Mini Agent 总览

> "把 Claude Code 或 Cursor 剥到底，核心就是一个套着大模型的 `while` 循环，加七八个让它能动手的工具。" —— CoreCoder README

## 这是什么？

`pigs-mini-agent` 是一个**自包含的、教学导向的**通用 AI Agent crate。它用不到 3000 行 Rust 代码展示了 AI Agent 最核心的工作原理——没有任何黑魔法，不依赖任何其他 `pigs-*` crate。

如果你想知道 Claude Code、Cursor、Codex 这些工具"底下到底在干什么"，读这个 crate 就够了。

## 核心循环

Agent 的全部"智能"来自一个循环：

```
┌──────────────────────────────────────────────────────┐
│                  Agent 核心循环                        │
│                                                       │
│  1. 用户发消息 ──→ 加入对话历史                         │
│                      │                                │
│  2. 调用 LLM ◄───────┘                                │
│     （把历史 + 工具列表发给大模型）                      │
│                      │                                │
│  3. LLM 回复 ────────┘                                │
│     ├── 纯文本 ──→ 返回给用户 ✓ 完成                    │
│     └── 工具调用 ──→ 执行工具 ──→ 结果加入历史 ──→ 回到 2 │
│                                                       │
│  4. 循环直到 LLM 不再需要工具，或达到最大轮次             │
└──────────────────────────────────────────────────────┘
```

Agent 代码本身不做任何"决策"——它只是忠实地执行循环。所有"思考"都来自 LLM（大语言模型），Agent 只是 LLM 的"手脚"。

## 文件结构

```
crates/pigs-mini-agent/
├── Cargo.toml                     # crate 配置
├── README.md                      # 快速上手指南
├── docs/                          # ← 你在这里（独立教学文档）
│   ├── 00-overview.md             # 总览（本文）
│   ├── 01-message.md              # 消息模型详解
│   ├── 02-tool.md                 # 工具系统详解
│   ├── 03-prompt.md               # 系统提示词详解
│   ├── 04-llm.md                  # LLM 客户端详解
│   ├── 05-tools.md                # 内置工具详解
│   ├── 06-agent.md                # Agent 循环详解（核心）
│   └── 07-error.md                # 错误处理详解
├── src/
│   ├── lib.rs                     # 模块声明 + crate 级文档
│   ├── message.rs                 # 消息模型
│   ├── tool.rs                    # 工具 trait + 注册表
│   ├── prompt.rs                  # 系统提示词
│   ├── llm.rs                     # LLM 客户端
│   ├── agent.rs                   # Agent 循环核心
│   ├── error.rs                   # 统一错误类型
│   └── tools/
│       ├── mod.rs                 # 工具注册工厂
│       ├── bash.rs                # 执行 shell 命令
│       ├── read_file.rs           # 读取文件
│       ├── write_file.rs          # 写入文件
│       └── edit_file.rs           # 搜索替换编辑
└── examples/
    └── chat.rs                    # 可运行终端聊天示例
```

## 推荐阅读顺序

每个 `docs/` 文档对应一个源码文件，建议按以下顺序阅读：

| 顺序 | 文档 | 对应源文件 | 核心问题 |
|---|---|---|---|
| 1 | [消息模型](01-message.md) | `src/message.rs` | 对话历史的数据结构是什么？ |
| 2 | [工具系统](02-tool.md) | `src/tool.rs` | Agent 如何调用工具？ |
| 3 | [系统提示词](03-prompt.md) | `src/prompt.rs` | 如何设定 Agent 的行为？ |
| 4 | [LLM 客户端](04-llm.md) | `src/llm.rs` | 如何与大模型 API 交互？ |
| 5 | [内置工具](05-tools.md) | `src/tools/` | 具体工具怎么实现？ |
| 6 | [Agent 循环](06-agent.md) | `src/agent.rs` | **核心：循环是怎么跑的？** |
| 7 | [错误处理](07-error.md) | `src/error.rs` | 出错了怎么办？ |

每一章正文之后都有 **「源码拆解」** 小节：按阅读顺序拆类型、字段、方法与调用链，对应 `src/` 里的真实代码（片段来源见 `docs/_walkthroughs/`）。

## 快速开始

```bash
# 设置 API 密钥（任选一个供应商）
export OPENAI_API_KEY="sk-xxxx"

# 运行示例
cargo run --example chat
```

代码中使用：

```rust
use pigs_mini_agent::{Agent, LlmClient, create_default_tools};

#[tokio::main]
async fn main() -> pigs_mini_agent::Result<()> {
    let llm = LlmClient::from_env()?;
    let tools = create_default_tools();
    let mut agent = Agent::new(llm, tools);
    let response = agent.chat("帮我创建一个 hello.txt 文件").await?;
    println!("{response}");
    Ok(())
}
```

## 设计决策

### 为什么自包含？

不依赖 `pigs-core`、`pigs-llm` 等其他 crate。读者只看这一个 crate 就能理解完整 Agent 循环，不需要在多个 crate 之间跳转。所有类型都自己定义。

### 为什么用中文注释和文档？

教学目的。中文注释让中文读者更容易理解"为什么"这样设计，而不只是"做了什么"。

### 为什么用 OpenAI 兼容格式？

因为它是最通用的——OpenAI、DeepSeek、Qwen、Kimi、Ollama 都实现了这个格式。一个客户端就能对接所有供应商。

### 为什么用非流式而非流式？

流式响应需要处理 SSE 事件流和工具参数碎片重组，增加约 200 行复杂代码。教学版优先展示核心逻辑。流式实现可参考 `pigs-llm` crate 的 `openai.rs`。

### 为什么 edit_file 用唯一匹配而非行号？

来自 CoreCoder 的设计哲学：

> "行号是陷阱，模型数错一行就悄悄改错地方。用唯一片段锚定，失败可恢复、成功可验证。"

## 与参考项目的关系

| 参考项目 | 语言 | 本 crate 借鉴了什么 |
|---|---|---|
| **CoreCoder** | Python | 极简循环骨架、工具设计、edit_file 唯一匹配、系统提示词动态拼接 |
| **claw-code/claw-analog** | Rust | Rust Agent 循环模式、工具 match 分发 |
| **codex** | Rust | 分层架构理念、消息类型设计 |
| **pigs-core/cli** | Rust | 类型安全 enum、thiserror 错误模式 |

## HTML 文档站点

同目录下的 [`html/index.html`](html/index.html) 是一套独立的深色教学站点（Terminal Codex 风格），含：

- 侧边栏导航与章节页
- Agent 核心循环可视化
- 键盘 `←` / `→` 翻页
- 代码块一键复制

建议用浏览器直接打开 `docs/html/index.html` 阅读。

## API 文档

除了本目录下的教学文档外，运行 `cargo doc -p pigs-mini-agent --open` 可以查看自动生成的 API 参考文档（基于源码中的 `///` 文档注释）。
