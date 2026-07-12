# 错误处理详解

> 对应源文件: [`src/error.rs`](../src/error.rs)

## 这个模块解决什么问题？

定义 crate 中所有操作可能返回的错误类型。统一的错误类型让 crate 内部的错误传递更简单——只需 `?` 操作符。

## 核心概念

### Rust 的错误处理：不靠异常

Rust 没有 `try/catch` 异常机制。错误处理靠 `Result<T, E>` 类型：

```rust
// Result 是一个枚举
enum Result<T, E> {
    Ok(T),     // 成功，包含值
    Err(E),    // 失败，包含错误
}
```

`?` 操作符是错误传播的快捷方式：

```rust
// 不用 ? 的写法
fn read_file() -> Result<String, MiniAgentError> {
    let content = match std::fs::read_to_string("file.txt") {
        Ok(c) => c,
        Err(e) => return Err(MiniAgentError::IoError(e.to_string())),
    };
    Ok(content)
}

// 用 ? 的写法（等价）
fn read_file() -> Result<String, MiniAgentError> {
    let content = std::fs::read_to_string("file.txt")?;  // 自动转换错误
    Ok(content)
}
```

`?` 之所以能自动转换，是因为我们实现了 `From<std::io::Error> for MiniAgentError`。

### 统一错误类型

```
┌──────────────────────────────────────────────────┐
│  MiniAgentError                                   │
│                                                   │
│  ├── LlmError(String)        LLM API 调用失败     │
│  ├── ToolError(String)       工具执行失败          │
│  ├── NetworkError(String)    网络请求失败          │
│  ├── ParseError(String)      JSON 解析失败         │
│  ├── IoError(String)         文件 IO 失败          │
│  ├── MaxRoundsReached(u32)   达到最大轮次          │
│  └── ConfigError(String)     配置错误              │
│                                                   │
│  From<std::io::Error>     自动转换 IO 错误         │
│  From<reqwest::Error>     自动转换网络错误         │
│  From<serde_json::Error>  自动转换 JSON 错误       │
│                                                   │
│  type Result<T> = std::result::Result<T, MiniAgentError> │
└──────────────────────────────────────────────────┘
```

## 错误类型详解

### LlmError vs NetworkError

| 错误 | 层级 | 触发场景 |
|---|---|---|
| `NetworkError` | 底层（reqwest） | DNS 解析失败、连接超时、TCP 错误 |
| `LlmError` | API 层 | HTTP 200 但返回错误 JSON、HTTP 4xx/5xx |

### MaxRoundsReached

这不是"错误"而是"保护"——Agent 循环了 `max_rounds` 次还没完成，可能是陷入了循环。包含轮次数字，方便诊断。

### ToolError 的特殊性

工具执行失败有两种处理方式：
1. **硬错误**（返回 `Err`）：工具根本无法执行（如找不到工具）
2. **软错误**（返回 `Ok` 但标记 `is_error`）：工具执行了但结果不理想（如命令返回非零退出码）

软错误的错误信息会作为工具结果返回给 LLM，让 LLM 看到错误并决定下一步。

## 关键设计决策

### 为什么用一个统一类型而非多个？

```rust
// 借鉴 pigs-core: 分成 ApiError + ToolError 两个类型
// 本 crate: 合并为一个 MiniAgentError
```

教学目的——一个统一类型让错误传递更简单。`?` 操作符只需要一个 `From` 实现就能自动转换。

### 为什么用 `thiserror`？

`thiserror` crate 自动为 enum 实现 `Display` 和 `Error` trait。只需要写 `#[error("...")]` 属性：

```rust
#[derive(Debug, Error)]
pub enum MiniAgentError {
    #[error("LLM 调用失败: {0}")]
    LlmError(String),
    // ...
}
```

不用 `thiserror` 的话，需要手动实现几十行 `Display` 和 `Error` trait。

### 为什么定义 `type Result<T>`？

```rust
pub type Result<T> = std::result::Result<T, MiniAgentError>;
```

这是 Rust 的惯用模式——让函数签名更简洁。`fn foo() -> Result<String>` 自动意味着 `Result<String, MiniAgentError>`。

## From 实现详解

```rust
// std::io::Error → MiniAgentError::IoError
impl From<std::io::Error> for MiniAgentError { ... }

// reqwest::Error → MiniAgentError::NetworkError
impl From<reqwest::Error> for MiniAgentError { ... }

// serde_json::Error → MiniAgentError::ParseError
impl From<serde_json::Error> for MiniAgentError { ... }
```

每个 `From` 实现都让 `?` 操作符能自动转换对应类型的错误。这是 Rust 错误转换的核心机制。

## 借鉴对比

| 项目 | 错误类型 | 特点 |
|---|---|---|
| CoreCoder (Python) | 字符串 + try/except | 无类型安全 |
| pigs-core | `ApiError` + `ToolError` 两个类型 | 更精确但更复杂 |
| codex | `CodexErr` enum（20+ 变体） | 覆盖所有场景 |
| **本 crate** | `MiniAgentError`（7 变体） | 统一类型，简化教学 |

## 测试覆盖

- IO 错误自动转换
- JSON 错误自动转换
- 错误 Display 包含原始信息
- MaxRoundsReached 的轮次信息


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
