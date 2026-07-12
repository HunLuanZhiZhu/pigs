# 教学版最简 Rust Agent Crate 实现计划

## 一、设计理念

借鉴 **CoreCoder**（Python "nanoGPT of coding agents"）的极简教学哲学，结合 **claw-analog**（Rust 最简 Agent 循环）和 **codex**（分层架构）的模式，创建一个**完全自包含、零 pigs-* 依赖**的教学 crate。

核心原则（来自 CoreCoder README）：
> "把 Claude Code 或 Cursor 剥到底，核心就是一个套着大模型的 `while` 循环，加七八个让它能动手的工具。"

这个 crate 的目标：**让读者只看这一个 crate，就能理解通用 AI Agent 的完整工作原理。**

## 二、Crate 概览

- **名称**: `pigs-mini-agent`
- **位置**: `D:\AIWorkSpace\pigs\crates\pigs-mini-agent\`
- **规模**: 约 1500-2000 行，多文件
- **依赖**: 仅 `serde` / `serde_json` / `tokio` / `reqwest` / `async-trait` / `thiserror`（全部已在 workspace dependencies 中）
- **注释语言**: 代码注释全部用**中文**（用户明确要求），每行/每个方法都有中文注释
- **不依赖**: 任何 `pigs-*` crate

## 三、文件结构

```
crates/pigs-mini-agent/
├── Cargo.toml
├── README.md                          ← 中文教学导读
├── src/
│   ├── lib.rs                         ← 模块声明 + crate 级文档（约 150 行）
│   ├── message.rs                     ← 消息模型（约 200 行）
│   ├── tool.rs                        ← 工具 trait + 注册表（约 200 行）
│   ├── llm.rs                         ← LLM 客户端（OpenAI 兼容，约 350 行）
│   ├── agent.rs                       ← Agent 循环核心（约 300 行）
│   ├── prompt.rs                      ← 系统提示词构造（约 80 行）
│   ├── tools/
│   │   ├── mod.rs                     ← 工具注册 + 默认工具集（约 80 行）
│   │   ├── bash.rs                    ← 执行 shell 命令（约 120 行）
│   │   ├── read_file.rs               ← 读文件（约 100 行）
│   │   ├── write_file.rs              ← 写文件（约 80 行）
│   │   └── edit_file.rs               ← 搜索替换编辑（约 150 行）
│   └── error.rs                       ← 统一错误类型（约 80 行）
└── examples/
    └── chat.rs                        ← 可运行示例：终端聊天（约 100 行）
```

**预计总行数**: 约 1890 行（含详细中文注释）

## 四、各模块详细设计

### 4.1 `error.rs` — 统一错误类型（约 80 行）

```rust
// 借鉴 pigs-core 的 thiserror 模式，但合并为一个统一错误类型
// 教学目的：展示 Rust 错误处理的最佳实践
pub enum MiniAgentError {
    LlmError(String),        // LLM API 调用失败
    ToolError(String),       // 工具执行失败
    NetworkError(String),    // 网络请求失败
    ParseError(String),      // JSON 解析失败
    IoError(String),         // 文件 IO 失败
    MaxRoundsReached,        // 达到最大轮次
}
```

每个变体都有 `///` 中文文档注释，解释什么场景会产生这个错误。

### 4.2 `message.rs` — 消息模型（约 200 行）

借鉴 CoreCoder 的 OpenAI 原生格式 + pigs-core 的类型安全 enum：

```rust
/// 消息角色 —— 对话中的四种角色
pub enum Role { System, User, Assistant, Tool }

/// 内容块 —— 一条消息可以包含多个内容块
/// 借鉴 Anthropic 的 content block 设计，也兼容 OpenAI 的 tool_calls 格式
pub enum ContentBlock {
    Text { text: String },                              // 纯文本
    ToolUse { id, name, input: Value },                 // 工具调用请求
    ToolResult { tool_use_id, output: String, is_error } // 工具执行结果
}

/// 消息 —— 对话历史中的一条消息
pub struct Message { role: Role, content: Vec<ContentBlock> }
```

关键设计决策（在注释中解释 why）：
- **为什么用 `Vec<ContentBlock>` 而非单个 String**：因为 assistant 消息可以同时包含文本和多个工具调用
- **为什么 ToolResult 是独立角色**：OpenAI 格式用 `role: "tool"`，Anthropic 放到 `role: "user"` 里——这里选 OpenAI 格式更直观
- 提供 `Message::user()`, `Message::assistant_text()`, `Message::assistant_with_tools()`, `Message::tool_result()` 构造器
- 提供 `to_openai_json()` 方法序列化为 OpenAI Chat Completions API 格式

### 4.3 `tool.rs` — 工具系统（约 200 行）

借鉴 CoreCoder 的 `Tool(ABC)` + pigs-core 的 `ToolHandler` trait：

```rust
/// 工具 trait —— 每个工具必须实现这个接口
/// 借鉴 CoreCoder 的 base.py: 三个属性 + 一个 execute 方法
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称 —— LLM 通过这个名字调用工具
    fn name(&self) -> &str;
    /// 工具描述 —— 告诉 LLM 这个工具能做什么
    fn description(&self) -> &str;
    /// 参数 schema —— JSON Schema 格式，告诉 LLM 怎么调用
    fn parameters(&self) -> serde_json::Value;
    /// 执行工具 —— 接收 JSON 参数，返回字符串结果
    async fn execute(&self, input: serde_json::Value) -> Result<String>;

    /// 生成 OpenAI function-calling 格式的 schema
    fn schema(&self) -> serde_json::Value { ... }
}

/// 工具注册表 —— 管理所有可用工具
pub struct ToolRegistry { tools: HashMap<String, Box<dyn Tool>> }
```

注释中详细解释：
- **为什么 `execute` 返回 `String` 而非复杂类型**：来自 CoreCoder 的设计——"所有工具都返回字符串，让消息拼接永远是 str"
- **为什么用 `Box<dyn Tool>` 动态分发**：教学目的，展示 Rust trait object 的用法
- **`schema()` 默认实现**：自动生成 `{"type":"function","function":{"name","description","parameters"}}` 格式

### 4.4 `llm.rs` — LLM 客户端（约 350 行）

借鉴 CoreCoder 的 `llm.py`（OpenAI 兼容客户端）+ pigs-llm 的 Rust 模式：

```rust
/// LLM 客户端 —— 连接 OpenAI 兼容 API 的客户端
/// 支持任何兼容 OpenAI Chat Completions API 的供应商
/// （OpenAI / DeepSeek / Qwen / Kimi / Ollama 等）
pub struct LlmClient {
    http: reqwest::Client,    // HTTP 客户端
    api_key: String,          // API 密钥
    base_url: String,         // API 基础 URL
    model: String,            // 模型名称
}

impl LlmClient {
    /// 发送聊天请求 —— 核心方法
    /// 将消息历史 + 工具定义发送给 LLM，获取回复
    pub async fn chat(&self, messages: &[Message], tools: &[serde_json::Value]) -> Result<LlmResponse>
}

/// LLM 的回复 —— 包含文本内容和可能的工具调用
pub struct LlmResponse {
    pub content: String,                    // 文本回复
    pub tool_calls: Vec<ToolCall>,          // 工具调用列表
}

/// 工具调用 —— LLM 请求执行的工具
pub struct ToolCall {
    pub id: String,            // 调用 ID（用于匹配结果）
    pub name: String,          // 工具名称
    pub arguments: Value,      // 参数（已解析为 JSON）
}
```

关键设计（注释中解释）：
- **重试逻辑**：指数退避重试 429/5xx，借鉴 CoreCoder `_call_with_retry`
- **非流式**：教学版先用非流式（简化 100+ 行 SSE 解析逻辑），注释中说明流式版可参考 pigs-llm
- **arguments 已解析**：在 LLM 层就 `serde_json::from_str` 解析参数 JSON，下游工具直接拿 `Value`——来自 CoreCoder 的设计
- **环境变量**：`OPENAI_API_KEY` / `OPENAI_BASE_URL` / `OPENAI_MODEL`（或直接构造）

### 4.5 `prompt.rs` — 系统提示词（约 80 行）

借鉴 CoreCoder 的 `prompt.py`（33 行动态拼接）：

```rust
/// 构建系统提示词 —— 动态拼接工具列表
pub fn build_system_prompt(tools: &ToolRegistry) -> String {
    // 1. 身份介绍
    // 2. 环境信息（cwd, OS）
    // 3. 工具清单（遍历 tools 生成 markdown 列表）
    // 4. 行为规则（读后再改、验证、简洁等）
}
```

注释中强调：**"改这 80 行里的一行就能看到 Agent 性格变化"**——来自 CoreCoder README 的教学理念。

### 4.6 `agent.rs` — Agent 循环核心（约 300 行）

**这是整个 crate 的心脏。** 借鉴 CoreCoder 的 `agent.py` 12 行骨架 + claw-analog 的 Rust 循环：

```rust
/// 通用 AI Agent —— 核心循环
/// 
/// 这是整个 crate 的核心。Agent 的工作模式很简单：
/// 1. 用户发消息 → 加入历史
/// 2. 把历史 + 工具定义发给 LLM
/// 3. LLM 回复了工具调用？→ 执行工具，结果加入历史，回到步骤 2
/// 4. LLM 回复了纯文本？→ 返回给用户，完成
pub struct Agent {
    llm: LlmClient,              // LLM 客户端
    tools: ToolRegistry,         // 工具注册表
    messages: Vec<Message>,      // 对话历史
    system_prompt: String,       // 系统提示词
    max_rounds: u32,             // 最大循环轮次（防止跑飞）
}

impl Agent {
    /// 聊天 —— 处理一条用户消息，可能经历多轮 LLM+工具循环
    pub async fn chat(&mut self, user_input: &str) -> Result<String> {
        // 1. 用户消息加入历史
        self.messages.push(Message::user(user_input));
        
        // 2. 有界循环 —— 防止 Agent 无限循环
        for round in 0..self.max_rounds {
            // 2.1 调用 LLM
            let response = self.llm.chat(&self.messages, &self.tool_schemas()).await?;
            
            // 2.2 没有工具调用 → LLM 完成了任务，返回文本
            if response.tool_calls.is_empty() {
                self.messages.push(Message::assistant_text(&response.content));
                return Ok(response.content);
            }
            
            // 2.3 有工具调用 → 先把 assistant 消息加入历史
            self.messages.push(Message::assistant_with_tools(
                &response.content, &response.tool_calls
            ));
            
            // 2.4 逐个执行工具
            for tc in &response.tool_calls {
                let result = self.tools.execute(&tc.name, &tc.arguments).await;
                self.messages.push(Message::tool_result(&tc.id, &result));
            }
            // 2.5 循环回到步骤 2.1，带着工具结果再次调用 LLM
        }
        
        // 3. 达到最大轮次
        Err(MiniAgentError::MaxRoundsReached)
    }
}
```

循环的每一行都有中文注释，解释**为什么**这么写。关键注释点：
- **为什么有界循环**：`max_rounds` 防止 Agent 无限调用工具——来自 CoreCoder `max_rounds=50`
- **为什么先 push assistant 再 push tool_result**：API 要求 assistant 的 tool_calls 必须有对应的 tool 回复——来自 CoreCoder `_answer_pending_tool_calls` 的教训
- **为什么工具结果用 `role: "tool"`**：OpenAI 格式要求，注释中对比 Anthropic 的不同处理
- **工具执行错误处理**：工具失败时返回错误字符串而非 panic，让 LLM 能看到错误并决定下一步

### 4.7 `tools/` — 内置工具实现（约 530 行）

借鉴 CoreCoder 的 7 个工具，精选 4 个最核心的：

| 工具 | 借鉴来源 | 行数 | 关键设计 |
|---|---|---|---|
| `bash.rs` | CoreCoder `bash.py` + pigs-tools `bash.rs` | 120 | 跨平台命令执行，超时控制，输出截断 |
| `read_file.rs` | CoreCoder `read.py` | 100 | 行号格式 `{n}\t{line}`，offset/limit |
| `write_file.rs` | CoreCoder `write.py` | 80 | 自动创建父目录 |
| `edit_file.rs` | CoreCoder `edit.py` | 150 | **唯一匹配 search-and-replace**（CoreCoder 的核心创新） |

`edit_file` 的注释特别强调（来自 CoreCoder README）：
> "行号是陷阱，模型数错一行就悄悄改错地方。用唯一片段锚定，失败可恢复、成功可验证。"

`tools/mod.rs` 提供 `create_default_tools()` 工厂函数，注册全部 4 个工具。

### 4.8 `examples/chat.rs` — 可运行示例（约 100 行）

一个简单的终端聊天程序，展示如何使用这个 crate：

```rust
// 教学示例：用 10 行代码启动一个终端 AI Agent
#[tokio::main]
async fn main() {
    // 1. 创建 LLM 客户端（从环境变量读取配置）
    let llm = LlmClient::from_env()?;
    // 2. 创建工具注册表（注册 4 个内置工具）
    let tools = create_default_tools();
    // 3. 创建 Agent
    let mut agent = Agent::new(llm, tools);
    // 4. REPL 循环
    loop {
        let input = read_line();  // 读取用户输入
        if input == "/quit" { break; }
        let response = agent.chat(&input).await?;  // 调用 Agent
        println!("{response}");
    }
}
```

### 4.9 `lib.rs` — 模块声明 + crate 文档（约 150 行）

crate 级文档注释（`//!`）包含：
- 这个 crate 是什么（教学版最简 Agent）
- 核心概念图解（ASCII 流程图）
- 快速开始代码示例
- 模块导航（每个模块的作用和阅读顺序）
- 设计决策说明（为什么自包含、为什么选这些工具）

### 4.10 `README.md` — 中文教学导读

包含：
- 这个 crate 的定位（"nanoGPT of Rust AI Agents"）
- 架构图
- 阅读顺序建议（`message.rs → tool.rs → llm.rs → agent.rs`）
- 与参考项目的对比表
- 如何运行示例

## 五、Workspace 集成

需要修改 `D:\AIWorkSpace\pigs\Cargo.toml`：

1. 在 `members` 数组中添加 `"crates/pigs-mini-agent"`
2. 在 `[workspace.dependencies]` 中添加 `pigs-mini-agent = { path = "crates/pigs-mini-agent" }`（虽然其他 crate 不会依赖它，但保持一致性）

注意：虽然加入了 workspace，但 `pigs-mini-agent` 的 `Cargo.toml` **不依赖任何 `pigs-*` crate**，完全自包含。

## 六、注释规范

根据用户要求"每一行每个方法都要有中文注释"：

- **每个文件**: `//!` 模块级文档，说明这个文件的职责和核心概念
- **每个 struct/enum**: `///` 文档注释，说明用途
- **每个字段**: 行内注释 `//`，说明含义
- **每个方法**: `///` 文档注释，说明功能、参数、返回值
- **关键逻辑行**: 行内注释 `//`，解释为什么这么写
- **设计决策**: 用 `// 为什么 xxx:` 格式的注释块，解释设计选择的原因和借鉴的参考项目
- **对比说明**: 在关键处用 `// 对比 CoreCoder:` / `// 对比 codex:` 说明差异

## 七、实现顺序

1. `Cargo.toml` + workspace 集成
2. `error.rs` — 基础错误类型
3. `message.rs` — 消息模型（无依赖）
4. `tool.rs` — 工具 trait + 注册表（依赖 message）
5. `prompt.rs` — 系统提示词（依赖 tool）
6. `llm.rs` — LLM 客户端（依赖 message, error）
7. `tools/` — 4 个内置工具实现（依赖 tool, error）
8. `agent.rs` — Agent 循环（依赖以上所有）
9. `lib.rs` — 模块声明 + 文档
10. `examples/chat.rs` — 可运行示例
11. `README.md` — 教学导读
12. `cargo build && cargo test && cargo clippy` 验证

## 八、与现有代码的关系

- **不冲突**: 新 crate 名为 `pigs-mini-agent`，不与任何现有 crate 重名
- **不依赖**: 不引用 `pigs-core` / `pigs-llm` / `pigs-tools` 等，完全自包含
- **平行存在**: 现有的 `pigs-cli/src/agent.rs`（439 行生产版）和新的 `pigs-mini-agent`（~1900 行教学版）平行存在，前者是实际使用的 Agent，后者是教学版
- **风格一致**: 虽然自包含，但代码风格（builder 模式、thiserror、serde）与现有 workspace 一致；唯一区别是注释语言——教学版用中文，现有代码用英文（AGENTS.md 约定代码注释用英文，但用户明确要求教学版用中文注释）