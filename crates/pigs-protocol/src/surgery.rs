//! 请求体手术：按协议对 JSON body 做定位与修改。
//!
//! 所有操作都基于 `serde_json::Value`，不构造 typed 模型——编排只需要改少数几个字段。
//!
//! 相位之间的信息传递方式与 legacy `pigs-api/src/protocol.rs` 一致：
//! - Pre / Executor 把指令**追加到最后一条 user 消息的文本上**（不动原有内容，
//!   图片等非文本块也不会丢）；
//! - Post 把上一只 pig 的产出作为**一条 assistant 消息接回对话**，再追加一条新的
//!   user 消息作为验收指令——模型看到的是"我自己写的稿子"，而不是"用户塞来的材料"。

use crate::route::Protocol;
use crate::{Error, Result};
use serde_json::{json, Value};

/// 相位指令的追加格式（与 legacy 一致）：空行 + 分隔线 + 空行 + 指令。
pub const SUFFIX_SEPARATOR: &str = "\n\n---\n\n";

/// 读取 body 中的 `model` 字段。
pub fn get_model(body: &Value) -> Option<&str> {
    body.get("model").and_then(|m| m.as_str())
}

/// 覆盖 `model` 字段。
pub fn set_model(body: &mut Value, model: &str) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), Value::String(model.into()));
    }
}

/// 设置流式标志（客户端要流式 → 子请求也流式）。
pub fn set_stream(body: &mut Value, stream: bool) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".into(), Value::Bool(stream));
    }
}

/// 客户端是否请求了流式。
pub fn has_client_stream(body: &Value) -> bool {
    body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false)
}

/// 去掉工具定义字段（编排层不执行工具，去掉后模型会以纯文本作答）。
/// 三种协议的工具字段名恰好都是 `tools`（Responses/Anthropic/OpenAI），附带清掉 `tool_choice`。
pub fn strip_tools(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("tools");
        obj.remove("tool_choice");
    }
}

/// 把相位指令追加到最后一条 user 消息的文本后面（Pre / Executor 相位用）。
///
/// - OpenAI Chat / Anthropic：`messages` 数组里最后一条 `role=="user"`；
/// - Responses：`input` 是字符串则直接拼接；是数组则改最后一条 user 项。
///
/// 字符串 content → 直接拼接；块数组 → 追加到最后一块文本 part（没有则新建一块），
/// 其余内容（图片、文档等）保持原样。
pub fn append_to_last_user_text(body: &mut Value, protocol: Protocol, suffix: &str) -> Result<()> {
    let addition = format!("{SUFFIX_SEPARATOR}{suffix}");
    match protocol {
        Protocol::OpenAI | Protocol::Anthropic => {
            let arr = body
                .get_mut("messages")
                .and_then(|m| m.as_array_mut())
                .ok_or(Error::NoUserMessage(protocol))?;
            let last_user = arr
                .iter_mut()
                .rev()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                .ok_or(Error::NoUserMessage(protocol))?;
            let content = last_user
                .get_mut("content")
                .ok_or(Error::NoUserMessage(protocol))?;
            append_to_text_content(content, &addition, protocol)?;
            Ok(())
        }
        Protocol::Responses => {
            let obj = body.as_object_mut().ok_or(Error::NoUserMessage(protocol))?;
            match obj.get_mut("input") {
                Some(Value::String(text)) => {
                    text.push_str(&addition);
                    Ok(())
                }
                Some(Value::Array(items)) => {
                    let last_user = items
                        .iter_mut()
                        .rev()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                        .ok_or(Error::NoUserMessage(protocol))?;
                    let content = last_user
                        .get_mut("content")
                        .ok_or(Error::NoUserMessage(protocol))?;
                    append_to_text_content(content, &addition, protocol)?;
                    Ok(())
                }
                _ => Err(Error::NoUserMessage(protocol)),
            }
        }
    }
}

/// 追加一条新的 user 消息（Post 相位的验收指令用）。
///
/// Responses 的 `input` 若是字符串，会先被就地展开成一条 user 消息（legacy 同款处理），
/// 以免丢掉原问题。
pub fn push_user_message(body: &mut Value, protocol: Protocol, text: &str) -> Result<()> {
    match protocol {
        Protocol::OpenAI | Protocol::Anthropic => {
            let arr = messages_mut(body, protocol)?;
            arr.push(json!({"role": "user", "content": text}));
            Ok(())
        }
        Protocol::Responses => {
            let items = input_items_mut(body, protocol)?;
            items.push(json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}]
            }));
            Ok(())
        }
    }
}

/// 把上一只 pig 的产出作为一条 assistant 消息接回对话。
pub fn push_assistant_message(body: &mut Value, protocol: Protocol, text: &str) -> Result<()> {
    match protocol {
        Protocol::OpenAI | Protocol::Anthropic => {
            let arr = messages_mut(body, protocol)?;
            arr.push(json!({"role": "assistant", "content": text}));
            Ok(())
        }
        Protocol::Responses => {
            let items = input_items_mut(body, protocol)?;
            items.push(json!({
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": text}]
            }));
            Ok(())
        }
    }
}

/// 提取最后一条 user 消息的文本（语言检测、日志用；提取不到返回空串）。
pub fn extract_last_user_text(body: &Value, protocol: Protocol) -> String {
    let user_text = |content: &Value| -> String {
        match content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    };
    match protocol {
        Protocol::OpenAI | Protocol::Anthropic => body
            .get("messages")
            .and_then(|m| m.as_array())
            .and_then(|arr| {
                arr.iter().rev().find(|m| {
                    m.get("role").and_then(|r| r.as_str()) == Some("user")
                })
            })
            .and_then(|m| m.get("content"))
            .map(user_text)
            .unwrap_or_default(),
        Protocol::Responses => match body.get("input") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(items)) => items
                .iter()
                .rev()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                .and_then(|m| m.get("content"))
                .map(user_text)
                .unwrap_or_default(),
            _ => String::new(),
        },
    }
}

/// OpenAI / Anthropic 的 `messages` 数组（缺失或类型不对 → 报错）。
fn messages_mut(body: &mut Value, protocol: Protocol) -> Result<&mut Vec<Value>> {
    body.get_mut("messages")
        .and_then(|m| m.as_array_mut())
        .ok_or(Error::NoUserMessage(protocol))
}

/// Responses 的 `input` 数组；字符串形式先展开成一条 user 消息（保留原问题）。
fn input_items_mut(body: &mut Value, protocol: Protocol) -> Result<&mut Vec<Value>> {
    let obj = body.as_object_mut().ok_or(Error::NoUserMessage(protocol))?;
    if let Some(Value::String(text)) = obj.get("input") {
        let original = text.clone();
        obj.insert(
            "input".into(),
            json!([{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": original}]
            }]),
        );
    }
    if !matches!(obj.get("input"), Some(Value::Array(_))) {
        obj.insert("input".into(), Value::Array(Vec::new()));
    }
    obj.get_mut("input")
        .and_then(|i| i.as_array_mut())
        .ok_or(Error::NoUserMessage(protocol))
}

/// 往 content（字符串或块数组）里追加文本（legacy `append_to_text_content` 同款）。
fn append_to_text_content(content: &mut Value, addition: &str, protocol: Protocol) -> Result<()> {
    match content {
        // 字符串：直接拼
        Value::String(text) => {
            text.push_str(addition);
            Ok(())
        }
        // 块数组：追加到最后一块文本 part；没有则新建
        Value::Array(parts) => {
            let appended = parts.iter_mut().rev().find_map(|part| {
                let ty = part.get("type").and_then(|t| t.as_str())?;
                if !ty.ends_with("text") {
                    return None;
                }
                match part.get_mut("text") {
                    Some(Value::String(text)) => Some(text),
                    _ => None,
                }
            });
            match appended {
                Some(text) => text.push_str(addition),
                None => {
                    let part_type = match protocol {
                        Protocol::OpenAI | Protocol::Anthropic => "text",
                        Protocol::Responses => "input_text",
                    };
                    parts.push(json!({"type": part_type, "text": addition}));
                }
            }
            Ok(())
        }
        _ => Err(Error::NoUserMessage(protocol)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_appends_instruction_to_last_user_only() {
        let mut body = json!({
            "model": "gpt-x",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"},
                {"role": "user", "content": "do the task"}
            ]
        });
        append_to_last_user_text(&mut body, Protocol::OpenAI, "指令").unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"], "hello");
        assert_eq!(msgs[3]["content"], "do the task\n\n---\n\n指令");
    }

    #[test]
    fn anthropic_appends_to_block_content_without_losing_other_blocks() {
        let mut body = json!({
            "model": "claude-x",
            "system": "sys",
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"data": "xxx"}},
                {"type": "text", "text": "看图"}
            ]}]
        });
        append_to_last_user_text(&mut body, Protocol::Anthropic, "指令").unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "不该新增块，图片要留着");
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[1]["text"], "看图\n\n---\n\n指令");
        // system 不受影响
        assert_eq!(body["system"], "sys");
    }

    #[test]
    fn anthropic_creates_text_block_when_none_exists() {
        let mut body = json!({
            "model": "claude-x",
            "messages": [{"role": "user", "content": [{"type": "image", "source": {}}]}]
        });
        append_to_last_user_text(&mut body, Protocol::Anthropic, "指令").unwrap();
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "\n\n---\n\n指令");
    }

    #[test]
    fn post_turn_is_assistant_draft_then_new_user_instruction() {
        let mut body = json!({
            "model": "gpt-x",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "帮我完成任务Z"}
            ]
        });
        push_assistant_message(&mut body, Protocol::OpenAI, "草稿").unwrap();
        push_user_message(&mut body, Protocol::OpenAI, "请验收").unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "帮我完成任务Z", "原问题不许被覆盖");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["content"], "草稿");
        assert_eq!(msgs[3]["role"], "user");
        assert_eq!(msgs[3]["content"], "请验收");
    }

    #[test]
    fn responses_string_input_is_expanded_and_kept() {
        let mut body = json!({"model": "m", "input": "原始问题"});
        push_assistant_message(&mut body, Protocol::Responses, "草稿").unwrap();
        push_user_message(&mut body, Protocol::Responses, "请验收").unwrap();
        let items = body["input"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][0]["text"], "原始问题");
        assert_eq!(items[1]["role"], "assistant");
        assert_eq!(items[1]["content"][0]["text"], "草稿");
        assert_eq!(items[2]["role"], "user");
        assert_eq!(items[2]["content"][0]["text"], "请验收");
    }

    #[test]
    fn responses_array_input_appends_and_creates_missing_input() {
        let mut body = json!({"model": "m", "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "q"}]}
        ]});
        append_to_last_user_text(&mut body, Protocol::Responses, "P").unwrap();
        assert_eq!(body["input"][0]["content"][0]["text"], "q\n\n---\n\nP");

        // 完全没有 input 字段 → 报错（不是静默丢弃）
        let mut body = json!({"model": "m"});
        let err = append_to_last_user_text(&mut body, Protocol::Responses, "P").unwrap_err();
        assert!(matches!(err, Error::NoUserMessage(_)));
    }

    #[test]
    fn no_user_message_is_an_error() {
        let mut body = json!({"model": "m", "messages": [
            {"role": "assistant", "content": "hi"}
        ]});
        let err = append_to_last_user_text(&mut body, Protocol::OpenAI, "P").unwrap_err();
        assert!(matches!(err, Error::NoUserMessage(_)));
    }

    #[test]
    fn model_stream_tools_ops() {
        let mut body = json!({"model": "x-pig", "stream": true, "tools": [1]});
        assert_eq!(get_model(&body), Some("x-pig"));
        set_model(&mut body, "x");
        set_stream(&mut body, false);
        strip_tools(&mut body);
        assert_eq!(get_model(&body), Some("x"));
        assert!(!has_client_stream(&body));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn last_user_text_extraction() {
        let body = json!({"messages": [
            {"role": "user", "content": "old"},
            {"role": "user", "content": [{"type": "text", "text": "问题A"}, {"type": "text", "text": "问题B"}]}
        ]});
        assert_eq!(extract_last_user_text(&body, Protocol::OpenAI), "问题A问题B");
        let body = json!({"input": "纯字符串"});
        assert_eq!(extract_last_user_text(&body, Protocol::Responses), "纯字符串");
    }
}
