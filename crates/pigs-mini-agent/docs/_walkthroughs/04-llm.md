

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
