//! 请求体修改：定位并编辑各协议的 JSON 数据。 / Request rewriting: locate and modify protocol-specific JSON fields.
//!
//! 基于 `serde_json::Value` 修改少数字段，不构建完整强类型模型。 / Edit a few fields via `serde_json::Value` without a full typed schema.
//!
//! 相位信息按历史协议方式传递。 / Phase context follows the historical protocol behavior.
//! - Pre/Exec 指令追加到最后一条用户消息。 / Pre/Exec instructions append to the last user message.
//!   保留图片等非文本块。 / Preserve images and other nontext blocks.
//! - Post 将前一相位产出作为助手消息写回。 / Post restores the prior phase output as an assistant message.
//!   然后以新用户消息提供验收指令。 / It then adds a new user message with the verification instruction.

use crate::route::Protocol;
use crate::{Error, Result};
use serde_json::{json, Value};

/// 相位指令格式：空行、分隔线、空行、正文。 / Phase-instruction format: newline, separator, newline, instruction.
pub const SUFFIX_SEPARATOR: &str = "\n\n---\n\n";

/// 读取请求体中的模型名。 / Read the model field from the body.
pub fn get_model(body: &Value) -> Option<&str> {
    body.get("model").and_then(|m| m.as_str())
}

/// 覆盖请求体中的模型名。 / Override the model field.
pub fn set_model(body: &mut Value, model: &str) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), Value::String(model.into()));
    }
}

/// 设置请求体流式标志。 / Set the streaming flag in the request body.
pub fn set_stream(body: &mut Value, stream: bool) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".into(), Value::Bool(stream));
    }
}

/// 判断客户端是否请求流式。 / Check whether the client requested streaming.
pub fn has_client_stream(body: &Value) -> bool {
    body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false)
}

/// 移除工具定义字段，使模型不能选择工具。 / Remove tool definitions to prevent tool selection.
/// 三协议都使用 `tools`；同时移除 `tool_choice`。 / All three protocols use `tools`; also remove `tool_choice`.
pub fn strip_tools(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("tools");
        obj.remove("tool_choice");
    }
}

/// 将 Pre/Exec 指令追加到最后一条用户消息文本。 / Append Pre/Exec instructions to the last user message text.
///
/// - Chat/Anthropic：修改 `messages` 里的最后一条用户消息。 / Chat/Anthropic: modify the last user entry in `messages`.
/// - Responses：字符串输入直接追加；数组输入修改最后一条用户项。 / Responses: append to string input or modify the final user item.
///
/// 字符串内容直接追加；块数组修改最后文本块或新建文本块。 / Append to string content or the last text block, creating one if absent.
/// 图片、文档等非文本内容保持原样。 / Keep images, documents, and other nontext content unchanged.
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

/// 将相位指令追加到请求尾部。 / Append phase instructions to the request tail.
///
/// 若末条为用户消息，则在其文本尾部追加。 / Append to the final message if it is from the user.
/// 否则新建一条用户消息。 / Otherwise create a new user message.
///
/// 只追加尾部，以保持上游提示词前缀缓存命中。 / Append only at the tail to preserve upstream prompt-cache prefixes.
pub fn append_instruction(body: &mut Value, protocol: Protocol, text: &str) -> Result<()> {
    let tail_is_user = match protocol {
        Protocol::OpenAI | Protocol::Anthropic => body
            .get("messages")
            .and_then(|m| m.as_array())
            .and_then(|arr| arr.last())
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some("user"),
        Protocol::Responses => match body.get("input") {
            Some(Value::String(_)) => true,
            Some(Value::Array(items)) => items
                .last()
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("user"),
            _ => false,
        },
    };
    if tail_is_user {
        append_to_last_user_text(body, protocol, text)
    } else {
        push_user_message(body, protocol, text)
    }
}

/// 新增用户消息，用于 Post 验收指令。 / Add a user message for the Post verification instruction.
///
/// Responses 字符串输入先展开成用户消息。 / Expand string-form Responses input into a user message.
/// 以免丢失原问题。 / Preserve the original user request.
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

/// 将协议原生对话记录追加到请求尾部。 / Append protocol-native conversation items.
///
/// 用于工具续接：阶段提示已在基础请求内。 / For tool continuation: the base request already contains the phase instruction.
/// 后续只追加助手/工具记录，不重新注入阶段提示。 / Append assistant/tool history without reinjecting the phase prompt.
pub fn append_transcript_items(
    body: &mut Value,
    protocol: Protocol,
    items: &[Value],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    match protocol {
        Protocol::OpenAI | Protocol::Anthropic => {
            messages_mut(body, protocol)?.extend(items.iter().cloned());
            Ok(())
        }
        Protocol::Responses => {
            input_items_mut(body, protocol)?.extend(items.iter().cloned());
            Ok(())
        }
    }
}

/// 将上一相位产出接为助手消息。 / Restore prior-phase output as an assistant message.
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

/// 提取最后一条用户消息文本，供语言判断和日志使用。 / Extract the last user text for language detection and logging.
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

/// 读取 Chat/Anthropic 的消息数组，非法则报错。 / Read the Chat/Anthropic message array or return an error.
fn messages_mut(body: &mut Value, protocol: Protocol) -> Result<&mut Vec<Value>> {
    body.get_mut("messages")
        .and_then(|m| m.as_array_mut())
        .ok_or(Error::NoUserMessage(protocol))
}

/// 读取 Responses 输入数组；字符串输入先包装成用户消息。 / Read the Responses input array, wrapping string input as a user message.
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

/// 向字符串或文本块内容追加文本。 / Append text to string or block-array content.
fn append_to_text_content(content: &mut Value, addition: &str, protocol: Protocol) -> Result<()> {
    match content {
        // 字符串直接追加。 / Append directly to strings.
        Value::String(text) => {
            text.push_str(addition);
            Ok(())
        }
        // 块数组修改最后文本块或新建文本块。 / Use the last text block or create one.
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
        // 系统消息不受影响。 / System messages are unchanged.
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

        // 缺少 input 字段时显式报错。 / Fail explicitly when the input field is absent.
        let mut body = json!({"model": "m"});
        let err = append_to_last_user_text(&mut body, Protocol::Responses, "P").unwrap_err();
        assert!(matches!(err, Error::NoUserMessage(_)));
    }

    #[test]
    fn phase_transcript_appends_after_the_single_phase_prompt() {
        let mut chat = json!({
            "messages": [{"role": "user", "content": "任务\n\n---\n\n阶段说明"}]
        });
        append_transcript_items(
            &mut chat,
            Protocol::OpenAI,
            &[
                json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_1"}]}),
                json!({"role": "tool", "tool_call_id": "call_1", "content": "ok"}),
            ],
        )
        .unwrap();
        let messages = chat["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "tool");

        let mut responses = json!({"input": "任务\n\n---\n\n阶段说明"});
        append_transcript_items(
            &mut responses,
            Protocol::Responses,
            &[json!({"type": "function_call_output", "call_id": "fc_1", "output": "ok"})],
        )
        .unwrap();
        let input = responses["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "function_call_output");
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
        let mut body = json!({"model": "x-pigs", "stream": true, "tools": [1]});
        assert_eq!(get_model(&body), Some("x-pigs"));
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
