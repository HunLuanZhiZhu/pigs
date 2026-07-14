//! LLM 客户端 —— 连接大语言模型 API 的客户端。
//!
//! 教学要点：
//! - LLM 客户端负责把消息历史发给 API，获取 LLM 的回复
//! - 回复可能是纯文本（LLM 完成任务了），也可能包含工具调用
//! - 本 crate 使用 OpenAI 兼容的 Chat Completions API
//!   （支持 OpenAI / DeepSeek / Qwen / Kimi / Ollama 等）
//!
//! 借鉴对比：
//! - 对比 CoreCoder `llm.py`（336 行）: 那里处理了流式 SSE、重试、成本核算。
//!   本 crate 是教学版，只用非流式（简化 100+ 行 SSE 逻辑），但保留重试。
//!   注释中会指出流式实现可以参考 pigs-llm 的 `openai.rs`
//! - 对比 pigs-llm `openai.rs`（636 行）: 那里有完整的流式 SSE 解析、
//!   prompt cache、usage tracking。本 crate 只保留最核心的请求/响应
//! - 对比 claw-analog: 那里用 `ProviderClient` enum 支持 Anthropic + OpenAI，
//!   本 crate 只支持 OpenAI 兼容格式（覆盖大部分供应商）
//!
//! 设计决策：为什么用非流式而非流式？
//! 流式响应（SSE）的难点在于——LLM 返回的 tool_calls 参数是**碎片化**的：
//! 每个工具的 arguments JSON 被分成多个 delta 片段，需要按 index 累积拼接。
//! CoreCoder 的 `llm.py` 用了 30+ 行处理这个，pigs-llm 用了更多。
//! 教学版用非流式——API 一次性返回完整的 tool_calls，简单直接。
//! 注释中会说明流式版的关键思路，供读者深入探索。

use std::time::Duration;

use serde_json::Value;

use crate::error::{MiniAgentError, Result};
use crate::message::{Message, ToolCall};

/// LLM 客户端 —— 连接 OpenAI 兼容 API 的客户端。
///
/// 支持任何兼容 OpenAI Chat Completions API 的供应商：
/// - OpenAI: `https://api.openai.com/v1`
/// - DeepSeek: `https://api.deepseek.com/v1`
/// - Qwen (通义千问): `https://dashscope.aliyuncs.com/compatible-mode/v1`
/// - Kimi (月之暗面): `https://api.moonshot.cn/v1`
/// - Ollama (本地): `http://localhost:11434/v1`
///
/// 教学要点：所有这些供应商都实现了 OpenAI 兼容 API，
/// 所以只需要一个客户端就能对接所有供应商——这就是标准化的力量。
pub struct LlmClient {
    /// HTTP 客户端 —— reqwest 用于发送 HTTP 请求
    http: reqwest::Client,
    /// API 密钥 —— 通过 Authorization header 发送
    api_key: String,
    /// API 基础 URL —— 例如 `<https://api.openai.com/v1>`
    base_url: String,
    /// 模型名称 —— 例如 "gpt-4o" / "deepseek-chat" / "qwen-plus"
    model: String,
}

impl LlmClient {
    /// 创建一个新的 LLM 客户端。
    ///
    /// 参数:
    /// - `api_key`: API 密钥（如 "sk-xxxx"）
    /// - `base_url`: API 基础 URL（如 `<https://api.openai.com/v1>`）
    /// - `model`: 模型名（如 "gpt-4o"）
    ///
    /// 教学要点：构造器返回 `Self` 而非 `Result`，
    /// 因为 reqwest::Client::new() 在极少数情况下可能失败（如 TLS 初始化），
    /// 但在教学版中我们用 unwrap_or_else 简化处理。
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        LlmClient {
            // 创建 HTTP 客户端，设置 120 秒超时（LLM 响可能很慢）
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key: api_key.into(),   // API 密钥
            base_url: base_url.into(), // 基础 URL
            model: model.into(),       // 模型名
        }
    }

    /// 从环境变量创建 LLM 客户端。
    ///
    /// 读取以下环境变量：
    /// - `OPENAI_API_KEY`: API 密钥（必需）
    /// - `OPENAI_BASE_URL`: API 基础 URL（可选，默认 `<https://api.openai.com/v1>`）
    /// - `OPENAI_MODEL`: 模型名（可选，默认 "gpt-4o"）
    ///
    /// 教学要点：用环境变量而非配置文件，是 12-Factor App 的推荐做法。
    /// 也是 CoreCoder 和几乎所有 CLI 工具的惯例。
    pub fn from_env() -> Result<Self> {
        // 读取 API 密钥（必需）
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            MiniAgentError::ConfigError("未设置 OPENAI_API_KEY 环境变量".to_string())
        })?;
        // 读取基础 URL（可选，有默认值）
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        // 读取模型名（可选，有默认值）
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        // 构造客户端
        Ok(LlmClient::new(api_key, base_url, model))
    }

    /// 获取当前使用的模型名。
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 发送聊天请求 —— LLM 客户端的核心方法。
    ///
    /// 将对话历史和工具定义发给 LLM API，获取回复。
    /// 回复可能包含文本内容、工具调用，或两者都有。
    ///
    /// 参数:
    /// - `messages`: 对话历史（包含 system / user / assistant / tool 消息）
    /// - `tools`: 工具 schema 列表（告诉 LLM 有哪些工具可用）
    ///
    /// 返回: `LlmResponse`，包含文本回复和可能的工具调用
    ///
    /// 教学要点：这是整个 crate 中唯一与外部服务交互的地方。
    /// Agent 循环调用这个方法，根据返回值决定下一步。
    pub async fn chat(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        // 构建请求体 —— OpenAI Chat Completions 格式
        let messages_json: Vec<Value> = messages
            .iter()
            .map(|m| m.to_openai_json()) // 使用 Message 的序列化方法
            .collect();

        // 构建完整请求体
        let mut request_body = serde_json::json!({
            "model": self.model,                // 模型名
            "messages": messages_json,          // 对话历史
        });

        // 如果有工具，加入 tools 字段
        if !tools.is_empty() {
            request_body["tools"] = Value::Array(tools.to_vec());
            // tool_choice: "auto" 让 LLM 自己决定是否使用工具
            request_body["tool_choice"] = serde_json::json!("auto");
        }

        // 带重试地调用 API
        let response_json = self.call_with_retry(&request_body).await?;

        // 解析响应
        parse_llm_response(&response_json)
    }

    /// 带重试的 API 调用 —— 指数退避策略。
    ///
    /// 借鉴 CoreCoder 的 `_call_with_retry`：
    /// - 429 (Rate Limited): 退避重试
    /// - 5xx (Server Error): 退避重试
    /// - 4xx (Client Error): 不重试，直接返回错误
    ///
    /// 教学要点：网络请求可能因各种原因失败（限流、服务器错误、网络波动），
    /// 重试是提高可靠性的基本手段。指数退避（exponential backoff）是标准做法：
    /// 每次等待时间翻倍，避免持续冲击服务器。
    async fn call_with_retry(&self, body: &Value) -> Result<Value> {
        // 最大重试次数
        let max_retries = 3;
        // 初始等待时间（秒）
        let mut wait_secs = 1;

        for attempt in 0..=max_retries {
            // 发送 POST 请求到 /chat/completions 端点
            let url = format!("{}/chat/completions", self.base_url);
            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(body)
                .send()
                .await; // 发送请求

            match response {
                Ok(resp) => {
                    // 获取 HTTP 状态码
                    let status = resp.status();

                    if status.is_success() {
                        // 成功 —— 解析 JSON 响应
                        let json: Value = resp.json().await.map_err(|e| {
                            MiniAgentError::ParseError(format!("解析 API 响应失败: {e}"))
                        })?;
                        return Ok(json);
                    }

                    // 获取错误响应体
                    let error_text = resp.text().await.unwrap_or_default();

                    // 429 限流或 5xx 服务器错误 —— 可以重试
                    if (status.as_u16() == 429 || status.is_server_error()) && attempt < max_retries
                    {
                        eprintln!(
                            "[llm] 请求失败 (HTTP {})，{}秒后重试 (第 {}/{} 次)",
                            status.as_u16(),
                            wait_secs,
                            attempt + 1,
                            max_retries
                        );
                        // 等待后重试
                        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                        wait_secs *= 2; // 指数退避：1 → 2 → 4 秒
                        continue;
                    }

                    // 4xx 客户端错误（非 429）—— 不重试
                    return Err(MiniAgentError::LlmError(format!(
                        "API 请求失败 (HTTP {}): {}",
                        status.as_u16(),
                        error_text
                    )));
                }
                Err(e) => {
                    // 网络错误（DNS 解析失败、连接超时等）—— 可以重试
                    if attempt < max_retries {
                        eprintln!(
                            "[llm] 网络错误: {e}，{}秒后重试 (第 {}/{} 次)",
                            wait_secs,
                            attempt + 1,
                            max_retries
                        );
                        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                        wait_secs *= 2;
                        continue;
                    }
                    // 重试次数用尽，返回错误
                    return Err(MiniAgentError::NetworkError(e.to_string()));
                }
            }
        }
        // 理论上不会到达这里（循环里所有路径都有 return）
        Err(MiniAgentError::LlmError("重试次数用尽".to_string()))
    }
}

/// LLM 的回复 —— 从 API 响应中解析出的结构。
///
/// 包含 LLM 的文本回复和可能的工具调用。
/// Agent 循环根据这个返回值决定下一步：
/// - 有 tool_calls → 执行工具，继续循环
/// - 没有 tool_calls → 返回文本给用户，结束
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// LLM 的文本回复 —— 可能是空字符串（如果 LLM 只返回了工具调用）
    pub content: String,

    /// 工具调用列表 —— 如果 LLM 决定要使用工具
    ///
    /// 空列表表示 LLM 没有请求工具调用，任务完成。
    pub tool_calls: Vec<ToolCall>,
}

/// 解析 OpenAI API 响应 JSON 为 `LlmResponse`。
///
/// OpenAI 响应格式（简化）：
/// ```json
/// {
///   "choices": [{
///     "message": {
///       "role": "assistant",
///       "content": "LLM 的文本回复",
///       "tool_calls": [{
///         "id": "call_001",
///         "type": "function",
///         "function": {
///           "name": "read_file",
///           "arguments": "{\"path\":\"/tmp/test\"}"
///         }
///       }]
///     },
///     "finish_reason": "tool_calls"
///   }]
/// }
/// ```
///
/// 教学要点：这个函数处理了 LLM 输出的"不确定性"——
/// 有些字段可能不存在、可能为 null、可能格式不对。
/// 用防御式编程处理每种情况。
fn parse_llm_response(json: &Value) -> Result<LlmResponse> {
    // 获取 choices 数组的第一个元素（通常只有一个）
    let choice = json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| MiniAgentError::ParseError("API 响应缺少 choices 字段".to_string()))?;

    // 获取 message 对象
    let message = choice
        .get("message")
        .ok_or_else(|| MiniAgentError::ParseError("API 响应缺少 message 字段".to_string()))?;

    // 提取文本内容（可能为 null）
    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    // 提取工具调用（可能不存在）
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for call in calls {
            // 解析每个工具调用
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            // 获取 function 对象（包含 name 和 arguments）
            let function = call.get("function").ok_or_else(|| {
                MiniAgentError::ParseError("工具调用缺少 function 字段".to_string())
            })?;

            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            // arguments 是 JSON 字符串，需要解析为 Value
            // 教学要点：OpenAI API 返回的 arguments 是字符串（如 "{\"path\":\"/tmp\"}"），
            // 不是 JSON 对象。需要 serde_json::from_str 解析。
            let arguments_str = function
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");

            // 解析参数 JSON 字符串——来自 CoreCoder 的设计：
            // 解析失败时降级为空对象 {}，让下游工具处理参数缺失
            let arguments: Value =
                serde_json::from_str(arguments_str).unwrap_or_else(|_| serde_json::json!({}));

            tool_calls.push(ToolCall {
                id,        // 工具调用 ID
                name,      // 工具名
                arguments, // 已解析的参数
            });
        }
    }

    Ok(LlmResponse {
        content,
        tool_calls,
    })
}

/// 用于测试的 mock 响应数据
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// 测试解析纯文本回复（无工具调用）
    #[test]
    fn test_parse_text_only_response() {
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "你好！我是 Agent。"
                },
                "finish_reason": "stop"
            }]
        });

        let response = parse_llm_response(&response_json).unwrap();
        // 验证文本内容
        assert_eq!(response.content, "你好！我是 Agent。");
        // 验证没有工具调用
        assert!(response.tool_calls.is_empty());
    }

    /// 测试解析含工具调用的回复
    #[test]
    fn test_parse_tool_call_response() {
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "让我读取这个文件",
                    "tool_calls": [{
                        "id": "call_001",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"/tmp/test.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let response = parse_llm_response(&response_json).unwrap();
        // 验证文本内容
        assert_eq!(response.content, "让我读取这个文件");
        // 验证有一个工具调用
        assert_eq!(response.tool_calls.len(), 1);
        // 验证工具调用详情
        assert_eq!(response.tool_calls[0].id, "call_001");
        assert_eq!(response.tool_calls[0].name, "read_file");
        // 验证参数已正确解析为 JSON 对象
        assert_eq!(
            response.tool_calls[0].arguments["path"].as_str().unwrap(),
            "/tmp/test.txt"
        );
    }

    /// 测试解析参数格式错误的情况（降级为空对象）
    #[test]
    fn test_parse_bad_arguments() {
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_002",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": "这不是合法JSON"
                        }
                    }]
                }
            }]
        });

        let response = parse_llm_response(&response_json).unwrap();
        // 验证参数降级为空对象
        assert_eq!(response.tool_calls.len(), 1);
        assert!(response.tool_calls[0].arguments.is_object());
    }

    /// 测试解析缺少 choices 的无效响应
    #[test]
    fn test_parse_missing_choices() {
        let response_json = serde_json::json!({"error": "bad request"});
        let result = parse_llm_response(&response_json);
        // 应该返回错误
        assert!(result.is_err());
    }

    /// 测试从环境变量创建客户端 —— 合并无 key 和有 key 两种情况为一个测试，
    /// 避免并行运行时环境变量竞争。
    ///
    /// 教学要点：环境变量是进程级共享状态，并行测试时会产生竞争条件。
    /// 一个测试设置了环境变量，另一个测试可能看到这个"脏"状态。
    /// 解决方案：把有依赖的测试合并到同一个串行执行的测试函数中。
    #[test]
    fn test_from_env() {
        // 保存原始环境变量值（可能存在也可能不存在）
        let old_key = std::env::var("OPENAI_API_KEY").ok();
        let old_url = std::env::var("OPENAI_BASE_URL").ok();
        let old_model = std::env::var("OPENAI_MODEL").ok();

        // --- 场景 1: 未设置 API Key 时应该报错 ---
        std::env::remove_var("OPENAI_API_KEY");
        let result = LlmClient::from_env();
        assert!(result.is_err(), "未设置 OPENAI_API_KEY 时应该报错");

        // --- 场景 2: 设置了 API Key 时应该成功 ---
        std::env::set_var("OPENAI_API_KEY", "test-key-12345");
        let client = LlmClient::from_env();
        assert!(client.is_ok(), "设置了 OPENAI_API_KEY 时应该成功");
        let client = client.unwrap();
        assert_eq!(client.api_key, "test-key-12345");
        assert_eq!(client.model, "gpt-4o"); // 默认值

        // --- 场景 3: 同时设置 BASE_URL 和 MODEL ---
        std::env::set_var("OPENAI_BASE_URL", "https://api.deepseek.com/v1");
        std::env::set_var("OPENAI_MODEL", "deepseek-chat");
        let client = LlmClient::from_env().unwrap();
        assert_eq!(client.base_url, "https://api.deepseek.com/v1");
        assert_eq!(client.model, "deepseek-chat");

        // 恢复原始环境变量
        match old_key {
            Some(key) => std::env::set_var("OPENAI_API_KEY", key),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match old_url {
            Some(url) => std::env::set_var("OPENAI_BASE_URL", url),
            None => std::env::remove_var("OPENAI_BASE_URL"),
        }
        match old_model {
            Some(model) => std::env::set_var("OPENAI_MODEL", model),
            None => std::env::remove_var("OPENAI_MODEL"),
        }
    }

    /// 测试创建客户端
    #[test]
    fn test_new_client() {
        let client = LlmClient::new("my-key", "https://api.example.com/v1", "my-model");
        assert_eq!(client.api_key, "my-key");
        assert_eq!(client.base_url, "https://api.example.com/v1");
        assert_eq!(client.model, "my-model");
        assert_eq!(client.model(), "my-model");
    }
}
