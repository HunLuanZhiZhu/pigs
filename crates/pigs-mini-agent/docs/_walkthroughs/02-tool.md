

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
