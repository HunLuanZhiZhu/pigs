//! 从上游响应（JSON / SSE 全文 / SSE 增量）提取文本，以及把最终答复合成为协议正确的响应。
//!
//! 两条相反方向的流式能力都在这里：
//! - [`SseTextStream`]：**读**上游 SSE，按字节边界增量提取文本；
//! - [`StreamEncoder`]：**写**客户端 SSE，边编排边逐段发帧。

use crate::output::{Part, ToolCall};
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
    /// 上游给的停止原因（最后一个相位的值，原样透传；没有则用协议默认值）。
    stop_reason: Option<String>,
    /// 上游给的 usage（跨相位累加后的结果，原样透传）。
    usage: Option<Value>,
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
            stop_reason: None,
            usage: None,
        }
    }

    /// 记录上游给的停止原因与 usage（原样透传，不改写；缺失时终止帧用协议默认值）。
    pub fn set_finish(&mut self, stop_reason: Option<String>, usage: Option<Value>) {
        self.stop_reason = stop_reason;
        self.usage = usage;
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

    /// 把内容序列发给客户端：`Text` 走文本块，工具调用与其它原生块**原样**发出去。
    ///
    /// 流式路径下 `Text` 已经在增量阶段发过了，调用方要先把 `Text` 摘掉再传进来。
    pub fn push_parts(&mut self, parts: &[Part]) -> String {
        if parts.is_empty() {
            return String::new();
        }
        // 发原生内容前先把正在进行的文本段收尾（Anthropic/Responses 的块要闭合）
        let mut frames = self.end_pig();
        for part in parts {
            frames.push_str(&match part {
                Part::Text(text) => self.push_text(text),
                Part::ToolCall(call) => self.push_tool_call(call),
                Part::Reasoning(text) => self.push_reasoning(text),
                Part::Native(block) => self.push_native(block),
            });
        }
        frames
    }

    /// 一段思考文本（协议原生字段名）。
    fn push_reasoning(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        match self.protocol {
            // Chat：delta.reasoning_content（各家通用叫法）
            Protocol::OpenAI => data_frame(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [{
                    "index": 0,
                    "delta": {"reasoning_content": text},
                    "finish_reason": Value::Null
                }]
            })),
            Protocol::Anthropic => format!(
                "{}{}{}{}",
                event_frame(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": self.next_index,
                        "content_block": {"type": "thinking", "thinking": ""}
                    })
                ),
                event_frame(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": self.next_index,
                        "delta": {"type": "thinking_delta", "thinking": text}
                    })
                ),
                event_frame(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": self.next_index,
                        "delta": {"type": "signature_delta", "signature": ""}
                    })
                ),
                event_frame(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": self.next_index})
                )
            ),
            Protocol::Responses => self.response_event(
                "response.reasoning_summary_text.delta",
                json!({"delta": text, "output_index": self.next_index, "summary_index": 0}),
            ),
        }
    }

    /// 一个原生内容块 / output item：原样发出去，不解释、不改写。
    fn push_native(&mut self, block: &Value) -> String {
        let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match self.protocol {
            Protocol::Anthropic => {
                let index = self.next_index;
                self.next_index += 1;
                let mut frames = String::new();
                if kind == "thinking" {
                    // 思考块按协议的三段式发：start（空 thinking）→ thinking_delta → signature_delta → stop
                    frames.push_str(&event_frame(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": {"type": "thinking", "thinking": ""}
                        }),
                    ));
                    frames.push_str(&event_frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {
                                "type": "thinking_delta",
                                "thinking": block.get("thinking").cloned().unwrap_or(json!(""))
                            }
                        }),
                    ));
                    frames.push_str(&event_frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {
                                "type": "signature_delta",
                                "signature": block.get("signature").cloned().unwrap_or(json!(""))
                            }
                        }),
                    ));
                } else {
                    frames.push_str(&event_frame(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": block
                        }),
                    ));
                }
                frames.push_str(&event_frame(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
                frames
            }
            Protocol::Responses => {
                let output_index = self.next_index;
                self.next_index += 1;
                let mut frames = self.response_event(
                    "response.output_item.added",
                    json!({"output_index": output_index, "item": block}),
                );
                frames.push_str(&self.response_event(
                    "response.output_item.done",
                    json!({"output_index": output_index, "item": block}),
                ));
                self.output.push(block.clone());
                frames
            }
            // Chat 没有内容块的概念：原生块无处安放，只能不发（并在文档里登记）
            Protocol::OpenAI => String::new(),
        }
    }

    /// 单个工具调用的原生帧。
    fn push_tool_call(&mut self, call: &ToolCall) -> String {
        if self.stop_reason.is_none() {
            self.stop_reason = Some(match self.protocol {
                Protocol::OpenAI => "tool_calls".into(),
                Protocol::Anthropic => "tool_use".into(),
                Protocol::Responses => String::new(),
            });
        }
        match self.protocol {
            Protocol::OpenAI => {
                let mut native = call.native.clone();
                if let Some(obj) = native.as_object_mut() {
                    obj.insert("index".into(), json!(self.next_index));
                }
                self.next_index += 1;
                data_frame(json!({
                    "id": self.id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [{"index": 0, "delta": {"tool_calls": [native]}, "finish_reason": Value::Null}]
                }))
            }
            Protocol::Anthropic => {
                let index = self.next_index;
                self.next_index += 1;
                let mut block = call.native.clone();
                if let Some(obj) = block.as_object_mut() {
                    obj.insert("input".into(), json!({}));
                }
                format!(
                    "{}{}{}",
                    event_frame(
                        "content_block_start",
                        json!({"type": "content_block_start", "index": index, "content_block": block}),
                    ),
                    event_frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "input_json_delta", "partial_json": call.arguments_json()}
                        }),
                    ),
                    event_frame(
                        "content_block_stop",
                        json!({"type": "content_block_stop", "index": index}),
                    )
                )
            }
            Protocol::Responses => {
                let output_index = self.next_index;
                self.next_index += 1;
                let mut frames = self.response_event(
                    "response.output_item.added",
                    json!({"output_index": output_index, "item": call.native}),
                );
                frames.push_str(&self.response_event(
                    "response.output_item.done",
                    json!({"output_index": output_index, "item": call.native}),
                ));
                self.output.push(call.native.clone());
                frames
            }
        }
    }

    /// 发协议终止帧（成功的完整序列）。
    pub fn finish(&mut self) -> String {
        // 停止原因与 usage 一律用上游给的值；上游没给才退回协议默认
        let stop_reason = self
            .stop_reason
            .clone()
            .unwrap_or_else(|| match self.protocol {
                Protocol::OpenAI => "stop".into(),
                Protocol::Anthropic => "end_turn".into(),
                Protocol::Responses => String::new(),
            });
        let usage = self.usage.clone().unwrap_or_else(|| json!({}));
        match self.protocol {
            Protocol::OpenAI => format!(
                "{}{}",
                data_frame(json!({
                    "id": self.id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": stop_reason}],
                    "usage": usage
                })),
                "data: [DONE]\n\n"
            ),
            Protocol::Anthropic => format!(
                "{}{}",
                event_frame(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                        "usage": usage
                    }),
                ),
                event_frame("message_stop", json!({"type": "message_stop"}))
            ),
            Protocol::Responses => {
                // Responses 用 status/incomplete_details 表达截断
                let (status, incomplete) = if stop_reason.is_empty() {
                    ("completed", Value::Null)
                } else {
                    ("incomplete", json!({"reason": stop_reason}))
                };
                let completed = json!({
                    "id": self.id,
                    "object": "response",
                    "created_at": self.created,
                    "status": status,
                    "model": self.model,
                    "output": std::mem::take(&mut self.output),
                    "usage": usage,
                    "error": Value::Null,
                    "incomplete_details": incomplete
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

/// 回给客户端的一轮内容：**按顺序**的内容序列 + 原样透传的停止原因与 usage。
///
/// `model` 是客户端请求的那个名字（带 `-pig`），原样回显；
/// `parts` 里 `Text` 走文本块，`Reasoning`/`Native`/`ToolCall` 按协议原生形状发出，一律不改写。
#[derive(Debug, Clone)]
pub struct ResponseContent<'a> {
    pub model: &'a str,
    pub parts: &'a [Part],
    pub stop_reason: Option<&'a str>,
    pub usage: Option<&'a Value>,
}

impl<'a> ResponseContent<'a> {
    /// 只有一段文本的简单构造（测试与纯文本路径用）。
    pub fn text_only(model: &'a str, text: &'a str) -> Self {
        Self {
            model,
            parts: std::slice::from_ref(Box::leak(Box::new(Part::Text(text.to_string())))),
            stop_reason: None,
            usage: None,
        }
    }

    fn usage_value(&self) -> Value {
        self.usage.cloned().unwrap_or_else(|| json!({}))
    }

    fn texts(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn reasonings(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Reasoning(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn tool_calls(&self) -> Vec<&ToolCall> {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect()
    }
}

/// 把一轮内容合成为协议正确的**非流式 JSON 响应**（客户端未要求流式时使用）。
pub fn synthesize_json(protocol: Protocol, content: &ResponseContent) -> Value {
    let usage = content.usage_value();
    let text = content.texts();
    let reasoning = content.reasonings();
    let tool_calls = content.tool_calls();
    match protocol {
        Protocol::OpenAI => {
            let stop = content.stop_reason.unwrap_or(if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            });
            let mut message = json!({"role": "assistant"});
            message["content"] = if text.is_empty() && !tool_calls.is_empty() {
                Value::Null
            } else {
                json!(text)
            };
            if !reasoning.is_empty() {
                // 协议原生思考字段：有就带上（名字与上游一致）
                message["reasoning_content"] = json!(reasoning);
            }
            if !tool_calls.is_empty() {
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| {
                        let mut native = call.native.clone();
                        if let Some(obj) = native.as_object_mut() {
                            obj.insert("index".into(), json!(index));
                        }
                        native
                    })
                    .collect();
                message["tool_calls"] = Value::Array(calls);
            }
            json!({
                "id": format!("chatcmpl-{}", Uuid::now_v7()),
                "object": "chat.completion",
                "created": now_secs(),
                "model": content.model,
                "choices": [{"index": 0, "message": message, "finish_reason": stop}],
                "usage": usage
            })
        }
        Protocol::Anthropic => {
            let mut blocks = Vec::new();
            for part in content.parts {
                match part {
                    Part::Text(text) => {
                        if !text.is_empty() {
                            blocks.push(json!({"type": "text", "text": text}));
                        }
                    }
                    Part::Reasoning(text) => {
                        blocks.push(json!({"type": "thinking", "thinking": text}));
                    }
                    Part::ToolCall(call) => blocks.push(call.native.clone()),
                    Part::Native(block) => blocks.push(block.clone()),
                }
            }
            let stop = content.stop_reason.unwrap_or(if tool_calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            });
            json!({
                "id": format!("msg_{}", Uuid::now_v7()),
                "type": "message",
                "role": "assistant",
                "model": content.model,
                "content": blocks,
                "stop_reason": stop,
                "stop_sequence": Value::Null,
                "usage": usage
            })
        }
        Protocol::Responses => {
            let mut output = Vec::new();
            for part in content.parts {
                match part {
                    Part::Text(text) => {
                        if !text.is_empty() {
                            output.push(json!({
                                "type": "message",
                                "role": "assistant",
                                "status": "completed",
                                "content": [{"type": "output_text", "text": text, "annotations": []}]
                            }));
                        }
                    }
                    Part::Reasoning(text) => output.push(json!({
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": text}]
                    })),
                    Part::ToolCall(call) => output.push(call.native.clone()),
                    Part::Native(item) => output.push(item.clone()),
                }
            }
            let (status, incomplete) = match content.stop_reason {
                Some(reason) => ("incomplete", json!({"reason": reason})),
                None => ("completed", Value::Null),
            };
            json!({
                "id": format!("resp_{}", Uuid::now_v7()),
                "object": "response",
                "status": status,
                "model": content.model,
                "output": output,
                "usage": usage,
                "error": Value::Null,
                "incomplete_details": incomplete
            })
        }
    }
}

/// 把一轮内容合成为**完整的 SSE 流文本**（客户端要求流式时使用）。
/// 与实时流式走同一个编码器，保证两条路径的事件形状完全一致。
pub fn synthesize_sse(protocol: Protocol, content: &ResponseContent) -> String {
    let mut encoder = StreamEncoder::new(protocol, content.model);
    encoder.set_finish(
        content.stop_reason.map(String::from),
        content.usage.cloned(),
    );
    let mut frames = encoder.start();
    frames.push_str(&encoder.push_parts(content.parts));
    frames.push_str(&encoder.finish());
    frames
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
        let openai = synthesize_json(Protocol::OpenAI, &ResponseContent::text_only("m", "t"));
        assert_eq!(openai["choices"][0]["message"]["content"], "t");
        assert_eq!(openai["choices"][0]["finish_reason"], "stop");
        let anthropic = synthesize_json(Protocol::Anthropic, &ResponseContent::text_only("m", "t"));
        assert_eq!(anthropic["content"][0]["text"], "t");
        assert_eq!(anthropic["stop_reason"], "end_turn");
        let responses = synthesize_json(Protocol::Responses, &ResponseContent::text_only("m", "t"));
        assert_eq!(responses["output"][0]["content"][0]["text"], "t");
        assert_eq!(responses["status"], "completed");
    }

    /// 思考内容必须原样交给客户端：Anthropic 的 thinking 块、Responses 的 reasoning 条目、
    /// Chat 的 reasoning_content 字段，一个都不许丢。
    #[test]
    fn thinking_reaches_the_client() {
        // Anthropic：thinking 块按顺序排在文本前面，签名一起带走
        let parts = vec![
            Part::Native(json!({"type": "thinking", "thinking": "先想", "signature": "sig"})),
            Part::Text("答案".into()),
        ];
        let content = ResponseContent {
            model: "claude-x-pig",
            parts: &parts,
            stop_reason: Some("end_turn"),
            usage: None,
        };
        let body = synthesize_json(Protocol::Anthropic, &content);
        assert_eq!(body["content"][0]["type"], "thinking");
        assert_eq!(body["content"][0]["thinking"], "先想");
        assert_eq!(body["content"][0]["signature"], "sig");
        assert_eq!(body["content"][1]["text"], "答案");

        let sse = synthesize_sse(Protocol::Anthropic, &content);
        assert!(sse.contains("thinking_delta"), "流式也要发思考增量");
        assert!(sse.contains("signature_delta"));
        let back = crate::output::parse_sse_output(Protocol::Anthropic, &sse);
        assert!(matches!(&back.parts[0], Part::Native(v) if v["thinking"] == "先想"));

        // Chat：思考走 reasoning_content
        let parts = vec![
            Part::Reasoning("想过了".into()),
            Part::Text("答案".into()),
        ];
        let content = ResponseContent {
            model: "gpt-x-pig",
            parts: &parts,
            stop_reason: Some("stop"),
            usage: None,
        };
        let body = synthesize_json(Protocol::OpenAI, &content);
        assert_eq!(body["choices"][0]["message"]["reasoning_content"], "想过了");
        assert_eq!(body["choices"][0]["message"]["content"], "答案");
        let sse = synthesize_sse(Protocol::OpenAI, &content);
        assert!(sse.contains("reasoning_content"));

        // Responses：reasoning 条目原样透传（含加密内容）
        let reasoning = json!({"type": "reasoning", "id": "rs_1", "encrypted_content": "blob"});
        let parts = vec![Part::Native(reasoning.clone()), Part::Text("答案".into())];
        let content = ResponseContent {
            model: "r-x-pig",
            parts: &parts,
            stop_reason: None,
            usage: None,
        };
        let body = synthesize_json(Protocol::Responses, &content);
        assert_eq!(body["output"][0], reasoning);
        let sse = synthesize_sse(Protocol::Responses, &content);
        assert!(sse.contains("blob"));
    }

    /// 上游给的 stop_reason 与 usage 必须原样透传（不许改写、不许清零）。
    #[test]
    fn synthesize_passes_through_stop_reason_and_usage() {
        let usage = json!({"input_tokens": 11, "output_tokens": 22});
        let content = ResponseContent {
            model: "m-pig",
            parts: &[Part::Text("被截断的一半".into())],
            stop_reason: Some("max_tokens"),
            usage: Some(&usage),
        };
        let anthropic = synthesize_json(Protocol::Anthropic, &content);
        assert_eq!(anthropic["stop_reason"], "max_tokens");
        assert_eq!(anthropic["usage"], usage);
        assert_eq!(anthropic["model"], "m-pig");

        let openai = synthesize_json(Protocol::OpenAI, &content);
        assert_eq!(openai["choices"][0]["finish_reason"], "max_tokens");
        assert_eq!(openai["usage"], usage);

        let responses = synthesize_json(Protocol::Responses, &content);
        assert_eq!(responses["status"], "incomplete");
        assert_eq!(responses["incomplete_details"]["reason"], "max_tokens");
        assert_eq!(responses["usage"], usage);
    }

    /// 工具调用：三协议的非流式响应都必须带上原生调用，finish_reason 说 tool_calls。
    #[test]
    fn synthesize_json_with_tool_calls() {
        let calls = [ToolCall {
            id: "call_1".into(),
            name: "Bash".into(),
            arguments: Value::String("{\"command\":\"ls\"}".into()),
            native: json!({"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"ls\"}"}}),
        }];
        let content = ResponseContent {
            model: "m",
            parts: &[Part::ToolCall(calls[0].clone())],
            stop_reason: Some("tool_calls"),
            usage: None,
        };
        let openai = synthesize_json(Protocol::OpenAI, &content);
        assert!(openai["choices"][0]["message"]["content"].is_null());
        assert_eq!(openai["choices"][0]["message"]["tool_calls"][0]["id"], "call_1");
        assert_eq!(openai["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn synthesize_sse_roundtrips_through_extractor() {
        for (p, done) in [
            (Protocol::OpenAI, "[DONE]"),
            (Protocol::Anthropic, "message_stop"),
            (Protocol::Responses, "response.completed"),
        ] {
            let content = ResponseContent::text_only("m", "最终文本");
            let sse = synthesize_sse(p, &content);
            assert!(sse.contains(done), "{p:?} 缺少结束标记");
            assert_eq!(extract_sse_text(p, &sse).unwrap(), "最终文本");
        }
    }

    /// 流式工具调用：回吐的帧必须能被自己的解析器还原成同一次调用。
    #[test]
    fn encoder_emits_tool_calls_that_round_trip() {
        for protocol in [Protocol::OpenAI, Protocol::Anthropic, Protocol::Responses] {
            let native = match protocol {
                Protocol::OpenAI => json!({
                    "id": "call_1", "type": "function",
                    "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}
                }),
                Protocol::Anthropic => json!({
                    "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}
                }),
                Protocol::Responses => json!({
                    "type": "function_call", "call_id": "fc_1", "name": "Bash", "arguments": "{\"command\":\"ls\"}"
                }),
            };
            let calls = [ToolCall {
                id: match protocol {
                    Protocol::OpenAI => "call_1".into(),
                    Protocol::Anthropic => "toolu_1".into(),
                    Protocol::Responses => "fc_1".into(),
                },
                name: "Bash".into(),
                arguments: Value::String("{\"command\":\"ls\"}".into()),
                native,
            }];
            let mut encoder = StreamEncoder::new(protocol, "gpt-x-pig");
            encoder.set_finish(Some("tool_calls".into()), None);
            let mut frames = encoder.start();
            frames.push_str(&encoder.push_text("我先看一下"));
            frames.push_str(&encoder.push_parts(&calls.iter().cloned().map(Part::ToolCall).collect::<Vec<_>>()));
            frames.push_str(&encoder.finish());

            let parsed = crate::output::parse_sse_output(protocol, &frames);
            assert_eq!(parsed.tool_calls.len(), 1, "{protocol:?} 调用丢失");
            assert_eq!(parsed.tool_calls[0].name, "Bash");
            assert_eq!(parsed.tool_calls[0].arguments_json(), "{\"command\":\"ls\"}");
            assert_eq!(parsed.text, "我先看一下", "{protocol:?} 文本丢失");
            // 终止帧必须说"有工具调用"（三协议各自的表达）
            match protocol {
                Protocol::OpenAI => assert!(frames.contains("\"tool_calls\"")),
                Protocol::Anthropic => assert!(frames.contains("tool_use")),
                Protocol::Responses => assert!(frames.contains("function_call")),
            }
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
