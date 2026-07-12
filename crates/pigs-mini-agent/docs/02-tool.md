# 工具系统详解

> 对应源文件: [`src/tool.rs`](../src/tool.rs)

## 这个模块解决什么问题？

工具是 Agent "动手"的能力——没有工具，Agent 只能聊天。这个模块定义了"所有工具长什么样"的统一接口（`Tool` trait）和管理工具的注册表（`ToolRegistry`）。

## 核心概念

### Tool trait

每个工具必须实现 4 个方法：

```
┌──────────────────────────────────────────────────┐
│  Tool trait（工具接口）                            │
│                                                   │
│  fn name(&self) -> &str           工具名          │
│  fn description(&self) -> &str    工具描述        │
│  fn parameters(&self) -> Value    参数 schema     │
│  async fn execute(&self, input)   执行工具        │
│                                                   │
│  fn schema(&self) -> Value        默认实现：      │
│                                    生成 OpenAI    │
│                                    function 格式  │
└──────────────────────────────────────────────────┘
```

### ToolRegistry

```
┌──────────────────────────────────────────────────┐
│  ToolRegistry（工具注册表）                        │
│                                                   │
│  tools: HashMap<String, Box<dyn Tool>>            │
│                                                   │
│  ┌─────────┐  ┌────────────┐  ┌───────────┐      │
│  │  bash   │  │ read_file  │  │ write_file│      │
│  └─────────┘  └────────────┘  └───────────┘      │
│  ┌──────────┐                                    │
│  │ edit_file│  ... 可以注册任意多个工具             │
│  └──────────┘                                    │
│                                                   │
│  方法：                                           │
│  - register(tool)  注册工具                       │
│  - execute(name, input)  按名执行                 │
│  - schemas()  获取所有工具的 schema（发给 LLM）    │
│  - has(name) / names() / len() / is_empty()       │
└──────────────────────────────────────────────────┘
```

## 关键设计决策

### 为什么 `execute` 返回 `String`？

来自 CoreCoder 的设计——"所有工具都返回字符串，让消息拼接永远是 str"。

这样 Agent 循环里处理工具结果时不用做类型匹配，简单直接。如果工具需要返回结构化数据，自己在 `execute` 内部序列化为字符串即可。

### 为什么用 `Box<dyn Tool>`？

`Box<dyn Tool>` 是 Rust 的 trait object——不同的工具（`ReadFileTool`、`BashTool` 等）可以存到同一个 `HashMap` 里。这是 Rust 实现"多态"的方式，类似于 Python 里存不同类的对象到同一个 dict。

### 为什么用 `#[async_trait]`？

Rust 的 trait 不支持直接定义 `async fn`（因为 async fn 的返回类型是匿名的 `impl Future`）。`async_trait` 宏通过 `Pin<Box<dyn Future>>` 绕过这个限制。

### 为什么 `schema()` 是默认实现？

`schema()` 自动生成 `{"type":"function","function":{"name","description","parameters"}}` 格式。这个格式是 OpenAI function-calling 标准，所有子类都不需要重写。

## 添加新工具的步骤

```rust
use async_trait::async_trait;
use pigs_mini_agent::{Tool, Result};
use serde_json::Value;

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "我的工具的描述" }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "输入文本" }
            },
            "required": ["input"]
        })
    }
    async fn execute(&self, input: Value) -> Result<String> {
        let text = input["input"].as_str().unwrap_or("");
        Ok(format!("处理结果: {text}"))
    }
}

// 注册到 Agent
let mut tools = ToolRegistry::new();
tools.register(Box::new(MyTool));
```

## 借鉴对比

| 项目 | 工具抽象 | 区别 |
|---|---|---|
| CoreCoder (Python) | `Tool(ABC)` + 3 属性 + `execute(**kwargs)` | 无类型，用 dict 传参 |
| pigs-core | `ToolHandler` trait + `Pin<Box<dyn Future>>` | 手动装箱 Future，更底层 |
| claw-analog | 不用 trait——一个大 `match` 分发函数 | 简单但不灵活 |
| **本 crate** | `Tool` trait + `#[async_trait]` | 教学版，用宏简化异步 |

## 测试覆盖

- 注册和执行工具（用 `EchoTool` 测试桩）
- 执行不存在的工具返回错误
- 获取工具 schema 列表
- 获取工具名称列表


---

## 源码拆解（`src/tool.rs`）

### 1. 依赖

```rust
use std::collections::HashMap;
use async_trait::async_trait;
use serde_json::Value;
use crate::error::{MiniAgentError, Result};
```

- `HashMap`：名字 → 工具实例
- `async_trait`：trait 里写 `async fn execute`
- `Value`：LLM 传来的 JSON 参数

### 2. `Tool` trait 逐方法

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, input: Value) -> Result<String>;
    fn schema(&self) -> Value { /* 默认实现 */ }
}
```

| 约束 / 方法 | 含义 |
|---|---|
| `Send + Sync` | 可跨线程共享；异步环境需要 |
| `name` | 唯一蛇形名，出现在 prompt 与 tool_calls |
| `description` | 模型靠它决定何时调用 |
| `parameters` | JSON Schema，描述参数形状 |
| `execute` | 真正干活；返回 **字符串** 结果 |
| `schema` | 默认拼成 OpenAI function-calling 外层格式 |

**为什么 `execute` 返回 `String`？**  
CoreCoder 哲学：消息拼接永远是文本。结构化数据自己序列化成字符串即可。

**`schema` 默认实现** 固定包装：

```json
{ "type": "function", "function": { "name", "description", "parameters" } }
```

### 3. `ToolRegistry`

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}
```

`Box<dyn Tool>` = trait object：BashTool、ReadFileTool 等不同具体类型放进同一张表。

#### `register`

```rust
pub fn register(&mut self, tool: Box<dyn Tool>) {
    let name = tool.name().to_string();
    self.tools.insert(name, tool);
}
```

先取名再 `insert`（`insert` 会 move `tool`）。同名后注册覆盖前者。

#### `execute` —— 分发

```rust
let tool = self.tools.get(name)
    .ok_or_else(|| MiniAgentError::ToolError(format!("未知的工具: '{name}'")))?;
tool.execute(input).await
```

模型幻觉不存在的工具名 → `ToolError`；Agent 把错误字符串塞回历史。

#### 其它方法

| 方法 | 用途 |
|---|---|
| `has` | 是否注册 |
| `schemas` | 全部 schema → 发给 LLM |
| `names` | 工具名 → 拼 prompt |
| `len` / `is_empty` | 调试与测试 |
| `Default` | 等价于 `new()` |

### 4. 测试桩 `EchoTool`

覆盖：注册执行、未知工具、schemas 结构、names 列表。

### 5. 和 Agent 的衔接

```text
create_default_tools() → ToolRegistry
Agent::new(..., tools)
每轮 tool_schemas() → llm.chat
有 tool_calls → tools.execute(name, args)
```

读完应能自己实现一个 `Tool` 并 `register`。
