#![allow(clippy::unwrap_used)]

use pigs_api::http_runtime::{HttpTurnResult, HttpTurnStatus};
use pigs_api::output::{encode_json, encode_sse, encode_sse_error, StreamingEncoder};
use pigs_api::protocol::{NativeToolCall, NormalizedModelOutput, Protocol};
use serde_json::{json, Value};

fn output(_protocol: Protocol, tool_calls: Vec<NativeToolCall>) -> NormalizedModelOutput {
    NormalizedModelOutput {
        visible_text: "latest".into(),
        items: Vec::new(),
        tool_calls,
        tool_result_ids: Vec::new(),
        stop_reason: Some("stop".into()),
        status: Some("completed".into()),
        usage: Some(json!({"input_tokens": 2, "output_tokens": 3})),
    }
}

fn complete(protocol: Protocol) -> HttpTurnResult {
    HttpTurnResult {
        visible_text: "pre\n\nexecutor\n\npost".into(),
        usage: json!({"input_tokens": 6, "output_tokens": 9, "total_tokens": 15}),
        status: HttpTurnStatus::Complete,
        latest_output: output(protocol, Vec::new()),
    }
}

fn paused(protocol: Protocol) -> HttpTurnResult {
    let native = match protocol {
        Protocol::OpenAiChat => json!({
            "id": "call_1", "type": "function",
            "function": {"name": "run", "arguments": "{}"},
            "unknown": true
        }),
        Protocol::AnthropicMessages => json!({
            "type": "tool_use", "id": "call_1", "name": "run", "input": {"path": "a"},
            "cache_control": {"type": "ephemeral"}
        }),
        Protocol::OpenAiResponses => json!({
            "type": "function_call", "id": "fc_1", "call_id": "call_1",
            "name": "run", "arguments": "{}", "status": "completed", "unknown": true
        }),
    };
    let call = NativeToolCall {
        id: "call_1".into(),
        name: "run".into(),
        arguments: json!("{}"),
        native,
    };
    HttpTurnResult {
        visible_text: "pre\n\nneed tool".into(),
        usage: json!({"input_tokens": 4, "output_tokens": 2}),
        status: HttpTurnStatus::ToolPause {
            continuation_id: "cont_1".into(),
            tool_calls: vec![call.clone()],
        },
        latest_output: output(protocol, vec![call]),
    }
}

fn json_data(frames: &[String]) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|frame| {
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            (data != "[DONE]").then(|| serde_json::from_str(data).unwrap())
        })
        .collect()
}

#[test]
fn non_streaming_responses_use_the_entry_protocol_and_client_model() {
    let chat = encode_json(
        Protocol::OpenAiChat,
        "gpt-pig",
        &paused(Protocol::OpenAiChat),
    );
    assert_eq!(chat["model"], "gpt-pig");
    assert_eq!(chat["choices"][0]["message"]["content"], "pre\n\nneed tool");
    assert_eq!(
        chat["choices"][0]["message"]["tool_calls"][0]["unknown"],
        true
    );
    assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");

    let anthropic = encode_json(
        Protocol::AnthropicMessages,
        "claude-pig",
        &paused(Protocol::AnthropicMessages),
    );
    assert_eq!(anthropic["model"], "claude-pig");
    assert_eq!(anthropic["content"][0]["text"], "pre\n\nneed tool");
    assert_eq!(anthropic["content"][1]["type"], "tool_use");
    assert_eq!(anthropic["stop_reason"], "tool_use");

    let responses = encode_json(
        Protocol::OpenAiResponses,
        "gpt-pig",
        &paused(Protocol::OpenAiResponses),
    );
    assert_eq!(responses["model"], "gpt-pig");
    assert_eq!(
        responses["output"][0]["content"][0]["text"],
        "pre\n\nneed tool"
    );
    assert_eq!(responses["output"][1]["type"], "function_call");
    assert_eq!(responses["output"][1]["unknown"], true);
}

#[test]
fn chat_sse_is_parseable_and_ends_with_the_right_finish_reason() {
    let frames = encode_sse(
        Protocol::OpenAiChat,
        "gpt-pig",
        &complete(Protocol::OpenAiChat),
    );
    let data = json_data(&frames);
    assert_eq!(data[0]["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(
        data[1]["choices"][0]["delta"]["content"],
        "pre\n\nexecutor\n\npost"
    );
    assert_eq!(data.last().unwrap()["choices"][0]["finish_reason"], "stop");
    assert_eq!(frames.last().unwrap(), "data: [DONE]\n\n");
    assert!(!frames.join("").contains("PIGEND"));

    let paused_frames = encode_sse(
        Protocol::OpenAiChat,
        "gpt-pig",
        &paused(Protocol::OpenAiChat),
    );
    let paused_data = json_data(&paused_frames);
    assert_eq!(
        paused_data[2]["choices"][0]["delta"]["tool_calls"][0]["id"],
        "call_1"
    );
    assert_eq!(
        paused_data.last().unwrap()["choices"][0]["finish_reason"],
        "tool_calls"
    );
}

#[test]
fn anthropic_sse_has_a_legal_message_and_content_block_sequence() {
    let frames = encode_sse(
        Protocol::AnthropicMessages,
        "claude-pig",
        &paused(Protocol::AnthropicMessages),
    );
    let events: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame.lines().find_map(|line| line.strip_prefix("event: ")))
        .collect();
    assert_eq!(events.first(), Some(&"message_start"));
    assert_eq!(events.last(), Some(&"message_stop"));
    assert!(events.windows(3).any(|window| {
        window
            == [
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
            ]
    }));
    assert!(events.contains(&"message_delta"));
    let values = json_data(&frames);
    let tool_start = values
        .iter()
        .find(|value| value.pointer("/content_block/type") == Some(&json!("tool_use")))
        .unwrap();
    assert_eq!(tool_start["content_block"]["input"], json!({}));
    assert_eq!(
        tool_start["content_block"]["cache_control"],
        json!({"type": "ephemeral"})
    );
    let tool_delta = values
        .iter()
        .find(|value| value.pointer("/delta/type") == Some(&json!("input_json_delta")))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(tool_delta["delta"]["partial_json"].as_str().unwrap())
            .unwrap(),
        json!({"path": "a"})
    );
    for value in values {
        assert!(value.is_object());
    }
}

#[test]
fn responses_sse_has_monotonic_sequences_and_completed_terminal_event() {
    let frames = encode_sse(
        Protocol::OpenAiResponses,
        "gpt-pig",
        &complete(Protocol::OpenAiResponses),
    );
    let data = json_data(&frames);
    let sequences: Vec<u64> = data
        .iter()
        .map(|event| event["sequence_number"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, (0..sequences.len() as u64).collect::<Vec<_>>());
    assert_eq!(data.first().unwrap()["type"], "response.created");
    assert_eq!(data.last().unwrap()["type"], "response.completed");
    let added_ids: Vec<&str> = data
        .iter()
        .filter(|event| event["type"] == "response.output_item.added")
        .filter_map(|event| event.pointer("/item/id").and_then(Value::as_str))
        .collect();
    let completed_ids: Vec<&str> = data
        .last()
        .unwrap()
        .pointer("/response/output")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(completed_ids, added_ids);
    assert!(data
        .iter()
        .any(|event| event["type"] == "response.output_text.delta"));
}

#[test]
fn empty_phases_do_not_create_blocks_or_leading_boundaries() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let mut encoder = StreamingEncoder::new(protocol, "model-pig");
        let empty = [encoder.phase_start(), encoder.phase_end()].concat();
        assert!(empty.is_empty());
        let visible = encoder.text("actual");
        let deltas: String = json_data(&visible)
            .iter()
            .filter_map(|value| {
                value
                    .pointer("/choices/0/delta/content")
                    .or_else(|| value.pointer("/delta/text"))
                    .or_else(|| {
                        (value["type"] == "response.output_text.delta").then(|| &value["delta"])
                    })
                    .and_then(Value::as_str)
            })
            .collect();
        assert_eq!(deltas, "actual");
    }
}

#[test]
fn streaming_encoder_emits_phase_text_incrementally_and_one_terminal_sequence() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let mut encoder = StreamingEncoder::new(protocol, "model-pig");
        let start = encoder.start();
        assert!(!start.is_empty());
        let pre = encoder.text("pre");
        let executor = encoder.text("executor");
        let post = encoder.text("post");
        let emitted = [pre.clone(), executor.clone(), post.clone()].concat();
        let deltas: String = json_data(&emitted)
            .iter()
            .filter_map(|value| {
                value
                    .pointer("/choices/0/delta/content")
                    .or_else(|| value.pointer("/delta/text"))
                    .or_else(|| {
                        (value["type"] == "response.output_text.delta").then(|| &value["delta"])
                    })
                    .and_then(Value::as_str)
            })
            .collect();
        assert_eq!(deltas, "pre\n\nexecutor\n\npost");
        let finish = encoder.finish(&complete(protocol));
        let all_frames = [start.clone(), emitted.clone(), finish.clone()].concat();
        if protocol == Protocol::OpenAiResponses {
            let values = json_data(&all_frames);
            let live_ids: Vec<&str> = values
                .iter()
                .filter(|value| value["type"] == "response.output_item.added")
                .filter_map(|value| value.pointer("/item/id").and_then(Value::as_str))
                .collect();
            let completed_ids: Vec<&str> = values
                .last()
                .unwrap()
                .pointer("/response/output")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .collect();
            assert_eq!(completed_ids, live_ids);
        }
        let all = all_frames.join("");
        match protocol {
            Protocol::OpenAiChat => {
                assert_eq!(all.matches("[DONE]").count(), 1);
                assert_eq!(all.matches("\"finish_reason\":\"stop\"").count(), 1);
            }
            Protocol::AnthropicMessages => {
                assert_eq!(all.matches("message_start").count(), 2);
                assert_eq!(all.matches("message_stop").count(), 2);
            }
            Protocol::OpenAiResponses => {
                assert_eq!(all.matches("response.created").count(), 2);
                assert_eq!(all.matches("response.completed").count(), 2);
            }
        }
    }
}

#[test]
fn incremental_error_continues_the_open_stream_without_success_terminal_frames() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let mut encoder = StreamingEncoder::new(protocol, "model-pig");
        let frames = [
            encoder.start(),
            encoder.text("pre"),
            encoder.error("failed"),
        ]
        .concat();
        let joined = frames.join("");
        assert!(joined.contains("failed"));
        assert!(!joined.contains("[DONE]"));
        assert!(!joined.contains("message_stop"));
        assert!(!joined.contains("response.completed"));
        if protocol == Protocol::AnthropicMessages {
            assert_eq!(joined.matches("content_block_start").count(), 2);
            assert_eq!(joined.matches("content_block_stop").count(), 2);
        }
        if protocol == Protocol::OpenAiResponses {
            let values = json_data(&frames);
            let sequences: Vec<u64> = values
                .iter()
                .map(|value| value["sequence_number"].as_u64().unwrap())
                .collect();
            assert_eq!(sequences, (0..sequences.len() as u64).collect::<Vec<_>>());
            assert_eq!(values.last().unwrap()["type"], "response.failed");
        }
    }
}

#[test]
fn aborting_before_anthropic_phase_start_does_not_emit_an_orphan_stop() {
    let mut encoder = StreamingEncoder::new(Protocol::AnthropicMessages, "claude-pig");
    let frames = [
        encoder.start(),
        encoder.abort_phase(),
        encoder.error("failed"),
    ]
    .concat();
    let joined = frames.join("");
    assert!(!joined.contains("content_block_stop"));
    assert!(joined.contains("event: error"));
    assert!(!joined.contains("message_stop"));
}

#[test]
fn error_streams_never_emit_success_terminal_frames() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let joined = encode_sse_error(protocol, "turn failed").join("");
        assert!(joined.contains("turn failed"));
        assert!(!joined.contains("[DONE]"));
        assert!(!joined.contains("message_stop"));
        assert!(!joined.contains("response.completed"));
        if protocol == Protocol::OpenAiResponses {
            assert!(joined.contains("response.failed"));
        }
    }
}
