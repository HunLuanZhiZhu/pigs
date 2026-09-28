//! 协议原生的非流式与 SSE 响应编码。
//! Protocol-native non-streaming and SSE response encoding.
//!
//! 本模块把 `HttpTurnResult`（HTTP 相位运行时的输出）编码为三种协议的：
//! - 非流式 JSON 响应体（`encode_json`）
//! - 完整 SSE 帧序列（`encode_sse`）
//! - 流式失败帧（`encode_sse_error`）
//!
//! 此外提供有状态的 `StreamingEncoder`，用于相位执行仍在进行时逐段
//! 发出 SSE 帧（含相位边界、文本增量、工具调用、终止帧）。
//!
//! This module encodes an `HttpTurnResult` (the HTTP phase runtime's output)
//! into three protocols': non-streaming JSON body (`encode_json`); complete
//! SSE frame sequences (`encode_sse`); in-stream error frames
//! (`encode_sse_error`). It also provides a stateful `StreamingEncoder` for
//! emitting SSE frames incrementally while phase execution is still running
//! (phase boundaries, text deltas, tool calls, terminal frames).

use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::http_runtime::{HttpTurnResult, HttpTurnStatus};
use crate::protocol::{NativeToolCall, Protocol};

/// 有状态的 SSE 编码器，在相位执行仍在进行时使用。
/// Stateful SSE encoder used while phase execution is still in progress.
///
/// 调用方按生命周期顺序调用：`start` → (`phase_start` → `text_delta`* →
/// `phase_end`)* → `finish`/`abort_phase`/`error`。编码器内部维护索引、
/// 序列号和当前打开的内容块，以保证三种协议的事件结构合法。
///
/// Callers drive the lifecycle in order: `start` → (`phase_start` →
/// `text_delta`* → `phase_end`)* → `finish`/`abort_phase`/`error`. The
/// encoder tracks indices, sequence numbers, and the currently-open content
/// block so each protocol's event structure stays well-formed.
pub struct StreamingEncoder {
    /// 目标协议 / target protocol.
    protocol: Protocol,
    /// 客户端可见的模型名（通常带 -pig 后缀）/ client-visible model name (usually with -pig).
    client_model: String,
    /// 本次响应的唯一 ID / unique response ID for this stream.
    id: String,
    /// 响应创建时间戳 / response creation timestamp.
    created: i64,
    /// Responses 协议的序列号计数器 / sequence-number counter for the Responses protocol.
    sequence: u64,
    /// 下一个可用的 content-block / output-item 索引 / next available content-block / output-item index.
    next_index: usize,
    /// 是否已经发出过至少一段文本 / whether any text has been emitted yet.
    wrote_text: bool,
    /// 是否有一个待开始的相位文本（phase_start 已调用但还没 open）/ whether a phase text is pending open.
    pending_phase_text: bool,
    /// Anthropic 协议当前打开的 content_block 索引 / Anthropic currently-open content_block index.
    active_anthropic_block: Option<usize>,
    /// Responses 协议当前打开的 message item：(output_index, item_id, accumulated_text)
    /// Responses currently-open message item: (output_index, item_id, accumulated_text).
    active_response_item: Option<(usize, String, String)>,
    /// Responses 协议已完成的 output 数组（用于最终 response.completed）
    /// Responses completed output array (used in the final response.completed).
    response_output: Vec<Value>,
}

impl StreamingEncoder {
    /// 为一次客户端响应流创建编码器。
    /// Creates an encoder for one client response stream.
    pub fn new(protocol: Protocol, client_model: impl Into<String>) -> Self {
        // 按协议选择 ID 前缀 / Pick an ID prefix per protocol.
        let prefix = match protocol {
            Protocol::OpenAiChat => "chatcmpl",
            Protocol::AnthropicMessages => "msg",
            Protocol::OpenAiResponses => "resp",
        };
        Self {
            protocol,
            client_model: client_model.into(),
            id: response_id(prefix),
            created: Utc::now().timestamp(),
            sequence: 0,
            next_index: 0,
            wrote_text: false,
            pending_phase_text: false,
            active_anthropic_block: None,
            active_response_item: None,
            response_output: Vec::new(),
        }
    }

    /// 发出协议的起始帧，让 HTTP 流可以立即开始。
    /// Emits the protocol's opening frame so the HTTP stream can start immediately.
    pub fn start(&mut self) -> Vec<String> {
        match self.protocol {
            // OpenAI Chat：首帧 delta 含 role=assistant / OpenAI Chat: first delta with role=assistant.
            Protocol::OpenAiChat => vec![data_frame(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.client_model,
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": ""},
                    "finish_reason": null
                }]
            }))],
            // Anthropic：message_start 事件 / Anthropic: message_start event.
            Protocol::AnthropicMessages => vec![event_frame(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": self.id,
                        "type": "message",
                        "role": "assistant",
                        "model": self.client_model,
                        "content": [],
                        "stop_reason": null,
                        "stop_sequence": null,
                        "usage": {}
                    }
                }),
            )],
            // Responses：response.created 事件 / Responses: response.created event.
            Protocol::OpenAiResponses => {
                let event = self.response_event(
                    "response.created",
                    json!({
                        "response": {
                            "id": self.id,
                            "object": "response",
                            "created_at": self.created,
                            "status": "in_progress",
                            "model": self.client_model,
                            "output": []
                        }
                    }),
                );
                vec![event]
            }
        }
    }

    /// 标记一个相位文本开始（实际 open 推迟到第一次 text_delta）。
    /// Mark a phase text item as starting (the actual open is deferred to the first text_delta).
    pub fn phase_start(&mut self) -> Vec<String> {
        self.pending_phase_text = true;
        Vec::new()
    }

    /// 内部：真正打开一个相位文本块（处理相位间的空行边界）。
    /// Internal: actually open a phase text block (handles the blank-line boundary between phases).
    fn open_phase_text(&mut self) -> Vec<String> {
        // 相位之间用空行分隔；首个相位无需前导 / Separate phases with a blank line; no leading for the first.
        let boundary = if self.wrote_text { "\n\n" } else { "" };
        self.wrote_text = true;
        self.pending_phase_text = false;
        match self.protocol {
            Protocol::OpenAiChat => {
                // Chat 只需发出边界增量（无块开始事件）/ Chat only emits the boundary delta (no block-start event).
                if boundary.is_empty() {
                    Vec::new()
                } else {
                    self.chat_text_delta(boundary)
                }
            }
            Protocol::AnthropicMessages => {
                // Anthropic：打开一个新的 content_block / Anthropic: open a new content_block.
                let index = self.take_index();
                self.active_anthropic_block = Some(index);
                let mut frames = vec![event_frame(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "text", "text": ""}
                    }),
                )];
                if !boundary.is_empty() {
                    // 把边界作为 text_delta 发出 / Emit the boundary as a text_delta.
                    frames.push(event_frame(
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
            Protocol::OpenAiResponses => self.responses_text_start(boundary),
        }
    }

    /// 发出一段已确认安全的（无标记）可见文本增量。
    /// Emits a safe marker-free text delta for the current phase.
    pub fn text_delta(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        // 若有待开始的相位文本，先 open / If a phase text is pending, open it first.
        let mut frames = if self.pending_phase_text {
            self.open_phase_text()
        } else {
            Vec::new()
        };
        let delta = match self.protocol {
            Protocol::OpenAiChat => self.chat_text_delta(text),
            Protocol::AnthropicMessages => {
                // 必须有已打开的块才能发增量 / Need an open block to emit a delta.
                let Some(index) = self.active_anthropic_block else {
                    return frames;
                };
                vec![event_frame(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                )]
            }
            Protocol::OpenAiResponses => self.responses_text_delta(text),
        };
        frames.extend(delta);
        frames
    }

    /// 关闭当前相位文本块。
    /// Closes the current phase text item.
    pub fn phase_end(&mut self) -> Vec<String> {
        // 若 phase_start 后从未 open 过，直接清除标记即可 / If never opened, just clear the pending flag.
        if self.pending_phase_text {
            self.pending_phase_text = false;
            return Vec::new();
        }
        match self.protocol {
            // Chat 无块关闭事件 / Chat has no block-close event.
            Protocol::OpenAiChat => Vec::new(),
            Protocol::AnthropicMessages => {
                // 关闭当前 content_block / Close the current content_block.
                let Some(index) = self.active_anthropic_block.take() else {
                    return Vec::new();
                };
                vec![event_frame(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                )]
            }
            Protocol::OpenAiResponses => self.responses_text_end(),
        }
    }

    /// 一次性发出一个完整的相位输出（生命周期快捷方式）。
    /// Emits one complete phase output. Prefer the lifecycle methods for live deltas.
    pub fn text(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        // 等价于 phase_start → text_delta → phase_end / Equivalent to phase_start → text_delta → phase_end.
        [self.phase_start(), self.text_delta(text), self.phase_end()].concat()
    }

    /// 在暂停时发出原生工具调用，随后发出一个合法的终止序列。
    /// Emits native tool calls when paused, followed by one legal terminal sequence.
    pub fn finish(&mut self, result: &HttpTurnResult) -> Vec<String> {
        match self.protocol {
            Protocol::OpenAiChat => self.finish_chat(result),
            Protocol::AnthropicMessages => self.finish_anthropic(result),
            Protocol::OpenAiResponses => self.finish_responses(result),
        }
    }

    /// 在发出错误帧前关闭一个仍打开的相位块。
    /// Closes an active phase item before emitting an error event.
    pub fn abort_phase(&mut self) -> Vec<String> {
        match self.protocol {
            // Chat 无需关闭 / Chat needs no close.
            Protocol::OpenAiChat => Vec::new(),
            Protocol::AnthropicMessages => self.phase_end(),
            Protocol::OpenAiResponses => self.phase_end(),
        }
    }

    /// 为已打开的流发出错误帧（不含成功终止帧）。
    /// Emits an error for an already-open stream, without a success terminal frame.
    pub fn error(&mut self, message: &str) -> Vec<String> {
        match self.protocol {
            // Chat：data 帧含 error 对象 / Chat: data frame with an error object.
            Protocol::OpenAiChat => vec![data_frame(json!({
                "error": {"message": message, "type": "pigs_phase_error"}
            }))],
            // Anthropic：error 事件 / Anthropic: error event.
            Protocol::AnthropicMessages => vec![event_frame(
                "error",
                json!({
                    "type": "error",
                    "error": {"type": "pigs_phase_error", "message": message}
                }),
            )],
            // Responses：response.failed 事件 / Responses: response.failed event.
            Protocol::OpenAiResponses => vec![self.response_event(
                "response.failed",
                json!({
                    "response": {
                        "id": self.id,
                        "object": "response",
                        "status": "failed",
                        "error": {"code": "pigs_phase_error", "message": message}
                    }
                }),
            )],
        }
    }

    /// Chat 协议的终止序列（含工具调用帧 + finish_reason 帧 + [DONE]）。
    /// Chat protocol terminal sequence (tool-call frame + finish_reason frame + [DONE]).
    fn finish_chat(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
        // 若是工具暂停，先发出 tool_calls 帧 / If tool-paused, emit the tool_calls frame first.
        let finish_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
            frames.push(data_frame(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.client_model,
                "choices": [{
                    "index": 0,
                    "delta": {
                        // 给每个工具调用注入 index 字段（Chat 协议要求）/ Inject an index per tool call (required by Chat).
                        "tool_calls": tool_calls.iter().enumerate().map(|(index, call)| {
                            let mut native = call.native.clone();
                            if let Some(object) = native.as_object_mut() {
                                object.insert("index".into(), json!(index));
                            }
                            native
                        }).collect::<Vec<_>>()
                    },
                    "finish_reason": null
                }]
            })));
            "tool_calls"
        } else {
            "stop"
        };
        // 终止帧：finish_reason + usage / Terminal frame: finish_reason + usage.
        frames.push(data_frame(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.client_model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
            "usage": result.usage
        })));
        // Chat 流必须以 [DONE] 结尾 / Chat streams must end with [DONE].
        frames.push("data: [DONE]\n\n".to_string());
        frames
    }

    /// Anthropic 协议的终止序列（工具块 + message_delta + message_stop）。
    /// Anthropic protocol terminal sequence (tool blocks + message_delta + message_stop).
    fn finish_anthropic(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
        // 工具暂停 → 每个工具调用发出 start/delta/stop 三帧 / Tool pause → start/delta/stop per call.
        let stop_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
            for call in tool_calls {
                let index = self.take_index();
                // content_block_start（tool_use 块）/ content_block_start (tool_use block).
                frames.push(event_frame(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": anthropic_tool_start(call)
                    }),
                ));
                // input_json_delta（工具参数）/ input_json_delta (tool arguments).
                frames.push(event_frame(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": anthropic_tool_input(call)
                        }
                    }),
                ));
                // content_block_stop / content_block_stop.
                frames.push(event_frame(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            "tool_use"
        } else {
            "end_turn"
        };
        // message_delta（stop_reason + usage）/ message_delta (stop_reason + usage).
        frames.push(event_frame(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": result.usage
            }),
        ));
        // message_stop / message_stop.
        frames.push(event_frame("message_stop", json!({"type": "message_stop"})));
        frames
    }

    /// Responses 协议的终止序列（工具 item + response.completed）。
    /// Responses protocol terminal sequence (tool items + response.completed).
    fn finish_responses(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
        // 工具暂停 → 每个工具调用作为 output_item added/done / Tool pause → output_item added/done per call.
        if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
            for call in tool_calls {
                let output_index = self.take_index();
                frames.push(self.response_event(
                    "response.output_item.added",
                    json!({"output_index": output_index, "item": call.native}),
                ));
                frames.push(self.response_event(
                    "response.output_item.done",
                    json!({"output_index": output_index, "item": call.native}),
                ));
                self.response_output.push(call.native.clone());
            }
        }
        // 取出已完成的 output 数组用于 response.completed / Take the completed output array.
        let response_output = std::mem::take(&mut self.response_output);
        let completed = json!({
            "id": self.id,
            "object": "response",
            "created_at": self.created,
            "status": "completed",
            "model": self.client_model,
            "output": response_output,
            "usage": result.usage,
            "error": null,
            "incomplete_details": null
        });
        frames.push(self.response_event("response.completed", json!({"response": completed})));
        frames
    }

    /// Chat 协议的文本增量帧 / Chat protocol text-delta frame.
    fn chat_text_delta(&self, text: &str) -> Vec<String> {
        vec![data_frame(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.client_model,
            "choices": [{
                "index": 0,
                "delta": {"content": text},
                "finish_reason": null
            }]
        }))]
    }

    /// Responses 协议：开始一个 message output_item（含 content_part.added）。
    /// Responses protocol: start a message output_item (with content_part.added).
    fn responses_text_start(&mut self, boundary: &str) -> Vec<String> {
        let output_index = self.take_index();
        let item_id = response_id("msg");
        // 记录当前打开的 item 与累积文本 / Track the open item and accumulated text.
        self.active_response_item = Some((output_index, item_id.clone(), boundary.to_owned()));
        let mut frames = vec![
            // output_item.added（message item）/ output_item.added (message item).
            self.response_event(
                "response.output_item.added",
                json!({
                    "output_index": output_index,
                    "item": {
                        "type": "message", "id": item_id, "role": "assistant",
                        "status": "in_progress", "content": []
                    }
                }),
            ),
            // content_part.added（output_text 部分）/ content_part.added (output_text part).
            self.response_event(
                "response.content_part.added",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []}
                }),
            ),
        ];
        if !boundary.is_empty() {
            // 边界作为 output_text.delta 发出 / Emit the boundary as an output_text.delta.
            frames.push(self.response_event(
                "response.output_text.delta",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "delta": boundary
                }),
            ));
        }
        frames
    }

    /// Responses 协议：文本增量帧（并累积文本用于 done 时回填）。
    /// Responses protocol: text-delta frame (also accumulates text for the done event).
    fn responses_text_delta(&mut self, text: &str) -> Vec<String> {
        let Some((output_index, item_id, accumulated)) = &mut self.active_response_item else {
            // 没有打开的 item → 丢弃 / No open item → drop.
            return Vec::new();
        };
        accumulated.push_str(text);
        let output_index = *output_index;
        let item_id = item_id.clone();
        vec![self.response_event(
            "response.output_text.delta",
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "delta": text
            }),
        )]
    }

    /// Responses 协议：结束当前 message item（output_text.done + content_part.done + output_item.done）。
    /// Responses protocol: close the current message item (output_text.done + content_part.done + output_item.done).
    fn responses_text_end(&mut self) -> Vec<String> {
        let Some((output_index, item_id, text)) = self.active_response_item.take() else {
            return Vec::new();
        };
        // 构造已完成的 message item（含完整文本）/ Build the completed message item with full text.
        let completed_item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "completed", "content": [{
                "type": "output_text", "text": text, "annotations": []
            }]
        });
        // 推入 output 数组，供最终 response.completed 使用 / Push into the output array for response.completed.
        self.response_output.push(completed_item.clone());
        vec![
            // output_text.done（回填完整文本）/ output_text.done (full text).
            self.response_event(
                "response.output_text.done",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "text": text
                }),
            ),
            // content_part.done / content_part.done.
            self.response_event(
                "response.content_part.done",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "part": {"type": "output_text", "text": text, "annotations": []}
                }),
            ),
            // output_item.done / output_item.done.
            self.response_event(
                "response.output_item.done",
                json!({"output_index": output_index, "item": completed_item}),
            ),
        ]
    }

    /// Responses 协议：构造一个带 type 和 sequence_number 的事件帧。
    /// Responses protocol: build an event frame with type and sequence_number.
    fn response_event(&mut self, event_type: &str, extra: Value) -> String {
        let mut value = extra.as_object().cloned().unwrap_or_default();
        // 注入 type 字段 / Inject the type field.
        value.insert("type".into(), json!(event_type));
        // 注入递增的 sequence_number / Inject the incrementing sequence_number.
        value.insert("sequence_number".into(), json!(self.sequence));
        self.sequence += 1;
        event_frame(event_type, Value::Object(value))
    }

    /// 取下一个可用索引并自增 / Take the next available index and increment.
    fn take_index(&mut self) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

/// 把完成或工具暂停的轮次编码为入口协议的 JSON 响应。
/// Encodes a complete or tool-paused turn as the entry protocol's JSON response.
pub fn encode_json(protocol: Protocol, client_model: &str, result: &HttpTurnResult) -> Value {
    match protocol {
        Protocol::OpenAiChat => encode_chat_json(client_model, result),
        Protocol::AnthropicMessages => encode_anthropic_json(client_model, result),
        Protocol::OpenAiResponses => encode_responses_json(client_model, result),
    }
}

/// 把成功或工具暂停的轮次编码为完整的 SSE 帧序列。
/// Encodes a successful or tool-paused turn into complete SSE frames.
pub fn encode_sse(protocol: Protocol, client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    match protocol {
        Protocol::OpenAiChat => encode_chat_sse(client_model, result),
        Protocol::AnthropicMessages => encode_anthropic_sse(client_model, result),
        Protocol::OpenAiResponses => encode_responses_sse(client_model, result),
    }
}

/// 把流内运行时失败编码为 SSE 错误帧（不含任何成功终止帧）。
/// Encodes an in-stream runtime failure without any success terminal frame.
pub fn encode_sse_error(protocol: Protocol, message: &str) -> Vec<String> {
    match protocol {
        // Chat：data 帧含 error 对象 / Chat: data frame with an error object.
        Protocol::OpenAiChat => vec![data_frame(json!({
            "error": {"message": message, "type": "pigs_phase_error"}
        }))],
        // Anthropic：error 事件 / Anthropic: error event.
        Protocol::AnthropicMessages => vec![event_frame(
            "error",
            json!({
                "type": "error",
                "error": {"type": "pigs_phase_error", "message": message}
            }),
        )],
        // Responses：response.failed 事件（需自带 id）/ Responses: response.failed event (needs its own id).
        Protocol::OpenAiResponses => {
            let response_id = response_id("resp");
            vec![event_frame(
                "response.failed",
                json!({
                    "type": "response.failed",
                    "sequence_number": 0,
                    "response": {
                        "id": response_id,
                        "object": "response",
                        "status": "failed",
                        "error": {"code": "pigs_phase_error", "message": message}
                    }
                }),
            )]
        }
    }
}

/// 编码 Chat 非流式 JSON 响应 / Encode a Chat non-streaming JSON response.
fn encode_chat_json(client_model: &str, result: &HttpTurnResult) -> Value {
    // 基础 message 含 role + content / Base message with role + content.
    let mut message = json!({
        "role": "assistant",
        "content": result.visible_text,
    });
    // 工具暂停时附加 tool_calls 并改 finish_reason / On tool pause, attach tool_calls and set finish_reason.
    let finish_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        message["tool_calls"] = Value::Array(native_calls(tool_calls));
        "tool_calls"
    } else {
        "stop"
    };
    json!({
        "id": response_id("chatcmpl"),
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": client_model,
        "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
        "usage": result.usage,
    })
}

/// 编码 Anthropic 非流式 JSON 响应 / Encode an Anthropic non-streaming JSON response.
fn encode_anthropic_json(client_model: &str, result: &HttpTurnResult) -> Value {
    let mut content = Vec::new();
    // 有可见文本 → 先放 text 块 / Visible text → a text block first.
    if !result.visible_text.is_empty() {
        content.push(json!({"type": "text", "text": result.visible_text}));
    }
    // 工具暂停 → 追加 tool_use 块并改 stop_reason / Tool pause → append tool_use blocks, set stop_reason.
    let stop_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        content.extend(native_calls(tool_calls));
        "tool_use"
    } else {
        "end_turn"
    };
    json!({
        "id": response_id("msg"),
        "type": "message",
        "role": "assistant",
        "model": client_model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": result.usage,
    })
}

/// 编码 Responses 非流式 JSON 响应 / Encode a Responses non-streaming JSON response.
fn encode_responses_json(client_model: &str, result: &HttpTurnResult) -> Value {
    let mut output = Vec::new();
    // 有可见文本 → 先放 message item / Visible text → a message item first.
    if !result.visible_text.is_empty() {
        output.push(json!({
            "type": "message",
            "id": response_id("msg"),
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": result.visible_text,
                "annotations": []
            }]
        }));
    }
    // 工具暂停 → 追加 function_call items / Tool pause → append function_call items.
    if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        output.extend(native_calls(tool_calls));
    }
    json!({
        "id": response_id("resp"),
        "object": "response",
        "created_at": Utc::now().timestamp(),
        "status": "completed",
        "model": client_model,
        "output": output,
        "usage": result.usage,
        "error": null,
        "incomplete_details": null,
    })
}

/// 编码 Chat 完整 SSE 帧序列 / Encode a complete Chat SSE frame sequence.
fn encode_chat_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let id = response_id("chatcmpl");
    let created = Utc::now().timestamp();
    // 闭包：构造一个带 delta 和 finish_reason 的 data 帧 / Closure: build a data frame with delta + finish_reason.
    let base = |delta: Value, finish_reason: Value| {
        data_frame(json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": client_model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
        }))
    };
    // 首帧：role=assistant / First frame: role=assistant.
    let mut frames = vec![base(
        json!({"role": "assistant", "content": ""}),
        Value::Null,
    )];
    // 可见文本帧 / Visible text frame.
    if !result.visible_text.is_empty() {
        frames.push(base(json!({"content": result.visible_text}), Value::Null));
    }
    // 工具暂停 → tool_calls 帧 + finish_reason="tool_calls" / Tool pause → tool_calls frame + finish_reason.
    let finish_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        frames.push(base(
            json!({
                // 注入 index 字段 / Inject the index field.
                "tool_calls": tool_calls.iter().enumerate().map(|(index, call)| {
                    let mut native = call.native.clone();
                    if let Some(object) = native.as_object_mut() {
                        object.insert("index".into(), json!(index));
                    }
                    native
                }).collect::<Vec<_>>()
            }),
            Value::Null,
        ));
        "tool_calls"
    } else {
        "stop"
    };
    // 终止帧 + [DONE] / Terminal frame + [DONE].
    frames.push(base(
        Value::Object(Default::default()),
        json!(finish_reason),
    ));
    frames.push("data: [DONE]\n\n".to_string());
    frames
}

/// 编码 Anthropic 完整 SSE 帧序列 / Encode a complete Anthropic SSE frame sequence.
fn encode_anthropic_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let message_id = response_id("msg");
    // message_start / message_start.
    let mut frames = vec![event_frame(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": client_model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {}
            }
        }),
    )];
    // content_block 索引计数器 / content_block index counter.
    let mut index = 0usize;
    // 可见文本 → 一个 text 块（start/delta/stop）/ Visible text → one text block (start/delta/stop).
    if !result.visible_text.is_empty() {
        frames.push(event_frame(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "text", "text": ""}
            }),
        ));
        frames.push(event_frame(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "text_delta", "text": result.visible_text}
            }),
        ));
        frames.push(event_frame(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        ));
        index += 1;
    }
    // 工具暂停 → 每个工具一个 tool_use 块 / Tool pause → one tool_use block per call.
    let stop_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        for call in tool_calls {
            frames.push(event_frame(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": anthropic_tool_start(call)
                }),
            ));
            frames.push(event_frame(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": anthropic_tool_input(call)
                    }
                }),
            ));
            frames.push(event_frame(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            ));
            index += 1;
        }
        "tool_use"
    } else {
        "end_turn"
    };
    // message_delta（stop_reason + usage）/ message_delta (stop_reason + usage).
    frames.push(event_frame(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": result.usage
        }),
    ));
    // message_stop / message_stop.
    frames.push(event_frame("message_stop", json!({"type": "message_stop"})));
    frames
}

/// 编码 Responses 完整 SSE 帧序列 / Encode a complete Responses SSE frame sequence.
fn encode_responses_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let response_id_value = response_id("resp");
    let created_at = Utc::now().timestamp();
    // response 对象初始状态（in_progress）/ response object initial state (in_progress).
    let response = json!({
        "id": response_id_value,
        "object": "response",
        "created_at": created_at,
        "status": "in_progress",
        "model": client_model,
        "output": []
    });
    // 序列号计数器 / sequence-number counter.
    let mut sequence = 0u64;
    let mut frames = Vec::new();
    // response.created / response.created.
    push_response_event(
        &mut frames,
        "response.created",
        &mut sequence,
        json!({"response": response}),
    );
    // output_index 计数器 / output_index counter.
    let mut output_index = 0usize;
    // 已完成的 output 数组（用于 response.completed）/ completed output array (for response.completed).
    let mut completed_output = Vec::new();
    // 可见文本 → 一个完整 message item 生命周期 / Visible text → a full message item lifecycle.
    if !result.visible_text.is_empty() {
        let item_id = response_id("msg");
        let item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "in_progress", "content": []
        });
        // output_item.added / output_item.added.
        push_response_event(
            &mut frames,
            "response.output_item.added",
            &mut sequence,
            json!({"output_index": output_index, "item": item}),
        );
        // content_part.added / content_part.added.
        push_response_event(
            &mut frames,
            "response.content_part.added",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []}
            }),
        );
        // output_text.delta（完整可见文本）/ output_text.delta (full visible text).
        push_response_event(
            &mut frames,
            "response.output_text.delta",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "delta": result.visible_text
            }),
        );
        // output_text.done / output_text.done.
        push_response_event(
            &mut frames,
            "response.output_text.done",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "text": result.visible_text
            }),
        );
        // content_part.done / content_part.done.
        push_response_event(
            &mut frames,
            "response.content_part.done",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "part": {"type": "output_text", "text": result.visible_text, "annotations": []}
            }),
        );
        // 已完成的 message item / completed message item.
        let completed_item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "completed", "content": [{
                "type": "output_text", "text": result.visible_text, "annotations": []
            }]
        });
        // output_item.done / output_item.done.
        push_response_event(
            &mut frames,
            "response.output_item.done",
            &mut sequence,
            json!({"output_index": output_index, "item": completed_item}),
        );
        completed_output.push(completed_item);
        output_index += 1;
    }
    // 工具暂停 → 每个工具 added+done / Tool pause → added+done per call.
    if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        for call in tool_calls {
            push_response_event(
                &mut frames,
                "response.output_item.added",
                &mut sequence,
                json!({"output_index": output_index, "item": call.native}),
            );
            push_response_event(
                &mut frames,
                "response.output_item.done",
                &mut sequence,
                json!({"output_index": output_index, "item": call.native}),
            );
            completed_output.push(call.native.clone());
            output_index += 1;
        }
    }
    // 最终 response.completed / final response.completed.
    let completed = json!({
        "id": response_id_value,
        "object": "response",
        "created_at": created_at,
        "status": "completed",
        "model": client_model,
        "output": completed_output,
        "usage": result.usage,
        "error": null,
        "incomplete_details": null
    });
    push_response_event(
        &mut frames,
        "response.completed",
        &mut sequence,
        json!({"response": completed}),
    );
    frames
}

/// Responses 协议辅助：构造一个带 type + sequence_number 的事件帧并推进序列号。
/// Responses helper: build an event frame with type + sequence_number, advancing the counter.
fn push_response_event(
    frames: &mut Vec<String>,
    event_type: &str,
    sequence: &mut u64,
    extra: Value,
) {
    let mut value = extra.as_object().cloned().unwrap_or_default();
    value.insert("type".into(), json!(event_type));
    value.insert("sequence_number".into(), json!(*sequence));
    frames.push(event_frame(event_type, Value::Object(value)));
    *sequence += 1;
}

/// 构造 Anthropic tool_use 块的 content_block_start 值（input 初始化为空对象）。
/// Build the content_block_start value for an Anthropic tool_use block (input = {}).
fn anthropic_tool_start(call: &NativeToolCall) -> Value {
    let mut block = call.native.clone();
    // start 时 input 必须是空对象，实际值通过 input_json_delta 发出
    // At start, input must be an empty object; the real value arrives via input_json_delta.
    if let Some(object) = block.as_object_mut() {
        object.insert("input".into(), json!({}));
    }
    block
}

/// 把工具调用的 input（或回退到 arguments）序列化为 JSON 字符串。
/// Serialize a tool call's input (or fall back to arguments) as a JSON string.
fn anthropic_tool_input(call: &NativeToolCall) -> String {
    serde_json::to_string(call.native.get("input").unwrap_or(&call.arguments))
        .unwrap_or_else(|_| "{}".into())
}

/// 取出所有工具调用的原生 JSON 值 / Take the native JSON value of every tool call.
fn native_calls(calls: &[NativeToolCall]) -> Vec<Value> {
    calls.iter().map(|call| call.native.clone()).collect()
}

/// 生成一个 `{prefix}_{uuid}` 形式的响应 ID / Generate a `{prefix}_{uuid}` response ID.
fn response_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

/// 构造一个 `data: {value}\n\n` 形式的 SSE 数据帧 / Build a `data: {value}\n\n` SSE data frame.
fn data_frame(value: Value) -> String {
    format!("data: {value}\n\n")
}

/// 构造一个 `event: {event}\ndata: {value}\n\n` 形式的 SSE 事件帧 / Build a `event: {event}\ndata: {value}\n\n` SSE event frame.
fn event_frame(event: &str, value: Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
}
