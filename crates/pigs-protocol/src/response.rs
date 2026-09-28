//! 从上游响应（JSON 或 SSE 全文）提取文本，以及把最终答复合成为协议正确的响应。

use crate::route::Protocol;
use crate::Error;
use serde_json::{json, Value};

/// 从非流式 JSON 响应提取助手文本。
pub fn extract_response_text(protocol: Protocol, body: &Value) -> Option<String> {
    let join_blocks = |blocks: &Value, key: &str| -> Option<String> {
        blocks.as_array().map(|items| {
            items
                .iter()
                .filter_map(|b| {
                    let ty = b.get("type").and_then(|t| t.as_str())?;
                    if ty.ends_with("text") {
                        b.get("text").and_then(|t| t.as_str()).map(str::to_string)
                    } else {
                        // key 参数用于调试定位，不影响提取
                        let _ = key;
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
    };
    match protocol {
        Protocol::OpenAI => body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| match c {
                Value::String(s) => Some(s.clone()),
                Value::Array(_) => join_blocks(c, "content"),
                _ => None,
            }),
        Protocol::Anthropic => body
            .get("content")
            .and_then(|c| join_blocks(c, "content")),
        Protocol::Responses => body
            .get("output")
            .and_then(|o| o.as_array())
            .and_then(|items| {
                let texts: Vec<String> = items
                    .iter()
                    .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("message"))
                    .filter_map(|i| i.get("content").and_then(|c| join_blocks(c, "output")))
                    .collect();
                (!texts.is_empty()).then(|| texts.join(""))
            })
            .or_else(|| {
                // 某些网关直接给扁平字段
                body.get("output_text").and_then(|t| t.as_str()).map(String::from)
            }),
    }
}

/// 从 SSE 全文（多个 `data:` 行）提取助手文本，按协议识别增量字段。
pub fn extract_sse_text(protocol: Protocol, sse: &str) -> Option<String> {
    let mut text = String::new();
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue; // event:/空行/注释行：跳过（不能用 ? —— 会把整条流短路成 None）
        };
        let data = data.trim_start();
        if data == "[DONE]" || data.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match protocol {
            // Anthropic: content_block_delta → delta.text
            Protocol::Anthropic => {
                if v.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
                    if let Some(t) = v.pointer("/delta/text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
            }
            // OpenAI chat: choices[0].delta.content
            Protocol::OpenAI => {
                if let Some(t) = v
                    .pointer("/choices/0/delta/content")
                    .and_then(|t| t.as_str())
                {
                    text.push_str(t);
                }
            }
            // Responses: response.output_text.delta → delta
            Protocol::Responses => {
                if v.get("type").and_then(|t| t.as_str()) == Some("response.output_text.delta") {
                    if let Some(t) = v.get("delta").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

/// 把最终文本合成为协议正确的**非流式 JSON 响应**（客户端未要求流式时使用）。
pub fn synthesize_json(protocol: Protocol, model: &str, text: &str) -> Value {
    match protocol {
        Protocol::OpenAI => json!({
            "id": format!("chatcmpl-{}", uuid::Uuid::now_v7()),
            "object": "chat.completion",
            "created": now_secs(),
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        }),
        Protocol::Anthropic => json!({
            "id": format!("msg_{}", uuid::Uuid::now_v7()),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "stop_sequence": Value::Null,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }),
        Protocol::Responses => json!({
            "id": format!("resp_{}", uuid::Uuid::now_v7()),
            "object": "response",
            "status": "completed",
            "model": model,
            "output": [{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": []}]
            }],
            "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}
        }),
    }
}

/// 把最终文本合成为**完整的 SSE 流文本**（客户端要求流式时使用）。
/// 事件序列取各协议客户端能接受的最小合规集；文本以单个 delta 一次性给出。
pub fn synthesize_sse(protocol: Protocol, model: &str, text: &str) -> String {
    match protocol {
        Protocol::OpenAI => {
            let id = format!("chatcmpl-{}", uuid::Uuid::now_v7());
            let base = json!({
                "id": id, "object": "chat.completion.chunk", "created": now_secs(), "model": model
            });
            let mut first = base.clone();
            first["choices"] = json!([{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": Value::Null}]);
            let mut mid = base;
            mid["choices"] = json!([{"index": 0, "delta": {"content": text}, "finish_reason": Value::Null}]);
            format!(
                "data: {}\n\ndata: {}\n\ndata: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n",
                first, mid
            )
        }
        Protocol::Anthropic => {
            let id = format!("msg_{}", uuid::Uuid::now_v7());
            let esc = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
            format!(
                concat!(
                    "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"{id}\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"{model}\",\"content\":[],\"stop_reason\":null,\"usage\":{{\"input_tokens\":0,\"output_tokens\":0}}}}}}\n\n",
                    "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
                    "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{esc}}}}}\n\n",
                    "event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
                    "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":0}}}}\n\n",
                    "event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
                ),
                id = id,
                model = model,
                esc = esc,
            )
        }
        Protocol::Responses => {
            let esc = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
            format!(
                concat!(
                    "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":{esc},\"output_index\":0,\"content_index\":0}}\n\n",
                    "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"model\":\"{model}\"}}}}\n\n"
                ),
                esc = esc,
                model = model,
            )
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Error 里 NoText 在提取失败时使用；保持构造入口统一
#[allow(dead_code)]
pub(crate) fn no_text(protocol: Protocol) -> Error {
    Error::NoText(protocol)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn extracts_text_from_three_json_shapes() {
        let openai = json!({"choices":[{"message":{"role":"assistant","content":"答案A"}}]});
        assert_eq!(extract_response_text(Protocol::OpenAI, &openai).unwrap(), "答案A");

        let anthropic = json!({"content":[
            {"type":"text","text":"答案"},
            {"type":"text","text":"B"},
            {"type":"tool_use","id":"t1"}
        ]});
        assert_eq!(extract_response_text(Protocol::Anthropic, &anthropic).unwrap(), "答案B");

        let responses = json!({"output":[
            {"type":"message","content":[{"type":"output_text","text":"答案C"}]}
        ]});
        assert_eq!(extract_response_text(Protocol::Responses, &responses).unwrap(), "答案C");

        // 扁平 output_text 兜底
        let flat = json!({"output_text":"答案D"});
        assert_eq!(extract_response_text(Protocol::Responses, &flat).unwrap(), "答案D");
    }

    #[test]
    fn extracts_text_from_sse_streams() {
        let anthropic = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"好\"}}\n\n";
        assert_eq!(extract_sse_text(Protocol::Anthropic, anthropic).unwrap(), "你好");

        let openai = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\ndata: [DONE]\n\n";
        assert_eq!(extract_sse_text(Protocol::OpenAI, openai).unwrap(), "Hi!");

        let responses = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        assert_eq!(extract_sse_text(Protocol::Responses, responses).unwrap(), "ok");
    }

    #[test]
    fn synthesize_json_shapes() {
        let openai = synthesize_json(Protocol::OpenAI, "m", "t");
        assert_eq!(openai["choices"][0]["message"]["content"], "t");
        let anthropic = synthesize_json(Protocol::Anthropic, "m", "t");
        assert_eq!(anthropic["content"][0]["text"], "t");
        assert_eq!(anthropic["stop_reason"], "end_turn");
        let responses = synthesize_json(Protocol::Responses, "m", "t");
        assert_eq!(responses["output"][0]["content"][0]["text"], "t");
    }

    #[test]
    fn synthesize_sse_roundtrips_through_extractor() {
        for (p, done) in [
            (Protocol::OpenAI, "[DONE]"),
            (Protocol::Anthropic, "message_stop"),
            (Protocol::Responses, "response.completed"),
        ] {
            let sse = synthesize_sse(p, "m", "最终文本");
            assert!(sse.contains(done), "{p:?} 缺少结束标记");
            assert_eq!(extract_sse_text(p, &sse).unwrap(), "最终文本");
        }
    }
}
