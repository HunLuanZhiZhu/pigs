# LLM 客户端详解

> 对应源文件: [`src/llm.rs`](../src/llm.rs)

## 这个模块解决什么问题？

LLM 客户端负责把对话历史发给大模型 API，获取 LLM 的回复。回复可能是纯文本（LLM 完成任务了），也可能包含工具调用（LLM 要用工具）。

这是整个 crate 中**唯一与外部服务交互**的地方。

## 核心概念

### 数据流

```
┌──────────┐     HTTP POST      ┌──────────────┐
│ LlmClient│ ─────────────────→ │ OpenAI 兼容  │
│          │  /chat/completions │ API 服务器   │
│  chat()  │ ←───────────────── │              │
│          │     JSON 响应       └──────────────┘
└──────────┘
     ↓
┌──────────────────┐
│ LlmResponse      │
│  - content       │  文本回复
│  - tool_calls[]  │  工具调用列表
└──────────────────┘
```

### 支持的供应商

所有兼容 OpenAI Chat Completions API 的供应商：

| 供应商 | base_url | model 示例 |
|---|---|---|
| OpenAI | `https://api.openai.com/v1` | `gpt-4o` |
| DeepSeek | `https://api.deepseek.com/v1` | `deepseek-chat` |
| Qwen (通义千问) | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen-plus` |
| Kimi (月之暗面) | `https://api.moonshot.cn/v1` | `moonshot-v1-8k` |
| Ollama (本地) | `http://localhost:11434/v1` | `llama3` |

## 关键设计决策

### 为什么用非流式而非流式？

流式响应（SSE）的难点在于——LLM 返回的 tool_calls 参数是**碎片化**的：每个工具的 arguments JSON 被分成多个 delta 片段，需要按 index 累积拼接。

CoreCoder 的 `llm.py` 用了 30+ 行处理这个，pigs-llm 用了更多。教学版用非流式——API 一次性返回完整的 tool_calls，简单直接。

如果需要流式实现，可以参考：
- `pigs-llm` crate 的 `openai.rs`（636 行，含完整 SSE 解析）
- `claw-code` 的 `stream_to_message_response()` 函数

### 为什么有重试逻辑？

网络请求可能因各种原因失败。重试策略借鉴 CoreCoder 的 `_call_with_retry`：

| 错误类型 | HTTP 状态码 | 是否重试 | 原因 |
|---|---|---|---|
| 限流 | 429 | ✅ | 等一会就好 |
| 服务器错误 | 5xx | ✅ | 临时故障 |
| 网络错误 | 无 | ✅ | DNS/连接问题 |
| 客户端错误 | 4xx（非429） | ❌ | 请求本身有问题 |

重试使用**指数退避**：1秒 → 2秒 → 4秒，最多重试 3 次。

### 为什么在 LLM 层就解析 arguments？

OpenAI API 返回的 `arguments` 是 JSON **字符串**（如 `"{\"path\":\"/tmp\"}"`），不是 JSON 对象。

在 `parse_llm_response()` 中就做 `serde_json::from_str` 解析，下游工具直接拿 `Value` 用。解析失败时降级为空对象 `{}`——让下游工具处理参数缺失，而非在 LLM 层报错。这是 CoreCoder 的设计决策。

### 为什么用环境变量配置？

遵循 12-Factor App 原则，也是 CoreCoder 和几乎所有 CLI 工具的惯例：

| 环境变量 | 必需 | 默认值 | 说明 |
|---|---|---|---|
| `OPENAI_API_KEY` | ✅ | 无 | API 密钥 |
| `OPENAI_BASE_URL` | ❌ | `https://api.openai.com/v1` | API 地址 |
| `OPENAI_MODEL` | ❌ | `gpt-4o` | 模型名 |

## API 速览

### `LlmClient`

```rust
// 从环境变量创建
let llm = LlmClient::from_env()?;

// 手动创建
let llm = LlmClient::new("api-key", "https://api.example.com/v1", "model-name");

// 获取模型名
let model = llm.model();

// 发送聊天请求
let response = llm.chat(&messages, &tool_schemas).await?;
```

### `LlmResponse`

```rust
pub struct LlmResponse {
    pub content: String,           // 文本回复
    pub tool_calls: Vec<ToolCall>, // 工具调用列表
}
```

Agent 循环根据这个返回值决定下一步：
- `tool_calls` 为空 → LLM 完成任务，返回文本
- `tool_calls` 不为空 → 执行工具，继续循环

## 借鉴对比

| 项目 | 行数 | 流式 | 重试 | 多供应商 |
|---|---|---|---|---|
| CoreCoder `llm.py` | 336 | ✅ SSE | ✅ 指数退避 | OpenAI 兼容 + LiteLLM |
| pigs-llm `openai.rs` | 636 | ✅ SSE | ✅ | OpenAI + Anthropic |
| claw-analog | ~400 | ✅ SSE | ✅ | Anthropic + OpenAI 兼容 |
| **本 crate** | ~310 | ❌ 非流式 | ✅ 指数退避 | OpenAI 兼容 |

## 测试覆盖

- 解析纯文本回复
- 解析含工具调用的回复
- 解析参数格式错误的情况（降级为空对象）
- 解析缺少 choices 的无效响应
- 从环境变量创建客户端（无 key / 有 key / 有 URL+Model）
- 手动创建客户端


---

## 源码拆解（`src/llm.rs`）

### 1. 结构体字段

```rust
pub struct LlmClient {
    http: reqwest::Client,  // 120s 超时
    api_key: String,
    base_url: String,
    model: String,
}
```

字段私有；只能通过 `new` / `from_env` 构造。

### 2. `new`

```rust
reqwest::Client::builder()
    .timeout(Duration::from_secs(120))
    .build()
    .unwrap_or_else(|_| reqwest::Client::new())
```

LLM 可能很慢 → 120 秒超时。参数 `impl Into<String>` 可接 `&str` / `String`。

### 3. `from_env`

| 变量 | 必需 | 默认 |
|---|---|---|
| `OPENAI_API_KEY` | 是 | 无 → `ConfigError` |
| `OPENAI_BASE_URL` | 否 | `https://api.openai.com/v1` |
| `OPENAI_MODEL` | 否 | `gpt-4o` |

### 4. `chat` 主路径

```rust
// 1) Message → OpenAI JSON
// 2) body = { model, messages }
// 3) 有 tools → tools + tool_choice: "auto"
// 4) call_with_retry
// 5) parse_llm_response
```

### 5. `call_with_retry`

```rust
max_retries = 3; wait_secs = 1;
// POST {base}/chat/completions
// Authorization: Bearer {key}
```

| 情况 | 行为 |
|---|---|
| 2xx | 解析 JSON 返回 |
| 429 或 5xx 且可重试 | sleep 后 `wait_secs *= 2` |
| 其它 4xx | 立即 `LlmError` |
| 网络错误可重试 | 同上退避 |
| 网络错误用尽 | `NetworkError` |

退避：1s → 2s → 4s。

### 6. `LlmResponse`

```rust
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}
```

Agent 只看 `tool_calls.is_empty()`。

### 7. `parse_llm_response`

路径：`choices[0].message` → content + tool_calls。

```rust
let arguments: Value = serde_json::from_str(arguments_str)
    .unwrap_or_else(|_| json!({}));
```

- 缺字段用 `"unknown"` / `""` / `{}` 兜底  
- 非法 JSON 参数 → 空对象，不整请求失败  

### 8. 测试

纯文本 / tool_calls / 坏 arguments / 缺 choices；`from_env` 串行；`new` 字段。

读完应能画出：**Message → JSON body → HTTP → JSON → LlmResponse**。
