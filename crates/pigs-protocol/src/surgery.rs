//! 请求体手术：按协议对 JSON body 做定位与修改。
//!
//! 所有操作都基于 `serde_json::Value`，不构造 typed 模型——编排只需要改少数几个字段。

use crate::route::Protocol;
use crate::{Error, Result};
use serde_json::Value;

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

/// 强制关闭流式（编排子请求统一非流式，便于提取完整文本）。
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

/// 把最后一条 user 消息的文本替换为 `text`（编排的相位 payload）。
///
/// - OpenAI Chat：`messages` 数组中最后一条 `role=="user"` 的 `content`；
/// - Anthropic：同上（`content` 字符串与块数组都合法，统一替换为字符串）；
/// - Responses：`input` 为字符串则整体替换；为数组则改最后一条 user 项的
///   `content`（`input_text` 块），找不到 user 项则追加一条。
pub fn replace_last_user_text(body: &mut Value, protocol: Protocol, text: &str) -> Result<()> {
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
            if let Some(obj) = last_user.as_object_mut() {
                obj.insert("content".into(), Value::String(text.into()));
            }
            Ok(())
        }
        Protocol::Responses => {
            let obj = body.as_object_mut().ok_or(Error::NoUserMessage(protocol))?;
            match obj.get_mut("input") {
                Some(Value::String(s)) => {
                    *s = text.into();
                    Ok(())
                }
                Some(Value::Array(items)) => {
                    if let Some(last_user) = items
                        .iter_mut()
                        .rev()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                    {
                        if let Some(mo) = last_user.as_object_mut() {
                            mo.insert(
                                "content".into(),
                                serde_json::json!([{ "type": "input_text", "text": text }]),
                            );
                        }
                        Ok(())
                    } else {
                        items.push(serde_json::json!({
                            "role": "user",
                            "content": [{ "type": "input_text", "text": text }]
                        }));
                        Ok(())
                    }
                }
                _ => {
                    // 无 input 字段（或非字符串/数组）→ 追加字符串形式
                    obj.insert("input".into(), Value::String(text.into()));
                    Ok(())
                }
            }
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
                .filter_map(|p| {
                    p.get("text").and_then(|t| t.as_str()).or_else(|| {
                        // Responses 的 input_text 块 text 字段同名，无需特判
                        None::<&str>
                    })
                })
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_replaces_last_user_string_content() {
        let mut body = json!({
            "model": "gpt-x",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"},
                {"role": "user", "content": "do the task"}
            ]
        });
        replace_last_user_text(&mut body, Protocol::OpenAI, "PAYLOAD").unwrap();
        let msgs = body.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs[1]["content"], "hello");
        assert_eq!(msgs[3]["content"], "PAYLOAD");
    }

    #[test]
    fn anthropic_replaces_block_content_with_string() {
        let mut body = json!({
            "model": "claude-x",
            "system": "sys",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "q"}]}
            ]
        });
        replace_last_user_text(&mut body, Protocol::Anthropic, "PAYLOAD").unwrap();
        assert_eq!(body["messages"][0]["content"], "PAYLOAD");
        // system 不受影响
        assert_eq!(body["system"], "sys");
    }

    #[test]
    fn responses_string_and_array_input() {
        let mut body = json!({"model": "m", "input": "plain"});
        replace_last_user_text(&mut body, Protocol::Responses, "P1").unwrap();
        assert_eq!(body["input"], "P1");

        let mut body = json!({"model": "m", "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "q"}]}
        ]});
        replace_last_user_text(&mut body, Protocol::Responses, "P2").unwrap();
        assert_eq!(body["input"][0]["content"][0]["text"], "P2");

        // 无 input → 追加字符串
        let mut body = json!({"model": "m"});
        replace_last_user_text(&mut body, Protocol::Responses, "P3").unwrap();
        assert_eq!(body["input"], "P3");
    }

    #[test]
    fn no_user_message_is_an_error() {
        let mut body = json!({"model": "m", "messages": [
            {"role": "assistant", "content": "hi"}
        ]});
        let err = replace_last_user_text(&mut body, Protocol::OpenAI, "P").unwrap_err();
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
        assert_eq!(has_client_stream(&body), false);
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
