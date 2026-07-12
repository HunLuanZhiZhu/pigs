

---

## 源码拆解（`src/message.rs`）

本章对应文件约 400 行。下面按**阅读顺序**拆解每一块代码在做什么、为什么这样写。

### 1. 依赖与模块定位

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
```

- `Serialize` / `Deserialize`：消息要进对话历史、也要变成 JSON 发给 API。
- `Value`：工具参数是任意 JSON 对象，用动态 JSON 比硬编码 struct 更适合 LLM 输出。

文件顶部的 `//!` 文档已经说明：历史 = 消息列表；assistant 可同时带文本和工具调用。

### 2. `Role` —— 四种说话人

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}
```

| 属性 | 作用 |
|---|---|
| `Serialize` | 发 API 时变成 `"user"` 等字符串 |
| `rename_all = "lowercase"` | Rust 的 `User` → JSON 的 `"user"` |
| 四个变体 | 对齐 OpenAI Chat Completions 的四种 role |

`Display` 实现把角色打印成小写字符串，`to_openai_json()` 里直接用 `self.role.to_string()`。

**设计点**：选 OpenAI 的独立 `tool` 角色，而不是 Anthropic 把 tool result 塞进 `user`。教学上更清晰。

### 3. `ToolCall` —— LLM 的一次“请执行工具”

```rust
pub struct ToolCall {
    pub id: String,          // 匹配结果用
    pub name: String,        // 注册表里的工具名
    pub arguments: Value,  // 已解析的 JSON
}
```

- **为什么要 `id`**：一轮回复可以有多个 tool_calls；每个 tool 消息必须用同一个 id 回传。
- **为什么 `arguments` 是 `Value` 不是 `String`**：API 原始字段是 JSON **字符串**；在 `llm.rs` 的 `parse_llm_response` 里已经 `from_str`。下游工具直接 `.get("path")`，不必再解析。

### 4. `Message` 字段

```rust
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}
```

| 字段 | 谁用 | 含义 |
|---|---|---|
| `content` | 所有角色 | 文本；assistant 只有 tool_calls 时可为 `None` |
| `tool_calls` | 仅 assistant | LLM 请求执行的工具列表 |
| `tool_call_id` | 仅 tool | 对应哪个 `ToolCall.id` |

`skip_serializing_if`：序列化时省略 `null` 字段，避免 API 收到多余键。

### 5. 五个构造器（工厂方法）

```rust
Message::system(text)              // role=System
Message::user(text)                // role=User
Message::assistant_text(text)      // 纯文本结束
Message::assistant_with_tools(...) // 文本 + tool_calls
Message::tool_result(id, out, err) // role=Tool
```

**`assistant_with_tools` 细节**：空字符串则 `content = None`，避免序列化空 content。

**`tool_result` 细节**：`is_error` 时在文本前加 `[ERROR]`，让模型更容易看见失败。

### 6. 查询方法

```rust
pub fn has_tool_calls(&self) -> bool {
    self.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty())
}

pub fn get_tool_calls(&self) -> &[ToolCall] { /* Some → 切片，None → &[] */ }
```

Agent 用“有没有 tool_calls”决定继续还是结束。

### 7. `to_openai_json()` —— 内部类型 → API 线格式

1. 写入 `role`
2. 有 `content` → 写入
3. 有 `tool_calls` → 转成 OpenAI 格式；**注意** `arguments` 必须再序列化成 **字符串**（`tc.arguments.to_string()`）
4. 有 `tool_call_id` → tool 消息带上

### 8. 测试覆盖

构造器字段、`has_tool_calls`、错误前缀、`to_openai_json` 三种形态。

### 9. 和 Agent 循环的衔接

| 循环步骤 | Message API |
|---|---|
| 用户输入 | `Message::user` |
| LLM 纯文本结束 | `Message::assistant_text` |
| LLM 要工具 | `Message::assistant_with_tools` |
| 工具跑完 | `Message::tool_result` |
| 发给 API | 每条 `.to_openai_json()` |

读完应能回答：**对话状态如何用类型安全结构表示，并无损映射到 OpenAI 消息格式。**
