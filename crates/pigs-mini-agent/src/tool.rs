//! 工具系统 —— Tool trait 和 ToolRegistry。
//!
//! 教学要点：
//! - 工具是 Agent "动手"的能力——没有工具，Agent 只能聊天
//! - 工具 trait 定义了"所有工具长什么样"的统一接口
//! - 工具注册表管理所有可用工具，Agent 通过它来分发工具调用
//!
//! 借鉴对比：
//! - 对比 CoreCoder `tools/base.py`: Python 的 `Tool(ABC)` 有 3 个属性 + 1 个方法，
//!   本 crate 的 `Tool` trait 也是 3 个方法（name/description/parameters）+ 1 个（execute）
//! - 对比 pigs-core `tool.rs`: 那里用 `ToolHandler` trait + `Pin<Box<dyn Future>>` 返回类型，
//!   本 crate 用 `async_trait` 简化异步 trait 的定义，更易理解
//! - 对比 claw-analog: 那里不用 trait——工具是一个大 `match` 分发函数，
//!   本 crate 用 trait 是为了展示 Rust 的面向对象模式
//!
//! 设计决策：为什么 `execute` 返回 `String` 而非复杂类型？
//! 这是来自 CoreCoder 的设计——"所有工具都返回字符串，让消息拼接永远是 str"。
//! 这样 Agent 循环里处理工具结果时不用做类型匹配，简单直接。
//! 如果工具需要返回结构化数据，自己在 `execute` 内部序列化为字符串即可。

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{MiniAgentError, Result};

/// 工具 trait —— 每个工具必须实现的接口。
///
/// 实现这个 trait 就可以给 Agent 添加新能力。
/// 例如：读文件、写文件、执行命令、搜索代码等。
///
/// 教学要点：`#[async_trait]` 宏让 trait 可以定义 async fn。
/// 没有 `async_trait`，Rust 的 trait 不支持直接定义 async fn
/// （因为 async fn 的返回类型是匿名的 `impl Future`，trait 要求返回类型已知）。
/// `async_trait` 通过 `Pin<Box<dyn Future>>` 绕过这个限制。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称 —— LLM 通过这个名字调用工具。
    ///
    /// 要求：名称唯一，建议用蛇形命名（如 `read_file`、`edit_file`）。
    /// 这个名称会出现在系统提示词的工具列表中，也会出现在 LLM 的 tool_calls 中。
    fn name(&self) -> &str;

    /// 工具描述 —— 告诉 LLM 这个工具能做什么、什么时候用。
    ///
    /// 写好描述非常重要——LLM 根据描述决定是否调用这个工具。
    /// 例如："读取指定路径的文件内容，支持行号显示和范围读取"
    fn description(&self) -> &str;

    /// 参数 schema —— JSON Schema 格式，定义工具接受哪些参数。
    ///
    /// 教学要点：JSON Schema 是一种描述 JSON 数据结构的标准。
    /// LLM 根据 schema 知道应该传什么参数、什么类型。
    /// 例如 read_file 的 schema:
    /// ```json
    /// {
    ///   "type": "object",
    ///   "properties": {
    ///     "path": {"type": "string", "description": "要读取的文件路径"},
    ///     "offset": {"type": "integer", "description": "起始行号"}
    ///   },
    ///   "required": ["path"]
    /// }
    /// ```
    fn parameters(&self) -> Value;

    /// 执行工具 —— 接收 JSON 参数，返回执行结果字符串。
    ///
    /// 参数 `input` 是 LLM 传来的 JSON 值（已从字符串解析为 `Value`）。
    /// 返回值是工具执行的输出文本，会作为 tool 消息的内容发回给 LLM。
    ///
    /// 教学要点：返回 `Result<String, MiniAgentError>` 而非直接 panic。
    /// 工具出错时返回 `Err`，Agent 会把错误信息发回给 LLM，
    /// 让 LLM 看到错误并决定下一步（比如换个方式重试）。
    async fn execute(&self, input: Value) -> Result<String>;

    /// 生成 OpenAI function-calling 格式的工具 schema。
    ///
    /// 这是默认实现，子类通常不需要重写。
    /// 生成的格式：
    /// ```json
    /// {
    ///   "type": "function",
    ///   "function": {
    ///     "name": "...",
    ///     "description": "...",
    ///     "parameters": { ... }
    ///   }
    /// }
    /// ```
    ///
    /// 借鉴对比：CoreCoder 的 `Tool.schema()` 方法也是同样的逻辑和格式。
    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "function",                               // 固定为 "function"
            "function": {
                "name": self.name(),                          // 工具名称
                "description": self.description(),            // 工具描述
                "parameters": self.parameters(),              // 参数 schema
            }
        })
    }
}

/// 工具注册表 —— 管理所有可用工具。
///
/// 教学要点：`HashMap<String, Box<dyn Tool>>` 是 Rust 的 trait object 模式。
/// `Box<dyn Tool>` 是一个堆分配的、编译时大小未知的类型——
/// 不同的工具（read_file、bash 等）可以存到同一个 HashMap 里。
/// 这是 Rust 实现"多态"的方式（类似于 Python 里存不同类的对象到同一个 dict）。
///
/// 借鉴对比：
/// - 对比 CoreCoder: `self._tool_by_name = {t.name: t for t in tools}`
///   Python 版直接用 dict 存对象引用，Rust 版用 `Box<dyn Tool>` 存 trait object
/// - 对比 pigs-core: 那里也叫 `ToolRegistry`，设计几乎相同
/// - 对比 claw-analog: 那里不用注册表——工具是一个大 `match` 函数，
///   虽然简单但不灵活（添加新工具需要改分发函数代码）
pub struct ToolRegistry {
    /// 工具映射表 —— 工具名 → 工具实例
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// 创建一个空的工具注册表。
    ///
    /// 返回一个不含任何工具的 `ToolRegistry`。
    /// 之后可以通过 [`ToolRegistry::register`] 方法逐步添加工具。
    ///
    /// 教学要点：也可以用 `ToolRegistry::default()` 创建空注册表，
    /// 因为 `ToolRegistry` 实现了 `Default` trait。两者等价。
    pub fn new() -> Self {
        ToolRegistry {
            tools: HashMap::new(),
        }
    }

    /// 注册一个工具 —— 把工具加入注册表。
    ///
    /// 教学要点：传入 `Box<dyn Tool>` 而非泛型 `T: Tool`，
    /// 这样不同类型的工具可以注册到同一个注册表中。
    /// 这就是 trait object 的价值——统一管理不同类型。
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        // 先获取工具名，然后移动 tool 到 HashMap
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// 检查是否注册了某个工具名。
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// 获取所有工具的 schema 列表 —— 发送给 LLM 的工具定义。
    ///
    /// Agent 在每次调用 LLM 时，会把所有工具的 schema 作为
    /// `tools` 参数传给 API，告诉 LLM 有哪些工具可用。
    pub fn schemas(&self) -> Vec<Value> {
        // 遍历所有工具，调用每个工具的 schema() 方法
        self.tools.values().map(|t| t.schema()).collect()
    }

    /// 获取所有注册的工具名称。
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// 按名称执行工具 —— Agent 循环的核心分发逻辑。
    ///
    /// 当 LLM 返回 tool_call 时，Agent 用工具名在这里查找并执行。
    /// 如果找不到工具，返回错误——LLM 有时会"幻觉"出不存在的工具名。
    ///
    /// 参数:
    /// - `name`: 工具名（来自 LLM 的 tool_call.name）
    /// - `input`: 工具参数（来自 LLM 的 tool_call.arguments，已解析为 Value）
    ///
    /// 返回: 工具执行结果字符串
    pub async fn execute(&self, name: &str, input: Value) -> Result<String> {
        // 按名称查找工具
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| MiniAgentError::ToolError(format!("未知的工具: '{name}'")))?;
        // 找到了，调用工具的 execute 方法
        tool.execute(input).await
    }

    /// 获取注册的工具数量。
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 检查注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// 为 `ToolRegistry` 实现 `Default` trait —— 空注册表。
///
/// 教学要点：`Default` trait 让你可以用 `ToolRegistry::default()` 创建空实例。
/// 这在 Rust 中是惯用模式，很多标准库类型都实现了 `Default`。
impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 用一个简单的 "echo" 测试工具验证注册表功能

    use super::*;

    /// 回声工具 —— 测试用的简单工具，把输入原样返回。
    ///
    /// 这个工具在真实 Agent 中没用，但非常适合测试注册表。
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        /// 工具名: "echo"
        fn name(&self) -> &str {
            "echo"
        }

        /// 工具描述
        fn description(&self) -> &str {
            "回声工具 —— 原样返回输入的文本"
        }

        /// 参数 schema: 只需要一个 "text" 参数
        fn parameters(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "要回声的文本"
                    }
                },
                "required": ["text"]
            })
        }

        /// 执行: 把输入的 text 字段原样返回
        async fn execute(&self, input: Value) -> Result<String> {
            // 从 JSON 中提取 text 字段
            let text = input
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MiniAgentError::ToolError("缺少 'text' 参数".into()))?;
            // 原样返回
            Ok(text.to_string())
        }
    }

    /// 测试注册和执行工具
    #[tokio::test]
    async fn test_register_and_execute() {
        let mut registry = ToolRegistry::new();
        // 注册 echo 工具
        registry.register(Box::new(EchoTool));

        // 验证注册成功
        assert!(registry.has("echo"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        // 执行工具
        let result = registry
            .execute("echo", serde_json::json!({"text": "你好"}))
            .await
            .unwrap();
        // 验证结果
        assert_eq!(result, "你好");
    }

    /// 测试执行不存在的工具
    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let registry = ToolRegistry::new();
        // 执行不存在的工具应该返回错误
        let result = registry
            .execute("不存在的工具", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        // 验证错误类型
        match result {
            Err(MiniAgentError::ToolError(msg)) => assert!(msg.contains("未知的工具")),
            _ => panic!("期望 ToolError"),
        }
    }

    /// 测试获取工具 schema 列表
    #[test]
    fn test_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        // 验证 schema 格式
        let schema = &schemas[0];
        assert_eq!(schema["type"].as_str().unwrap(), "function");
        assert_eq!(schema["function"]["name"].as_str().unwrap(), "echo");
        assert!(schema["function"]["description"].is_string());
        assert!(schema["function"]["parameters"].is_object());
    }

    /// 测试获取工具名称列表
    #[test]
    fn test_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let names = registry.names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"echo".to_string()));
    }
}
