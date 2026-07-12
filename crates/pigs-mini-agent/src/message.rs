//! 消息模型 —— 对话历史的核心数据结构。
//!
//! 教学要点：
//! - AI Agent 的对话历史本质就是一个消息列表
//! - 每条消息有一个"角色"（role），表示是谁说的
//! - 消息内容不只是纯文本——assistant 消息可以包含"工具调用请求"，
//!   而工具执行结果也是消息的一种
//!
//! 借鉴对比：
//! - 对比 CoreCoder: Python 里用裸 dict 存消息（`{"role": "user", "content": "..."}`），
//!   本 crate 用 Rust 的类型系统保证消息格式正确
//! - 对比 pigs-core: 那里用了 `ContentBlock` enum，本 crate 简化了——
//!   assistant 消息的文本和工具调用放在同一结构体的不同字段
//! - 对比 codex: 那里用 `ResponseItem` enum 覆盖了 20+ 种消息类型，
//!   本 crate 只保留最基本的 4 种角色，足以展示 Agent 循环的核心
//!
//! 消息在对话历史中的流转：
//! ```text
//! 用户输入 → [User 消息] → 送给 LLM
//!                                ↓
//!                     LLM 返回 [Assistant 消息（含工具调用）]
//!                                ↓
//!                     执行工具 → [Tool 消息（工具结果）]
//!                                ↓
//!                     再送给 LLM → 可能再次调用工具...
//!                                ↓
//!                     LLM 返回 [Assistant 消息（纯文本）] → 返回给用户
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 消息角色 —— 标识一条消息是"谁说的"。
///
/// 对话中只有四种角色，这与 OpenAI Chat Completions API 对齐：
/// - `System`: 系统提示词（设定 Agent 的行为规则，只出现一次）
/// - `User`: 用户的输入
/// - `Assistant`: LLM 的回复
/// - `Tool`: 工具执行的结果
///
/// 教学要点：OpenAI 和 Anthropic 对工具结果的处理方式不同：
/// - OpenAI: 用 `role: "tool"` 作为独立角色
/// - Anthropic: 把工具结果放到 `role: "user"` 的消息里
///
/// 本 crate 选 OpenAI 格式，因为更直观——工具结果就是一种独立的消息。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] // 序列化时转为小写: "system" / "user" / "assistant" / "tool"
pub enum Role {
    /// 系统角色 —— Agent 的行为指令，通常是对话的第一条消息
    System,
    /// 用户角色 —— 人类用户的输入
    User,
    /// 助手角色 —— LLM 生成的回复（可能包含工具调用）
    Assistant,
    /// 工具角色 —— 工具执行的结果
    Tool,
}

/// 为 `Role` 实现 `Display` trait，方便日志打印和调试。
///
/// 例如 `println!("角色: {}", Role::User)` 会输出 "角色: user"
impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 系统角色显示为 "system"
            Role::System => write!(f, "system"),
            // 用户角色显示为 "user"
            Role::User => write!(f, "user"),
            // 助手角色显示为 "assistant"
            Role::Assistant => write!(f, "assistant"),
            // 工具角色显示为 "tool"
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// 工具调用 —— LLM 请求执行的一个工具。
///
/// 当 LLM 决定要使用工具时，它会返回一个或多个 `ToolCall`。
/// 每个 `ToolCall` 包含工具名、参数和一个用于匹配结果的唯一 ID。
///
/// 借鉴对比：
/// - 对比 CoreCoder: 那里定义了 `ToolCall(id, name, arguments: dict)`，
///   本 crate 的 `arguments` 用 `serde_json::Value` 替代 Python dict
/// - `arguments` 在 LLM 层就已经解析好了（从 JSON 字符串转为 Value），
///   下游工具直接拿 Value 用，不用重复解析——这是 CoreCoder 的设计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 工具调用的唯一 ID —— LLM 生成，用于匹配工具调用和执行结果。
    ///
    /// 为什么需要 ID：一次 LLM 回复可能包含多个工具调用，
    /// 工具结果消息需要通过这个 ID 告诉 LLM "这是哪个工具调用的结果"
    pub id: String,

    /// 工具名称 —— 对应 `ToolRegistry` 中注册的工具名。
    pub name: String,

    /// 工具参数 —— 已解析好的 JSON 值。
    ///
    /// 注意：OpenAI API 返回的 `arguments` 是 JSON **字符串**（如 `"{\"path\":\"/tmp\"}"`），
    /// 需要在 LLM 客户端层做 `serde_json::from_str` 解析。
    /// 解析失败时降级为空对象 `{}`，让下游工具处理参数缺失——
    /// 这也是 CoreCoder 的设计决策。
    pub arguments: Value,
}

/// 一条对话消息。
///
/// 这是对话历史的基本单元。消息按角色分类，assistant 消息可以
/// 同时包含文本回复和工具调用。
///
/// 为什么 assistant 消息需要同时有 `content` 和 `tool_calls`：
/// 因为 LLM 可以在回复中先说"让我帮你看看这个文件"，然后同时
/// 发起一个 `read_file` 工具调用。文本和工具调用是并存的。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息角色 —— system / user / assistant / tool
    pub role: Role,

    /// 文本内容 —— 消息的文本部分。
    ///
    /// 对于 user 消息：就是用户输入的文本
    /// 对于 assistant 消息：LLM 生成的文本回复（如果有工具调用，这里可能有也可能没有文本）
    /// 对于 tool 消息：工具执行的结果文本
    /// 对于 system 消息：系统提示词
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// 工具调用列表 —— 只有 assistant 消息可能有这个字段。
    ///
    /// 当 LLM 决定要使用工具时，会在这里返回一个或多个 `ToolCall`。
    /// 其他角色的消息这个字段为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    /// 工具调用 ID —— 只有 tool 消息有这个字段。
    ///
    /// 指明这个工具结果是对应哪个 `ToolCall::id` 的回复。
    /// OpenAI API 要求每个 assistant 的 tool_call 必须有对应的 tool 回复，
    /// 否则下次请求会报错——这是 CoreCoder `_answer_pending_tool_calls` 的教训。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// 创建一条系统消息 —— Agent 的行为指令。
    ///
    /// 系统消息通常是对话的第一条消息，定义 Agent 的角色、能力、规则。
    /// 整个对话过程中通常只有一条系统消息。
    pub fn system(text: impl Into<String>) -> Self {
        Message {
            role: Role::System,         // 系统角色
            content: Some(text.into()),  // 系统提示词文本
            tool_calls: None,            // 系统消息不会有工具调用
            tool_call_id: None,          // 系统消息不会有工具调用 ID
        }
    }

    /// 创建一条用户消息 —— 人类用户的输入。
    ///
    /// 用户消息只有文本内容，不会有工具调用。
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,            // 用户角色
            content: Some(text.into()),  // 用户输入的文本
            tool_calls: None,            // 用户不会发起工具调用
            tool_call_id: None,          // 用户消息没有工具调用 ID
        }
    }

    /// 创建一条 assistant 消息（纯文本，无工具调用）。
    ///
    /// 当 LLM 完成任务、不再需要工具时，回复的就是纯文本。
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,      // 助手角色
            content: Some(text.into()),  // LLM 的文本回复
            tool_calls: None,            // 纯文本消息没有工具调用
            tool_call_id: None,          // 助手消息没有工具调用 ID
        }
    }

    /// 创建一条 assistant 消息（含工具调用）。
    ///
    /// 当 LLM 决定要使用工具时，用这个构造器。
    /// `text` 可以是空字符串（LLM 可能只返回工具调用不附带文字）。
    /// `calls` 是 LLM 请求执行的工具调用列表。
    pub fn assistant_with_tools(
        text: impl Into<String>,
        calls: Vec<ToolCall>,
    ) -> Self {
        let content = text.into();
        Message {
            role: Role::Assistant,                       // 助手角色
            content: if content.is_empty() { None } else { Some(content) }, // 空文本则不序列化
            tool_calls: Some(calls),                     // 工具调用列表
            tool_call_id: None,                          // 助手消息没有工具调用 ID
        }
    }

    /// 创建一条 tool 消息 —— 工具执行的结果。
    ///
    /// `tool_call_id` 必须对应某个 assistant 消息中 `ToolCall::id`，
    /// 这样 LLM 才能知道这是哪个工具调用的结果。
    /// `output` 是工具执行的输出文本。
    /// `is_error` 标记是否为错误结果——LLM 可以根据这个调整策略。
    pub fn tool_result(tool_call_id: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        // 如果是错误，在输出前加 [ERROR] 前缀，让 LLM 更容易识别
        let output_text = if is_error {
            format!("[ERROR] {}", output.into())
        } else {
            output.into()
        };
        Message {
            role: Role::Tool,                                  // 工具角色
            content: Some(output_text),                         // 工具执行结果文本
            tool_calls: None,                                   // 工具消息不会有工具调用
            tool_call_id: Some(tool_call_id.into()),            // 关联的工具调用 ID
        }
    }

    /// 判断这条消息是否包含工具调用。
    ///
    /// Agent 循环用这个方法判断 LLM 是否请求了工具调用：
    /// 如果有 → 执行工具，继续循环
    /// 如果没有 → LLM 完成了任务，返回文本
    pub fn has_tool_calls(&self) -> bool {
        // tool_calls 不为 None 且不为空列表时返回 true
        self.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty())
    }

    /// 获取这条消息的工具调用列表（如果有）。
    ///
    /// 返回 `&[ToolCall]` 切片引用，调用者可以遍历。
    /// 如果没有工具调用，返回空切片。
    pub fn get_tool_calls(&self) -> &[ToolCall] {
        // 如果 tool_calls 为 None，返回空切片；否则返回内部 Vec 的切片
        match &self.tool_calls {
            Some(calls) => calls,
            None => &[],
        }
    }

    /// 将消息序列化为 OpenAI Chat Completions API 格式的 JSON 值。
    ///
    /// 教学要点：这个方法展示了如何将内部类型映射到 API 请求格式。
    /// OpenAI 格式：
    /// - user/system/tool: `{"role": "user", "content": "..."}`
    /// - assistant（纯文本）: `{"role": "assistant", "content": "..."}`
    /// - assistant（含工具调用）: `{"role": "assistant", "content": "...", "tool_calls": [...]}`
    /// - tool（工具结果）: `{"role": "tool", "tool_call_id": "...", "content": "..."}`
    pub fn to_openai_json(&self) -> Value {
        // 基础结构：角色 + 内容
        let mut msg = serde_json::json!({
            "role": self.role.to_string(),  // 角色转为字符串 "user"/"assistant" 等
        });

        // 如果有文本内容，加入 content 字段
        if let Some(content) = &self.content {
            msg["content"] = Value::String(content.clone());
        }

        // 如果有工具调用，加入 tool_calls 字段
        // 格式: [{"id": "...", "type": "function", "function": {"name": "...", "arguments": "..."}}]
        if let Some(calls) = &self.tool_calls {
            let calls_json: Vec<Value> = calls
                .iter()
                .map(|tc| {
                    // 注意：OpenAI 要求 arguments 是 JSON **字符串**，不是对象
                    // 所以这里要把 Value 序列化回字符串
                    serde_json::json!({
                        "id": tc.id,                                        // 工具调用 ID
                        "type": "function",                                  // 固定为 "function"
                        "function": {
                            "name": tc.name,                                // 工具名
                            "arguments": tc.arguments.to_string(),          // 参数转回 JSON 字符串
                        }
                    })
                })
                .collect();
            msg["tool_calls"] = Value::Array(calls_json);
        }

        // 如果是 tool 消息，加入 tool_call_id 字段
        if let Some(id) = &self.tool_call_id {
            msg["tool_call_id"] = Value::String(id.clone());
        }

        msg
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // 测试模块 —— 验证消息模型的构造、序列化和工具调用提取

    use super::*;

    /// 测试创建用户消息
    #[test]
    fn test_user_message() {
        let msg = Message::user("你好，请帮我读一下文件");
        // 验证角色
        assert_eq!(msg.role, Role::User);
        // 验证内容
        assert_eq!(msg.content.as_deref(), Some("你好，请帮我读一下文件"));
        // 用户消息不应该有工具调用
        assert!(!msg.has_tool_calls());
    }

    /// 测试创建含工具调用的 assistant 消息
    #[test]
    fn test_assistant_with_tools() {
        let tool_call = ToolCall {
            id: "call_001".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let msg = Message::assistant_with_tools("让我看看这个文件", vec![tool_call]);

        // 验证角色
        assert_eq!(msg.role, Role::Assistant);
        // 验证包含工具调用
        assert!(msg.has_tool_calls());
        // 验证工具调用列表
        let calls = msg.get_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].id, "call_001");
    }

    /// 测试创建工具结果消息
    #[test]
    fn test_tool_result_message() {
        let msg = Message::tool_result("call_001", "文件内容: hello world", false);
        // 验证角色
        assert_eq!(msg.role, Role::Tool);
        // 验证关联的工具调用 ID
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_001"));
        // 验证内容
        assert!(msg.content.as_deref().unwrap().contains("hello world"));
    }

    /// 测试工具结果消息的错误标记
    #[test]
    fn test_tool_result_error() {
        let msg = Message::tool_result("call_002", "文件不存在", true);
        // 错误结果应该有 [ERROR] 前缀
        let content = msg.content.as_deref().unwrap();
        assert!(content.contains("[ERROR]"));
        assert!(content.contains("文件不存在"));
    }

    /// 测试序列化为 OpenAI 格式
    #[test]
    fn test_to_openai_json_user() {
        let msg = Message::user("测试消息");
        let json = msg.to_openai_json();
        // 验证 role 字段
        assert_eq!(json["role"].as_str().unwrap(), "user");
        // 验证 content 字段
        assert_eq!(json["content"].as_str().unwrap(), "测试消息");
    }

    /// 测试 assistant 工具调用消息的序列化
    #[test]
    fn test_to_openai_json_with_tools() {
        let tool_call = ToolCall {
            id: "call_001".to_string(),
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "ls -la"}),
        };
        let msg = Message::assistant_with_tools("正在执行", vec![tool_call]);
        let json = msg.to_openai_json();

        // 验证 role
        assert_eq!(json["role"].as_str().unwrap(), "assistant");
        // 验证 tool_calls 存在
        let calls = json["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        // 验证工具调用格式
        assert_eq!(calls[0]["id"].as_str().unwrap(), "call_001");
        assert_eq!(calls[0]["type"].as_str().unwrap(), "function");
        assert_eq!(calls[0]["function"]["name"].as_str().unwrap(), "bash");
        // arguments 应该是字符串（OpenAI 格式要求）
        assert!(calls[0]["function"]["arguments"].is_string());
    }

    /// 测试 tool 消息的序列化
    #[test]
    fn test_to_openai_json_tool() {
        let msg = Message::tool_result("call_001", "执行成功", false);
        let json = msg.to_openai_json();
        // 验证 role
        assert_eq!(json["role"].as_str().unwrap(), "tool");
        // 验证 tool_call_id
        assert_eq!(json["tool_call_id"].as_str().unwrap(), "call_001");
    }
}
