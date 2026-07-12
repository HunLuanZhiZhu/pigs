

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
