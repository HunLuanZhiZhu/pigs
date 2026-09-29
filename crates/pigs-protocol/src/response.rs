//! 从上游响应（JSON / SSE 全文 / SSE 增量）提取文本，以及把最终答复合成为协议正确的响应。
//!
//! 两条相反方向的流式能力都在这里：
//! - [`SseTextStream`]：**读**上游 SSE，按字节边界增量提取文本；
//! - [`StreamEncoder`]：**写**客户端 SSE，边编排边逐段发帧。

use crate::route::Protocol;
use serde_json::{json, Value};
use uuid::Uuid;

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

/// 从单条 SSE 负载提取文本增量（按协议识别字段；其它事件返回 None）。
fn sse_payload_text(protocol: Protocol, payload: &Value) -> Option<String> {
    let ty = payload.get("type").and_then(|t| t.as_str());
    match protocol {
        // Anthropic: content_block_delta → delta.text
        Protocol::Anthropic => (ty == Some("content_block_delta"))
            .then(|| payload.pointer("/delta/text").and_then(|t| t.as_str()))
            .flatten()
            .map(String::from),
        // OpenAI Chat: choices[0].delta.content
        Protocol::OpenAI => payload
            .pointer("/choices/0/delta/content")
            .and_then(|t| t.as_str())
            .map(String::from),
        // Responses: response.output_text.delta → delta
        Protocol::Responses => (ty == Some("response.output_text.delta"))
            .then(|| payload.get("delta").and_then(|t| t.as_str()))
            .flatten()
            .map(String::from),
    }
}

/// 解析一行 SSE（不含换行符）：`data:` 行走协议解析，其余行（`event:` / 注释 / 空行）忽略。
fn sse_line_text(protocol: Protocol, line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim_start();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    let value: Value = serde_json::from_str(data).ok()?;
    sse_payload_text(protocol, &value)
}

/// 增量 SSE 文本提取器：喂入任意字节边界的响应块，吐出本次新增的文本。
///
/// 上游 SSE 的块边界可以落在任何地方——一行中间，甚至一个中文字符的中间——
/// 所以这里用字节缓冲按行切分：只处理完整行，尾部残缺字节留到下一次。
#[derive(Debug, Clone)]
pub struct SseTextStream {
    protocol: Protocol,
    buf: Vec<u8>,
}

impl SseTextStream {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            buf: Vec::new(),
        }
    }

    /// 喂入一段响应字节，返回本次新增的文本（可能为空串）。
    pub fn push(&mut self, chunk: &[u8]) -> String {
        self.buf.extend_from_slice(chunk);
        let mut out = String::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(text) = sse_line_text(self.protocol, line.trim_end_matches(['\r', '\n'])) {
                out.push_str(&text);
            }
        }
        out
    }

    /// 流结束：处理末尾没有换行的残留行。
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.buf);
        let line = String::from_utf8_lossy(&rest);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return String::new();
        }
        sse_line_text(self.protocol, line).unwrap_or_default()
    }
}

/// 从 SSE 全文（多个 `data:` 行）提取助手文本。
pub fn extract_sse_text(protocol: Protocol, sse: &str) -> Option<String> {
    let mut stream = SseTextStream::new(protocol);
    let mut text = stream.push(sse.as_bytes());
    text.push_str(&stream.finish());
    (!text.is_empty()).then_some(text)
}

/// 有状态的客户端 SSE 编码器：编排还在进行时就逐段发帧。
///
/// 生命周期：`start` →（`push_text` → `end_pig`）* → `finish`；出错走 `error`。
/// 每只 pig 的文本是一个独立的块（Anthropic 的 content_block / Responses 的
/// message item），相位之间自动补一个空行；控制标记在编排层已剥掉，这里不管。
pub struct StreamEncoder {
    protocol: Protocol,
    model: String,
    id: String,
    created: u64,
    /// Responses 协议的 sequence_number。
    sequence: u64,
    /// 下一个可用的 content_block / output_item 索引。
    next_index: usize,
    /// 是否已经发出过至少一段文本（决定是否补空行）。
    wrote_text: bool,
    /// 当前是否有打开的文本块。
    open: bool,
    /// Responses 当前 message item 的 id 与已累积文本（done 事件要回填）。
    item_id: String,
    item_text: String,
    /// Responses 已完成的 output 数组（response.completed 要用）。
    output: Vec<Value>,
}

impl StreamEncoder {
    pub fn new(protocol: Protocol, model: impl Into<String>) -> Self {
        let prefix = match protocol {
            Protocol::OpenAI => "chatcmpl",
            Protocol::Anthropic => "msg",
            Protocol::Responses => "resp",
        };
        Self {
            protocol,
            model: model.into(),
            id: response_id(prefix),
            created: now_secs(),
            sequence: 0,
            next_index: 0,
            wrote_text: false,
            open: false,
            item_id: String::new(),
            item_text: String::new(),
            output: Vec::new(),
        }
    }

    /// 发协议起始帧，让客户端流立刻开始（不依赖第一次上游增量）。
    pub fn start(&mut self) -> String {
        match self.protocol {
            Protocol::OpenAI => data_frame(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": ""},
                    "finish_reason": Value::Null
                }]
            })),
            Protocol::Anthropic => event_frame(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": self.id,
                        "type": "message",
                        "role": "assistant",
                        "model": self.model,
                        "content": [],
                        "stop_reason": Value::Null,
                        "stop_sequence": Value::Null,
                        "usage": {}
                    }
                }),
            ),
            Protocol::Responses => self.response_event(
                "response.created",
                json!({
                    "response": {
                        "id": self.id,
                        "object": "response",
                        "created_at": self.created,
                        "status": "in_progress",
                        "model": self.model,
                        "output": []
                    }
                }),
            ),
        }
    }

    /// 打开一只 pig 的文本块（相位间补空行）。
    fn open_block(&mut self) -> String {
        let boundary = if self.wrote_text { "\n\n" } else { "" };
        self.open = true;
        match self.protocol {
            Protocol::OpenAI => {
                // Chat 没有块开始事件，空行作为普通增量发出即可
                if boundary.is_empty() {
                    String::new()
                } else {
                    self.chat_delta(boundary)
                }
            }
            Protocol::Anthropic => {
                let index = self.next_index;
                self.next_index += 1;
                let mut frames = event_frame(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "text", "text": ""}
                    }),
                );
                if !boundary.is_empty() {
                    frames.push_str(&event_frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "text_delta", "text": boundary}
                        }),
                    ));
                }
                frames
            }
            Protocol::Responses => {
                let output_index = self.next_index;
                self.next_index += 1;
                self.item_id = response_id("msg");
                self.item_text = boundary.to_string();
                let mut frames = self.response_event(
                    "response.output_item.added",
                    json!({
                        "output_index": output_index,
                        "item": {
                            "type": "message",
                            "id": self.item_id,
                            "role": "assistant",
                            "status": "in_progress",
                            "content": []
                        }
                    }),
                );
                frames.push_str(&self.response_event(
                    "response.content_part.added",
                    json!({
                        "item_id": self.item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": {"type": "output_text", "text": "", "annotations": []}
                    }),
                ));
                if !boundary.is_empty() {
                    frames.push_str(&self.responses_delta(boundary));
                }
                frames
            }
        }
    }

    /// 发出一段可见文本增量（需要时先开块）。
    pub fn push_text(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut frames = if self.open {
            String::new()
        } else {
            self.open_block()
        };
        self.wrote_text = true;
        frames.push_str(&match self.protocol {
            Protocol::OpenAI => self.chat_delta(text),
            Protocol::Anthropic => {
                let index = self.next_index.saturating_sub(1);
                event_frame(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                )
            }
            Protocol::Responses => self.responses_delta(text),
        });
        frames
    }

    /// 关闭当前 pig 的文本块。
    pub fn end_pig(&mut self) -> String {
        if !self.open {
            return String::new();
        }
        self.open = false;
        match self.protocol {
            // Chat 没有块结束事件
            Protocol::OpenAI => String::new(),
            Protocol::Anthropic => {
                let index = self.next_index.saturating_sub(1);
                event_frame(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                )
            }
            Protocol::Responses => self.responses_item_done(),
        }
    }

    /// 发协议终止帧（成功的完整序列）。
    pub fn finish(&mut self) -> String {
        match self.protocol {
            Protocol::OpenAI => format!(
                "{}{}",
                data_frame(json!({
                    "id": self.id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
                })),
                "data: [DONE]\n\n"
            ),
            Protocol::Anthropic => format!(
                "{}{}",
                event_frame(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn", "stop_sequence": Value::Null},
                        "usage": {}
                    }),
                ),
                event_frame("message_stop", json!({"type": "message_stop"}))
            ),
            Protocol::Responses => {
                let completed = json!({
                    "id": self.id,
                    "object": "response",
                    "created_at": self.created,
                    "status": "completed",
                    "model": self.model,
                    "output": std::mem::take(&mut self.output),
                    "usage": {},
                    "error": Value::Null,
                    "incomplete_details": Value::Null
                });
                self.response_event("response.completed", json!({"response": completed}))
            }
        }
    }

    /// 发流内错误帧（不含成功终止帧；调用前会先关掉仍开着的块）。
    pub fn error(&mut self, message: &str) -> String {
        let mut frames = self.end_pig();
        frames.push_str(&match self.protocol {
            Protocol::OpenAI => data_frame(json!({
                "error": {"message": message, "type": "pigs_phase_error"}
            })),
            Protocol::Anthropic => event_frame(
                "error",
                json!({
                    "type": "error",
                    "error": {"type": "pigs_phase_error", "message": message}
                }),
            ),
            Protocol::Responses => self.response_event(
                "response.failed",
                json!({
                    "response": {
                        "id": self.id,
                        "object": "response",
                        "status": "failed",
                        "error": {"code": "pigs_phase_error", "message": message}
                    }
                }),
            ),
        });
        frames
    }

    /// Chat 协议的一段文本增量帧。
    fn chat_delta(&self, text: &str) -> String {
        data_frame(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": {"content": text},
                "finish_reason": Value::Null
            }]
        }))
    }

    /// Responses 协议的一段文本增量帧（同时累积文本供 done 事件回填）。
    fn responses_delta(&mut self, text: &str) -> String {
        self.item_text.push_str(text);
        let output_index = self.next_index.saturating_sub(1);
        let item_id = self.item_id.clone();
        self.response_event(
            "response.output_text.delta",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "delta": text
            }),
        )
    }

    /// Responses 协议：结束当前 message item（output_text.done → content_part.done → output_item.done）。
    fn responses_item_done(&mut self) -> String {
        let output_index = self.next_index.saturating_sub(1);
        let item_id = std::mem::take(&mut self.item_id);
        let text = std::mem::take(&mut self.item_text);
        if item_id.is_empty() {
            return String::new();
        }
        let item = json!({
            "type": "message",
            "id": item_id,
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}]
        });
        self.output.push(item.clone());
        let mut frames = self.response_event(
            "response.output_text.done",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": text
            }),
        );
        frames.push_str(&self.response_event(
            "response.content_part.done",
            json!({
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": {"type": "output_text", "text": text, "annotations": []}
            }),
        ));
        frames.push_str(&self.response_event(
            "response.output_item.done",
            json!({"output_index": output_index, "item": item}),
        ));
        frames
    }

    /// Responses 协议：构造带 type + sequence_number 的事件帧。
    fn response_event(&mut self, event_type: &str, extra: Value) -> String {
        let mut value = extra.as_object().cloned().unwrap_or_default();
        value.insert("type".into(), json!(event_type));
        value.insert("sequence_number".into(), json!(self.sequence));
        self.sequence += 1;
        event_frame(event_type, Value::Object(value))
    }
}

/// 把最终文本合成为协议正确的**非流式 JSON 响应**（客户端未要求流式时使用）。
pub fn synthesize_json(protocol: Protocol, model: &str, text: &str) -> Value {
    match protocol {
        Protocol::OpenAI => json!({
            "id": format!("chatcmpl-{}", Uuid::now_v7()),
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
            "id": format!("msg_{}", Uuid::now_v7()),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "stop_sequence": Value::Null,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        }),
        Protocol::Responses => json!({
            "id": format!("resp_{}", Uuid::now_v7()),
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
/// 与实时流式走同一个编码器，保证两条路径的事件形状完全一致。
pub fn synthesize_sse(protocol: Protocol, model: &str, text: &str) -> String {
    let mut encoder = StreamEncoder::new(protocol, model);
    format!(
        "{}{}{}",
        encoder.start(),
        encoder.push_text(text),
        encoder.finish()
    )
}

/// 生成一个 `{prefix}_{uuid}` 形式的响应 ID。
fn response_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 构造一个 `data: {value}\n\n` 形式的 SSE 数据帧。
fn data_frame(value: Value) -> String {
    format!("data: {value}\n\n")
}

/// 构造一个 `event: {event}\ndata: {value}\n\n` 形式的 SSE 事件帧。
fn event_frame(event: &str, value: Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
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
        assert_eq!(extract_sse_text(Protocol::Responses, &responses).unwrap(), "ok");
    }

    /// 增量提取：同一段 SSE 不论按什么字节边界切开，结果必须与整段一致
    /// （切点落在多字节中文字符中间也要活下来）。
    #[test]
    fn incremental_sse_extraction_is_boundary_agnostic() {
        let sse = "event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"中文增量\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"尾巴\"}}\n\n";
        let whole = extract_sse_text(Protocol::Anthropic, sse).unwrap();
        assert_eq!(whole, "中文增量尾巴");

        for step in 1..=7 {
            let mut stream = SseTextStream::new(Protocol::Anthropic);
            let mut got = String::new();
            for chunk in sse.as_bytes().chunks(step) {
                got.push_str(&stream.push(chunk));
            }
            got.push_str(&stream.finish());
            assert_eq!(got, whole, "按 {step} 字节切片时结果不一致");
        }

        // 末尾没有换行的残留行也要收下
        let mut stream = SseTextStream::new(Protocol::OpenAI);
        let tail = "data: {\"choices\":[{\"delta\":{\"content\":\"尾\"}}]}";
        assert_eq!(stream.push(tail.as_bytes()), "");
        assert_eq!(stream.finish(), "尾");
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

    /// 编码器逐段推进：三只 pig 的文本必须按序拼出、相位间有空行、
    /// Responses 的 sequence_number 连续递增。
    #[test]
    fn encoder_emits_pigs_progressively() {
        for protocol in [Protocol::OpenAI, Protocol::Anthropic, Protocol::Responses] {
            let mut encoder = StreamEncoder::new(protocol, "gpt-x");
            let mut frames = encoder.start();
            assert!(!frames.is_empty(), "{protocol:?} 的起始帧不能为空");
            for pig_text in ["分析：需要X", "执行结果……", "验收通过"] {
                frames.push_str(&encoder.push_text(&pig_text[..3]));
                frames.push_str(&encoder.push_text(&pig_text[3..]));
                frames.push_str(&encoder.end_pig());
            }
            frames.push_str(&encoder.finish());
            assert_eq!(
                extract_sse_text(protocol, &frames).unwrap(),
                "分析：需要X\n\n执行结果……\n\n验收通过",
                "{protocol:?} 的相位拼接不一致"
            );
            if protocol == Protocol::Responses {
                // sequence_number 从 0 起连续
                let seqs: Vec<u64> = frames
                    .lines()
                    .filter_map(|l| l.strip_prefix("data: "))
                    .filter_map(|d| serde_json::from_str::<Value>(d).ok())
                    .filter_map(|v| v.get("sequence_number").and_then(|s| s.as_u64()))
                    .collect();
                assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
                assert!(seqs.len() > 10, "Responses 事件序列偏少: {}", seqs.len());
            }
        }
    }

    /// 流内错误帧不是成功终止帧，且 Anthropic/Responses 会先关掉打开的块。
    #[test]
    fn encoder_error_closes_open_block() {
        let mut encoder = StreamEncoder::new(Protocol::Anthropic, "m");
        let mut frames = encoder.push_text("半截");
        frames.push_str(&encoder.error("上游挂了"));
        assert!(frames.contains("content_block_stop"));
        assert!(frames.contains("\"type\":\"error\""));
        assert!(!frames.contains("message_stop"));
    }
}
