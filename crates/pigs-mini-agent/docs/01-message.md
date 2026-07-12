# 消息模型详解

> 对应源文件: [`src/message.rs`](../src/message.rs)

## 这个模块解决什么问题？

AI Agent 的对话历史本质就是一个消息列表。这个模块定义了消息的数据结构——"谁说了什么"以及"消息里有什么"。

## 核心概念

### 三种数据类型

```
┌─────────────────────────────────────────────────┐
│  Role (角色)                                     │
│  ├── System     系统提示词（设定 Agent 行为）      │
│  ├── User       用户输入                          │
│  ├── Assistant  LLM 回复                         │
│  └── Tool       工具执行结果                      │
│                                                   │
│  ToolCall (工具调用)                              │
│  ├── id          唯一标识（用于匹配结果）           │
│  ├── name        工具名                           │
│  └── arguments   参数（已解析为 JSON Value）       │
│                                                   │
│  Message (消息)                                   │
│  ├── role            消息角色                      │
│  ├── content         文本内容                     │
│  ├── tool_calls      工具调用列表（仅 assistant）   │
│  └── tool_call_id    关联的工具调用 ID（仅 tool）   │
└─────────────────────────────────────────────────┘
```

### 消息在对话中的流转

```
用户输入 → [User 消息] → 送给 LLM
                            ↓
                 LLM 返回 [Assistant 消息（含工具调用）]
                            ↓
                 执行工具 → [Tool 消息（工具结果）]
                            ↓
                 再送给 LLM → 可能再次调用工具...
                            ↓
                 LLM 返回 [Assistant 消息（纯文本）] → 返回给用户
```

## 关键设计决策

### 为什么用 `Option<String>` 而非 `String` 存内容？

assistant 消息可以只有工具调用、没有文本。用 `Option` 表示"可能没有文本"。
同时配合 `#[serde(skip_serializing_if = "Option::is_none")]`，序列化时不输出空字段。

### 为什么 `arguments` 是 `Value` 而非 `String`？

OpenAI API 返回的 `arguments` 是 JSON **字符串**（如 `"{\"path\":\"/tmp\"}"`）。
在 LLM 客户端层（`llm.rs`）就做 `serde_json::from_str` 解析为 `Value`，下游工具直接拿 `Value` 用，不用重复解析——这是 CoreCoder 的设计。

### 为什么 ToolResult 用独立角色 `role: "tool"`？

OpenAI 和 Anthropic 对工具结果的处理方式不同：
- **OpenAI**: 用 `role: "tool"` 作为独立角色
- **Anthropic**: 把工具结果放到 `role: "user"` 的消息里

本 crate 选 OpenAI 格式，因为更直观——工具结果就是一种独立的消息。

## API 速览

### `Role` 枚举

```rust
pub enum Role { System, User, Assistant, Tool }
```

四种角色，通过 `#[serde(rename_all = "lowercase")]` 序列化为小写字符串。

### `Message` 的构造器

| 方法 | 用途 |
|---|---|
| `Message::system(text)` | 创建系统消息 |
| `Message::user(text)` | 创建用户消息 |
| `Message::assistant_text(text)` | 创建纯文本 assistant 消息 |
| `Message::assistant_with_tools(text, calls)` | 创建含工具调用的 assistant 消息 |
| `Message::tool_result(id, output, is_error)` | 创建工具结果消息 |

### `Message` 的方法

| 方法 | 用途 |
|---|---|
| `has_tool_calls()` | 判断是否包含工具调用 |
| `get_tool_calls()` | 获取工具调用列表 |
| `to_openai_json()` | 序列化为 OpenAI API 格式 |

## 借鉴对比

| 项目 | 消息表示 | 区别 |
|---|---|---|
| CoreCoder (Python) | 裸 dict `{"role": "user", "content": "..."}` | 无类型安全 |
| pigs-core | `ContentBlock` enum + `Message` | 类型安全，但更复杂 |
| codex | `ResponseItem` enum（20+ 变体） | 覆盖所有场景，过于庞大 |
| **本 crate** | `Message` struct + `Option` 字段 | 简化版，足够展示核心循环 |

## 测试覆盖

- 创建各种类型的消息
- 工具调用提取
- 错误标记
- OpenAI JSON 序列化


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
