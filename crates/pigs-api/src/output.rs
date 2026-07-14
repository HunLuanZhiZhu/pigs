//! Protocol-native non-streaming and SSE response encoding.

use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::http_runtime::{HttpTurnResult, HttpTurnStatus};
use crate::protocol::{NativeToolCall, Protocol};

/// Stateful SSE encoder used while phase execution is still in progress.
pub struct StreamingEncoder {
    protocol: Protocol,
    client_model: String,
    id: String,
    created: i64,
    sequence: u64,
    next_index: usize,
    wrote_text: bool,
    pending_phase_text: bool,
    active_anthropic_block: Option<usize>,
    active_response_item: Option<(usize, String, String)>,
    response_output: Vec<Value>,
}

impl StreamingEncoder {
    /// Creates an encoder for one client response stream.
    pub fn new(protocol: Protocol, client_model: impl Into<String>) -> Self {
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

    /// Emits the protocol's opening frame so the HTTP stream can start immediately.
    pub fn start(&mut self) -> Vec<String> {
        match self.protocol {
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

    /// Starts one phase text item and inserts a stable blank-line boundary.
    pub fn phase_start(&mut self) -> Vec<String> {
        self.pending_phase_text = true;
        Vec::new()
    }

    fn open_phase_text(&mut self) -> Vec<String> {
        let boundary = if self.wrote_text { "\n\n" } else { "" };
        self.wrote_text = true;
        self.pending_phase_text = false;
        match self.protocol {
            Protocol::OpenAiChat => {
                if boundary.is_empty() {
                    Vec::new()
                } else {
                    self.chat_text_delta(boundary)
                }
            }
            Protocol::AnthropicMessages => {
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

    /// Emits a safe marker-free text delta for the current phase.
    pub fn text_delta(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut frames = if self.pending_phase_text {
            self.open_phase_text()
        } else {
            Vec::new()
        };
        let delta = match self.protocol {
            Protocol::OpenAiChat => self.chat_text_delta(text),
            Protocol::AnthropicMessages => {
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

    /// Closes the current phase text item.
    pub fn phase_end(&mut self) -> Vec<String> {
        if self.pending_phase_text {
            self.pending_phase_text = false;
            return Vec::new();
        }
        match self.protocol {
            Protocol::OpenAiChat => Vec::new(),
            Protocol::AnthropicMessages => {
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

    /// Emits one complete phase output. Prefer the lifecycle methods for live deltas.
    pub fn text(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        [self.phase_start(), self.text_delta(text), self.phase_end()].concat()
    }

    /// Emits native tool calls when paused, followed by one legal terminal sequence.
    pub fn finish(&mut self, result: &HttpTurnResult) -> Vec<String> {
        match self.protocol {
            Protocol::OpenAiChat => self.finish_chat(result),
            Protocol::AnthropicMessages => self.finish_anthropic(result),
            Protocol::OpenAiResponses => self.finish_responses(result),
        }
    }

    /// Closes an active phase item before emitting an error event.
    pub fn abort_phase(&mut self) -> Vec<String> {
        match self.protocol {
            Protocol::OpenAiChat => Vec::new(),
            Protocol::AnthropicMessages => self.phase_end(),
            Protocol::OpenAiResponses => self.phase_end(),
        }
    }

    /// Emits an error for an already-open stream, without a success terminal frame.
    pub fn error(&mut self, message: &str) -> Vec<String> {
        match self.protocol {
            Protocol::OpenAiChat => vec![data_frame(json!({
                "error": {"message": message, "type": "pigs_phase_error"}
            }))],
            Protocol::AnthropicMessages => vec![event_frame(
                "error",
                json!({
                    "type": "error",
                    "error": {"type": "pigs_phase_error", "message": message}
                }),
            )],
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

    fn finish_chat(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
        let finish_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
            frames.push(data_frame(json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.client_model,
                "choices": [{
                    "index": 0,
                    "delta": {
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
        frames.push(data_frame(json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.client_model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
            "usage": result.usage
        })));
        frames.push("data: [DONE]\n\n".to_string());
        frames
    }

    fn finish_anthropic(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
        let stop_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
            for call in tool_calls {
                let index = self.take_index();
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
            }
            "tool_use"
        } else {
            "end_turn"
        };
        frames.push(event_frame(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": result.usage
            }),
        ));
        frames.push(event_frame("message_stop", json!({"type": "message_stop"})));
        frames
    }

    fn finish_responses(&mut self, result: &HttpTurnResult) -> Vec<String> {
        let mut frames = Vec::new();
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

    fn responses_text_start(&mut self, boundary: &str) -> Vec<String> {
        let output_index = self.take_index();
        let item_id = response_id("msg");
        self.active_response_item = Some((output_index, item_id.clone(), boundary.to_owned()));
        let mut frames = vec![
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
            self.response_event(
                "response.content_part.added",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []}
                }),
            ),
        ];
        if !boundary.is_empty() {
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

    fn responses_text_delta(&mut self, text: &str) -> Vec<String> {
        let Some((output_index, item_id, accumulated)) = &mut self.active_response_item else {
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

    fn responses_text_end(&mut self) -> Vec<String> {
        let Some((output_index, item_id, text)) = self.active_response_item.take() else {
            return Vec::new();
        };
        let completed_item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "completed", "content": [{
                "type": "output_text", "text": text, "annotations": []
            }]
        });
        self.response_output.push(completed_item.clone());
        vec![
            self.response_event(
                "response.output_text.done",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "text": text
                }),
            ),
            self.response_event(
                "response.content_part.done",
                json!({
                    "item_id": item_id, "output_index": output_index, "content_index": 0,
                    "part": {"type": "output_text", "text": text, "annotations": []}
                }),
            ),
            self.response_event(
                "response.output_item.done",
                json!({"output_index": output_index, "item": completed_item}),
            ),
        ]
    }

    fn response_event(&mut self, event_type: &str, extra: Value) -> String {
        let mut value = extra.as_object().cloned().unwrap_or_default();
        value.insert("type".into(), json!(event_type));
        value.insert("sequence_number".into(), json!(self.sequence));
        self.sequence += 1;
        event_frame(event_type, Value::Object(value))
    }

    fn take_index(&mut self) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

/// Encodes a complete or tool-paused turn as the entry protocol's JSON response.
pub fn encode_json(protocol: Protocol, client_model: &str, result: &HttpTurnResult) -> Value {
    match protocol {
        Protocol::OpenAiChat => encode_chat_json(client_model, result),
        Protocol::AnthropicMessages => encode_anthropic_json(client_model, result),
        Protocol::OpenAiResponses => encode_responses_json(client_model, result),
    }
}

/// Encodes a successful or tool-paused turn into complete SSE frames.
pub fn encode_sse(protocol: Protocol, client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    match protocol {
        Protocol::OpenAiChat => encode_chat_sse(client_model, result),
        Protocol::AnthropicMessages => encode_anthropic_sse(client_model, result),
        Protocol::OpenAiResponses => encode_responses_sse(client_model, result),
    }
}

/// Encodes an in-stream runtime failure without any success terminal frame.
pub fn encode_sse_error(protocol: Protocol, message: &str) -> Vec<String> {
    match protocol {
        Protocol::OpenAiChat => vec![data_frame(json!({
            "error": {"message": message, "type": "pigs_phase_error"}
        }))],
        Protocol::AnthropicMessages => vec![event_frame(
            "error",
            json!({
                "type": "error",
                "error": {"type": "pigs_phase_error", "message": message}
            }),
        )],
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

fn encode_chat_json(client_model: &str, result: &HttpTurnResult) -> Value {
    let mut message = json!({
        "role": "assistant",
        "content": result.visible_text,
    });
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

fn encode_anthropic_json(client_model: &str, result: &HttpTurnResult) -> Value {
    let mut content = Vec::new();
    if !result.visible_text.is_empty() {
        content.push(json!({"type": "text", "text": result.visible_text}));
    }
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

fn encode_responses_json(client_model: &str, result: &HttpTurnResult) -> Value {
    let mut output = Vec::new();
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

fn encode_chat_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let id = response_id("chatcmpl");
    let created = Utc::now().timestamp();
    let base = |delta: Value, finish_reason: Value| {
        data_frame(json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": client_model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
        }))
    };
    let mut frames = vec![base(
        json!({"role": "assistant", "content": ""}),
        Value::Null,
    )];
    if !result.visible_text.is_empty() {
        frames.push(base(json!({"content": result.visible_text}), Value::Null));
    }
    let finish_reason = if let HttpTurnStatus::ToolPause { tool_calls, .. } = &result.status {
        frames.push(base(
            json!({
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
    frames.push(base(
        Value::Object(Default::default()),
        json!(finish_reason),
    ));
    frames.push("data: [DONE]\n\n".to_string());
    frames
}

fn encode_anthropic_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let message_id = response_id("msg");
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
    let mut index = 0usize;
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
    frames.push(event_frame(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
            "usage": result.usage
        }),
    ));
    frames.push(event_frame("message_stop", json!({"type": "message_stop"})));
    frames
}

fn encode_responses_sse(client_model: &str, result: &HttpTurnResult) -> Vec<String> {
    let response_id_value = response_id("resp");
    let created_at = Utc::now().timestamp();
    let response = json!({
        "id": response_id_value,
        "object": "response",
        "created_at": created_at,
        "status": "in_progress",
        "model": client_model,
        "output": []
    });
    let mut sequence = 0u64;
    let mut frames = Vec::new();
    push_response_event(
        &mut frames,
        "response.created",
        &mut sequence,
        json!({"response": response}),
    );
    let mut output_index = 0usize;
    let mut completed_output = Vec::new();
    if !result.visible_text.is_empty() {
        let item_id = response_id("msg");
        let item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "in_progress", "content": []
        });
        push_response_event(
            &mut frames,
            "response.output_item.added",
            &mut sequence,
            json!({"output_index": output_index, "item": item}),
        );
        push_response_event(
            &mut frames,
            "response.content_part.added",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []}
            }),
        );
        push_response_event(
            &mut frames,
            "response.output_text.delta",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "delta": result.visible_text
            }),
        );
        push_response_event(
            &mut frames,
            "response.output_text.done",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "text": result.visible_text
            }),
        );
        push_response_event(
            &mut frames,
            "response.content_part.done",
            &mut sequence,
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "part": {"type": "output_text", "text": result.visible_text, "annotations": []}
            }),
        );
        let completed_item = json!({
            "type": "message", "id": item_id, "role": "assistant",
            "status": "completed", "content": [{
                "type": "output_text", "text": result.visible_text, "annotations": []
            }]
        });
        push_response_event(
            &mut frames,
            "response.output_item.done",
            &mut sequence,
            json!({"output_index": output_index, "item": completed_item}),
        );
        completed_output.push(completed_item);
        output_index += 1;
    }
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

fn anthropic_tool_start(call: &NativeToolCall) -> Value {
    let mut block = call.native.clone();
    if let Some(object) = block.as_object_mut() {
        object.insert("input".into(), json!({}));
    }
    block
}

fn anthropic_tool_input(call: &NativeToolCall) -> String {
    serde_json::to_string(call.native.get("input").unwrap_or(&call.arguments))
        .unwrap_or_else(|_| "{}".into())
}

fn native_calls(calls: &[NativeToolCall]) -> Vec<Value> {
    calls.iter().map(|call| call.native.clone()).collect()
}

fn response_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn data_frame(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn event_frame(event: &str, value: Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
}
