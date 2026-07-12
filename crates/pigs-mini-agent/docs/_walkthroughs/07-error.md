

---

## 源码拆解（`src/error.rs`）

### 1. thiserror 枚举

```rust
#[derive(Debug, Error)]
pub enum MiniAgentError {
    #[error("LLM 调用失败: {0}")]
    LlmError(String),
    // ...
    #[error("Agent 达到最大轮次限制 ({0} 轮)，强制停止")]
    MaxRoundsReached(u32),
}
```

- `#[error("...")]`：自动 `Display`  
- `{0}`：元组变体字段  
- `Debug + Error`：可在 `Result` 里 `?`  

### 2. 七个变体对照调用点

| 变体 | 典型产生位置 |
|---|---|
| `LlmError` | `call_with_retry` HTTP 4xx / 业务失败 |
| `ToolError` | 缺参、未知工具、文件操作失败 |
| `NetworkError` | reqwest 连接失败耗尽重试 |
| `ParseError` | 响应 JSON / From serde_json |
| `IoError` | From `std::io::Error` |
| `MaxRoundsReached` | `Agent::chat` 打满轮次 |
| `ConfigError` | 缺 `OPENAI_API_KEY` |

### 3. 三个 `From`

```rust
impl From<std::io::Error> for MiniAgentError { /* IoError */ }
impl From<reqwest::Error> for MiniAgentError { /* NetworkError */ }
impl From<serde_json::Error> for MiniAgentError { /* ParseError */ }
```

工具层常 **故意** `map_err` 成更具体的 `ToolError("读取文件失败...")`，对模型更友好，而不是裸 `IoError`。

### 4. 类型别名

```rust
pub type Result<T> = std::result::Result<T, MiniAgentError>;
```

### 5. 硬错误 vs 软错误

| 类型 | 表现 | 例子 |
|---|---|---|
| 硬错误 | `Err(...)` 冒泡 | 未知工具、超时、API 401 |
| 软错误 | `Ok` 文本或 `is_error=true` | bash 非零退出 |

`Agent::chat` 对 `tools.execute` 的 `Err` 会转成字符串 + `is_error`，**不**因单工具失败中止整次 chat；`llm.chat` 失败才会冒泡。

### 6. 测试

`From` 转换、`Display` 文案、`MaxRoundsReached` 含轮次数。

读完应能说明：**何时 `?` 冒泡，何时把错误变成 tool 消息字符串。**
