# Agent 循环详解

> 对应源文件: [`src/agent.rs`](../src/agent.rs)
>
> **这是整个 crate 的心脏。**

## 这个模块解决什么问题？

实现了 Agent 的核心循环：调 LLM → 有工具调用？执行工具，继续循环 : 返回文本，结束。

所有"智能"来自 LLM，Agent 代码只是忠实地执行循环——把历史发给 LLM，看 LLM 要干什么，帮 LLM 执行工具，把结果告诉 LLM。

## 核心循环

### 12 行骨架

借鉴 CoreCoder `agent.py` 的 12 行骨架：

```python
# CoreCoder 的 Python 版
for _ in range(self.max_rounds):
    reply = self.llm.chat(self.messages, self.tools)
    if not reply.tool_calls:
        return reply.text
    results = run_parallel(reply.tool_calls)
    self.messages += results
return "(hit the round limit)"
```

```rust
// 本 crate 的 Rust 版
for round in 0..self.max_rounds {
    let response = self.llm.chat(&messages, &tool_schemas).await?;
    if response.tool_calls.is_empty() {
        self.messages.push(Message::assistant_text(&response.content));
        return Ok(response.content);
    }
    self.messages.push(Message::assistant_with_tools(&response.content, response.tool_calls.clone()));
    for tc in &response.tool_calls {
        let result = self.tools.execute(&tc.name, tc.arguments.clone()).await;
        self.messages.push(Message::tool_result(&tc.id, &output, is_error));
    }
}
return Err(MiniAgentError::MaxRoundsReached(self.max_rounds));
```

### 循环图示

```
用户输入 "帮我创建一个 hello.rs 文件"
    │
    ▼
[1] 用户消息加入历史
    │
    ▼
[2] 调用 LLM（历史 + 工具定义）
    │
    ▼
[3] LLM 回复了什么？
    │
    ├── 纯文本（无工具调用）──→ [4] 返回文本给用户 ✓ 结束
    │
    └── 工具调用（如 write_file）
            │
            ▼
        [5] 把 assistant 消息（含工具调用）加入历史
            │
            ▼
        [6] 执行工具，把结果加入历史
            │
            ▼
        [7] 回到 [2]，带着工具结果再次调用 LLM ↺ 循环
```

## 关键设计决策

### 为什么有界循环（`for` 而非 `loop`）？

Agent 可能在某些场景下陷入循环："调用工具 → LLM 再次要求调用工具 → 调用工具 → ..."

`max_rounds`（默认 50，借鉴 CoreCoder）作为安全阀，超过后强制停止。这不是"错误"而是"保护"——就像 while 循环要避免无限循环一样。

### 为什么先 push assistant 再 push tool_result？

```rust
// ✅ 正确顺序
self.messages.push(Message::assistant_with_tools(...));  // 先
self.messages.push(Message::tool_result(...));           // 后
```

OpenAI API 要求 assistant 的 tool_calls 必须有对应的 tool 回复。如果顺序搞反了，API 会拒绝请求。

这个教训来自 CoreCoder 的 `_answer_pending_tool_calls` 方法——当中断时，需要给所有没回上话的 tool_call 补一条 `"[interrupted]"` 回复，否则下次请求会报错。

### 工具失败时为什么不 panic？

```rust
let (output, is_error) = match result {
    Ok(output) => (output, false),
    Err(e) => (e.to_string(), true),  // 错误信息作为工具结果返回
};
```

工具失败时把错误信息作为工具结果返回给 LLM。这样 LLM 能看到错误并决定下一步（比如换一种方式重试）。这与 CoreCoder 的 `_exec_tool` 设计一致。

### 为什么每次都发送完整历史？

LLM 没有记忆——它靠历史消息"回忆"之前发生了什么。每次调用都发送从开始到现在的完整历史。

后果：历史越长，请求越慢越贵（token 更多）。生产级 Agent（如 codex）有自动压缩（auto-compact）机制，本教学版不做。

## Agent 结构体

```rust
pub struct Agent {
    pub llm: LlmClient,              // LLM 客户端（"大脑"）
    pub tools: ToolRegistry,         // 工具注册表（"手脚"）
    pub messages: Vec<Message>,      // 对话历史
    pub system_prompt: String,       // 系统提示词
    pub max_rounds: u32,             // 最大循环轮次
}
```

### Agent 的"分工"

```
┌───────────────────────────────────────────────────┐
│  Agent 结构体                                      │
│                                                    │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐ │
│  │   LLM    │  │  Tools   │  │    Messages      │ │
│  │ (大脑)   │  │ (手脚)   │  │  (记忆/历史)     │ │
│  │          │  │          │  │                   │ │
│  │ 做决策   │  │ 执行命令 │  │  完整对话记录     │ │
│  │ 推理     │  │ 读写文件 │  │  (每次全发给LLM) │ │
│  └──────────┘  └──────────┘  └──────────────────┘ │
│                                                    │
│  系统提示词 → 定义行为规则                          │
│  max_rounds → 防止无限循环                          │
└───────────────────────────────────────────────────┘
```

## API 速览

### 创建 Agent

```rust
let llm = LlmClient::from_env()?;
let tools = create_default_tools();
let mut agent = Agent::new(llm, tools);

// 可选：自定义配置
let agent = Agent::new(llm, tools)
    .with_max_rounds(30)                    // 自定义最大轮次
    .with_system_prompt("你是一个测试 Agent"); // 自定义提示词
```

### 聊天

```rust
let response = agent.chat("帮我创建一个 hello.txt 文件").await?;
println!("{response}");
```

### 其他方法

| 方法 | 用途 |
|---|---|
| `agent.reset()` | 清空对话历史（相当于新对话） |
| `agent.history_len()` | 获取历史消息数 |

## 借鉴对比

| 项目 | 循环位置 | 行数 | 特性 |
|---|---|---|---|
| CoreCoder (Python) | `Agent.chat()` | 50 | 并行工具执行、上下文压缩 |
| claw-analog (Rust) | `run()` 函数 | 200 | 流式、权限、NDJSON 输出 |
| codex (Rust) | `run_turn()` | 300+ | pending input、hooks、cancellation |
| pigs (Rust) | `Agent::run_turn()` | 439 | 流式打印、权限、会话持久化 |
| **本 crate** | `Agent::chat()` | ~100 | 纯循环，无额外复杂度 |

本 crate 是所有版本中最简的——只保留循环本质，其他（流式、权限、压缩、持久化）都去掉了。

## 测试覆盖

- Agent 创建和初始状态
- builder 模式（`with_max_rounds` / `with_system_prompt`）
- 重置历史
- `full_messages` 包含系统提示词

> 注意：`chat()` 的完整测试需要 mock LLM 服务器，这里只测试不需要网络的功能。


---

## 源码拆解（`src/agent.rs`）

### 1. 结构体

```rust
pub struct Agent {
    pub llm: LlmClient,
    pub tools: ToolRegistry,
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub max_rounds: u32,
}
```

五个字段全 `pub`，方便教学调试。

### 2. `new`

```rust
let system_prompt = build_system_prompt(&tools);
Agent { llm, tools, messages: Vec::new(), system_prompt, max_rounds: 50 }
```

建 Agent 时按**当前**工具表生成 prompt。之后改 tools 默认不重算（可用 `with_system_prompt`）。

### 3. Builder

`with_max_rounds` / `with_system_prompt`：消费 `self` 返回 `Self`，支持链式调用。

### 4. 私有辅助

**`full_messages`**：`system` + `messages.clone()`。system **不**存在 `self.messages` 里。

**`tool_schemas`**：`self.tools.schemas()` 转发。

### 5. `chat` 逐步

#### 步骤 1：入历史

```rust
self.messages.push(Message::user(user_input));
```

#### 步骤 2：有界 for

```rust
for round in 0..self.max_rounds {
```

#### 步骤 2.1：调模型

```rust
let response = self.llm.chat(&self.full_messages(), &self.tool_schemas()).await?;
```

#### 步骤 2.2：结束分支

```rust
if response.tool_calls.is_empty() {
    self.messages.push(Message::assistant_text(&response.content));
    return Ok(response.content);
}
```

#### 步骤 2.3：assistant + tools 入历史（必须先于 tool 结果）

```rust
self.messages.push(Message::assistant_with_tools(
    &response.content,
    response.tool_calls.clone(),
));
```

#### 步骤 2.4：顺序执行工具

```rust
for tc in &response.tool_calls {
    let result = self.tools.execute(&tc.name, tc.arguments.clone()).await;
    let (output, is_error) = match result {
        Ok(o) => (o, false),
        Err(e) => (e.to_string(), true), // 不中断 chat
    };
    self.messages.push(Message::tool_result(&tc.id, &output, is_error));
}
```

#### 步骤 2.5：循环体结束 → 下一轮再 `llm.chat`

#### 步骤 3：打满轮次

```rust
Err(MiniAgentError::MaxRoundsReached(self.max_rounds))
```

### 6. `reset` / `history_len`

- `reset`：清空 `messages`，保留 system_prompt  
- `history_len`：只计 `messages`，**不含** system  

### 7. 测试范围

不测真实网络 `chat`；测创建、builder、reset、full_messages。

### 8. 心智模型

```text
user push
  → [for]
      full_messages + schemas → LLM
      empty tools? → assistant_text + return
      else assistant_with_tools
           for each tool → execute → tool_result
  → MaxRoundsReached
```

读完应能**默写** `chat` 分支，并说明为何 assistant 必须先于 tool 入历史。
