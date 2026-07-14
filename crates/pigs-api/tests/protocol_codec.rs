#![allow(clippy::unwrap_used, clippy::expect_used)]

use pigs_api::protocol::{
    CodecError, HeaderPair, InternalTransportOverrides, NativeTranscriptItem, NativeTranscriptKind,
    Protocol, ProtocolCodec,
};
use serde_json::{json, Value};

fn parse(protocol: Protocol, body: Value) -> pigs_api::protocol::HttpRequestEnvelope {
    ProtocolCodec::new(protocol)
        .parse_request(
            "POST",
            "/v1/test?beta=true",
            vec![
                HeaderPair::new("authorization", "Bearer secret"),
                HeaderPair::new("x-client", "pigs-test"),
            ],
            body,
        )
        .unwrap()
}

#[test]
fn chat_preserves_rich_request_and_mutates_only_current_user() {
    let body = json!({
        "model": "gpt-4.1-pig-pig",
        "stream": true,
        "messages": [
            {
                "role": "system",
                "content": "Keep this system prompt",
                "name": "policy",
                "cache_control": {"type": "ephemeral"}
            },
            {"role": "assistant", "content": null, "tool_calls": [{
                "id": "call_old",
                "type": "function",
                "function": {"name": "lookup", "arguments": "{\"q\":\"old\"}"},
                "provider_extension": true
            }]},
            {"role": "tool", "tool_call_id": "call_old", "content": "old result"},
            {"role": "user", "content": [
                {"type": "text", "text": "Inspect this ", "cache_control": {"type": "ephemeral"}},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA==", "detail": "high"}, "text": "NON_TEXT_IMAGE_FIELD"},
                {"type": "text", "text": "carefully", "unknown": [1, 2, 3]}
            ], "name": "operator", "custom_message_field": {"keep": true}}
        ],
        "tools": [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}],
        "tool_choice": {"type": "function", "function": {"name": "lookup"}},
        "reasoning_effort": "high",
        "metadata": {"trace": "abc"},
        "unknown_top_level": {"nested": [true, null, 7]}
    });

    let request = parse(Protocol::OpenAiChat, body.clone());

    assert_eq!(request.method, "POST");
    assert_eq!(request.path_and_query, "/v1/test?beta=true");
    assert_eq!(request.headers[1], HeaderPair::new("x-client", "pigs-test"));
    assert_eq!(request.protocol, Protocol::OpenAiChat);
    assert_eq!(request.client_model, "gpt-4.1-pig-pig");
    assert_eq!(request.real_model, "gpt-4.1-pig");
    assert!(request.stream);
    assert_eq!(
        request.current_user_text().unwrap(),
        "Inspect this carefully"
    );
    assert_eq!(request.tool_result_ids(), &["call_old"]);
    assert_eq!(request.body, body);

    let pre = request
        .for_pre("PRE SUFFIX", &InternalTransportOverrides::default())
        .unwrap();
    let executor = request
        .for_executor("EXECUTOR SUFFIX", &InternalTransportOverrides::default())
        .unwrap();

    let mut expected_pre = body.clone();
    expected_pre["messages"][3]["content"][2]["text"] = json!("carefully\n\n---\n\nPRE SUFFIX");
    assert_eq!(pre.body, expected_pre);

    let mut expected_executor = body;
    expected_executor["messages"][3]["content"][2]["text"] =
        json!("carefully\n\n---\n\nEXECUTOR SUFFIX");
    assert_eq!(executor.body, expected_executor);
}

#[test]
fn anthropic_preserves_system_cache_media_tools_and_unknown_fields() {
    let body = json!({
        "model": "claude-sonnet-4-pig",
        "stream": false,
        "system": [
            {"type": "text", "text": "System one", "cache_control": {"type": "ephemeral"}},
            {"type": "text", "text": "System two", "extension": {"x": 1}}
        ],
        "messages": [
            {"role": "user", "content": "Earlier question", "custom": "old"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "private", "signature": "sig"},
                {"type": "tool_use", "id": "toolu_old", "name": "read", "input": {"path": "a"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_old", "content": [{"type": "text", "text": "result"}]},
                {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "AA=="}, "text": "NON_TEXT_DOCUMENT_FIELD"},
                {"type": "image", "source": {"type": "url", "url": "https://example.test/image.png"}},
                {"type": "text", "text": "Review document", "cache_control": {"type": "ephemeral"}, "future": 9}
            ], "future_message_field": ["keep"]}
        ],
        "tools": [{"name": "read", "description": "read", "input_schema": {"type": "object"}, "cache_control": {"type": "ephemeral"}}],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        "thinking": {"type": "enabled", "budget_tokens": 2048},
        "metadata": {"user_id": "u-1"},
        "future_top_level": {"keep": "yes"}
    });

    let request = parse(Protocol::AnthropicMessages, body.clone());
    assert_eq!(request.current_user_text().unwrap(), "Review document");
    assert_eq!(request.tool_result_ids(), &["toolu_old"]);
    assert_eq!(request.body, body);

    let mutated = request
        .for_pre(
            "Plan without changing native blocks",
            &InternalTransportOverrides {
                model: Some("claude-sonnet-4".into()),
                stream: Some(true),
            },
        )
        .unwrap();
    let mut expected = body;
    expected["model"] = json!("claude-sonnet-4");
    expected["stream"] = json!(true);
    expected["messages"][2]["content"][3]["text"] =
        json!("Review document\n\n---\n\nPlan without changing native blocks");
    assert_eq!(mutated.body, expected);
}

#[test]
fn responses_preserves_instructions_reasoning_tools_media_and_unknown_items() {
    let body = json!({
        "model": "o4-mini-pig",
        "stream": false,
        "instructions": {"segments": ["one", {"type": "policy", "value": "two"}]},
        "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Earlier"}]},
            {"type": "reasoning", "id": "rs_old", "summary": [{"type": "summary_text", "text": "summary"}], "encrypted_content": "enc"},
            {"type": "function_call", "call_id": "call_old", "name": "lookup", "arguments": "{\"q\":1}", "status": "completed"},
            {"type": "function_call_output", "call_id": "call_old", "output": [{"type": "input_text", "text": "tool output"}], "future": true},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "Analyze ", "annotations": []},
                {"type": "input_image", "image_url": "data:image/png;base64,AA==", "detail": "high", "text": "NON_TEXT_IMAGE_FIELD"},
                {"type": "input_file", "file_data": "data:application/pdf;base64,AA==", "filename": "brief.pdf"},
                {"type": "input_text", "text": "the attachments", "future_part": {"keep": true}}
            ], "status": "completed", "future_message": 3}
        ],
        "tools": [{"type": "function", "name": "lookup", "parameters": {"type": "object"}, "strict": true}],
        "tool_choice": "required",
        "reasoning": {"effort": "high", "summary": "detailed"},
        "include": ["reasoning.encrypted_content"],
        "metadata": {"job": "j1"},
        "future_top_level": [1, {"two": 2}]
    });

    let request = parse(Protocol::OpenAiResponses, body.clone());
    assert_eq!(
        request.current_user_text().unwrap(),
        "Analyze the attachments"
    );
    assert_eq!(request.tool_result_ids(), &["call_old"]);
    assert_eq!(request.body, body);

    let mutated = request
        .for_executor("Execute now", &InternalTransportOverrides::default())
        .unwrap();
    let mut expected = body;
    expected["input"][4]["content"][3]["text"] = json!("the attachments\n\n---\n\nExecute now");
    assert_eq!(mutated.body, expected);
}

#[test]
fn responses_string_input_is_supported_and_suffix_is_stripped_once() {
    let body = json!({
        "model": "gpt-5-pig-pig",
        "input": "Write the implementation",
        "stream": false,
        "instructions": ["preserve", {"unknown": true}],
        "metadata": {"keep": true}
    });
    let request = parse(Protocol::OpenAiResponses, body.clone());

    assert_eq!(request.client_model, "gpt-5-pig-pig");
    assert_eq!(request.real_model, "gpt-5-pig");
    assert_eq!(
        request.current_user_text().unwrap(),
        "Write the implementation"
    );

    let pre = request
        .for_pre("First analyze", &InternalTransportOverrides::default())
        .unwrap();
    let mut expected = body;
    expected["input"] = json!("Write the implementation\n\n---\n\nFirst analyze");
    assert_eq!(pre.body, expected);
}

#[test]
fn all_protocols_accept_native_tool_result_continuations() {
    let chat_result = json!({
        "role": "tool",
        "tool_call_id": "call_chat",
        "content": [{"type": "text", "text": "done"}],
        "unknown": true
    });
    let chat = parse(
        Protocol::OpenAiChat,
        json!({
            "model": "gpt-pig",
            "messages": [
                {"role": "user", "content": "do it"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": "call_chat", "type": "function", "function": {"name": "run", "arguments": "{}"}}]},
                chat_result
            ]
        }),
    );
    assert!(chat.is_continuation());
    assert_eq!(chat.tool_result_groups()[0].item.value, chat_result);

    let anthropic_result = json!({
        "role": "user",
        "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "one", "cache_control": {"type": "ephemeral"}},
            {"type": "tool_result", "tool_use_id": "toolu_2", "content": [{"type": "text", "text": "two"}]}
        ],
        "unknown": 7
    });
    let anthropic = parse(
        Protocol::AnthropicMessages,
        json!({
            "model": "claude-pig",
            "messages": [
                {"role": "user", "content": "do it"},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "one", "input": {}}, {"type": "tool_use", "id": "toolu_2", "name": "two", "input": {}}]},
                anthropic_result
            ]
        }),
    );
    assert!(anthropic.is_continuation());
    assert_eq!(
        anthropic.tool_result_groups()[0].ids,
        ["toolu_1", "toolu_2"]
    );
    assert_eq!(
        anthropic.tool_result_groups()[0].item.value,
        anthropic_result
    );

    let responses_result = json!({
        "type": "function_call_output",
        "call_id": "call_resp",
        "output": [{"type": "input_text", "text": "done"}],
        "unknown": {"keep": true}
    });
    let responses = parse(
        Protocol::OpenAiResponses,
        json!({
            "model": "gpt-pig",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "do it"}]},
                {"type": "function_call", "call_id": "call_resp", "name": "run", "arguments": "{}"},
                responses_result
            ]
        }),
    );
    assert!(responses.is_continuation());
    assert_eq!(
        responses.tool_result_groups()[0].item.value,
        responses_result
    );
}

#[test]
fn requests_still_reject_a_last_assistant_input_without_tool_results() {
    let fixtures = [
        (
            Protocol::OpenAiChat,
            json!({"model": "gpt-pig", "messages": [{"role": "assistant", "content": "done"}]}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model": "claude-pig", "messages": [{"role": "assistant", "content": "done"}]}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model": "gpt-pig", "input": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "done"}]}]}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model": "gpt-pig", "input": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "done"}]}]}),
        ),
    ];

    for (protocol, body) in fixtures {
        let error = ProtocolCodec::new(protocol)
            .parse_request("POST", "/test", Vec::new(), body)
            .unwrap_err();
        assert!(matches!(error, CodecError::LastInputNotUser { .. }));
    }
}

#[test]
fn post_drops_original_question_and_appends_native_transcript_then_review_prompt() {
    let body = json!({
        "model": "gpt-4.1-pig",
        "stream": true,
        "messages": [
            {"role": "system", "content": "system", "cache": {"keep": true}},
            {"role": "user", "content": "old question"},
            {"role": "assistant", "content": "old answer", "unknown": "keep"},
            {"role": "user", "content": [{"type": "text", "text": "ORIGINAL QUESTION"}, {"type": "image_url", "image_url": {"url": "x"}}]}
        ],
        "metadata": {"keep": true}
    });
    let request = parse(Protocol::OpenAiChat, body);
    let transcript_values = [
        json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_new", "type": "function", "function": {"name": "edit", "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": "call_new", "content": [{"type": "text", "text": "edited"}], "unknown": 1}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "executor answer"}], "refusal": null}),
        json!({"role": "assistant", "content": "first post review", "post_extension": true}),
    ];
    let transcript = vec![
        NativeTranscriptItem::new(
            Protocol::OpenAiChat,
            NativeTranscriptKind::Assistant,
            transcript_values[0].clone(),
        ),
        NativeTranscriptItem::new(
            Protocol::OpenAiChat,
            NativeTranscriptKind::ToolResult,
            transcript_values[1].clone(),
        ),
        NativeTranscriptItem::new(
            Protocol::OpenAiChat,
            NativeTranscriptKind::Assistant,
            transcript_values[2].clone(),
        ),
        NativeTranscriptItem::new(
            Protocol::OpenAiChat,
            NativeTranscriptKind::Assistant,
            transcript_values[3].clone(),
        ),
    ];

    let post = request
        .for_post(
            &transcript,
            "Review independently",
            &InternalTransportOverrides {
                model: Some("gpt-4.1".into()),
                stream: Some(false),
            },
        )
        .unwrap();

    assert_eq!(post.body["model"], "gpt-4.1");
    assert_eq!(post.body["stream"], false);
    assert_eq!(post.body["metadata"], json!({"keep": true}));
    assert_eq!(
        post.body["messages"],
        json!([
            {"role": "system", "content": "system", "cache": {"keep": true}},
            {"role": "user", "content": "old question"},
            {"role": "assistant", "content": "old answer", "unknown": "keep"},
            transcript_values[0].clone(),
            transcript_values[1].clone(),
            transcript_values[2].clone(),
            transcript_values[3].clone(),
            {"role": "user", "content": "Review independently"}
        ])
    );
    assert!(!post.body.to_string().contains("ORIGINAL QUESTION"));
}

#[test]
fn responses_string_post_inserts_transcript_and_independent_user_prompt() {
    let request = parse(
        Protocol::OpenAiResponses,
        json!({
            "model": "gpt-5-pig",
            "input": "Do not repeat this question",
            "instructions": "keep",
            "metadata": {"x": 1}
        }),
    );
    let transcript_values = [
        json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "opaque"}),
        json!({"type": "message", "id": "msg_1", "role": "assistant", "content": [{"type": "output_text", "text": "executor result", "annotations": []}]}),
    ];
    let transcript = vec![
        NativeTranscriptItem::new(
            Protocol::OpenAiResponses,
            NativeTranscriptKind::Reasoning,
            transcript_values[0].clone(),
        ),
        NativeTranscriptItem::new(
            Protocol::OpenAiResponses,
            NativeTranscriptKind::Assistant,
            transcript_values[1].clone(),
        ),
    ];

    let post = request
        .for_post(
            &transcript,
            "Review only the transcript",
            &InternalTransportOverrides::default(),
        )
        .unwrap();

    assert_eq!(post.body["instructions"], "keep");
    assert_eq!(post.body["metadata"], json!({"x": 1}));
    assert_eq!(
        post.body["input"],
        json!([
            transcript_values[0].clone(),
            transcript_values[1].clone(),
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Review only the transcript"}]
            }
        ])
    );
    assert!(!post
        .body
        .to_string()
        .contains("Do not repeat this question"));
}

#[test]
fn chat_extracts_native_output_tool_calls_usage_and_request_tool_results() {
    let request = parse(
        Protocol::OpenAiChat,
        json!({
            "model": "gpt-pig",
            "messages": [
                {"role": "assistant", "content": null, "tool_calls": [{"id": "call_prev", "type": "function", "function": {"name": "read", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "call_prev", "content": "result"},
                {"role": "user", "content": "continue"}
            ]
        }),
    );
    let native_message = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "Visible "}, {"type": "text", "text": "answer", "annotation": "keep"}],
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "function": {"name": "write", "arguments": "{\"path\":\"a\"}"},
            "provider": {"keep": true}
        }],
        "refusal": null,
        "unknown": 7
    });
    let response = json!({
        "id": "chatcmpl_1",
        "choices": [{"index": 0, "message": native_message, "finish_reason": "tool_calls", "logprobs": null}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14, "details": {"cached_tokens": 5}}
    });

    let output = ProtocolCodec::new(Protocol::OpenAiChat)
        .extract_response(&response)
        .unwrap();
    assert_eq!(output.visible_text, "Visible answer");
    assert_eq!(output.items.len(), 1);
    assert_eq!(output.items[0].value, native_message);
    assert_eq!(output.tool_calls[0].id, "call_1");
    assert_eq!(output.tool_calls[0].name, "write");
    assert_eq!(output.tool_calls[0].arguments, json!("{\"path\":\"a\"}"));
    assert_eq!(request.tool_result_ids(), &["call_prev"]);
    assert_eq!(output.stop_reason.as_deref(), Some("tool_calls"));
    assert_eq!(output.status, None);
    assert_eq!(output.usage, Some(response["usage"].clone()));
}

#[test]
fn anthropic_extracts_complete_blocks_tool_calls_and_request_tool_results() {
    let request = parse(
        Protocol::AnthropicMessages,
        json!({
            "model": "claude-pig",
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_prev", "name": "read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_prev", "content": "done"}]},
                {"role": "assistant", "content": "ack"},
                {"role": "user", "content": "continue"}
            ]
        }),
    );
    let blocks = json!([
        {"type": "thinking", "thinking": "private", "signature": "sig"},
        {"type": "text", "text": "Need a tool", "citations": [{"type": "page_location", "page_number": 3}]},
        {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"query": "pigs"}, "cache_control": {"type": "ephemeral"}}
    ]);
    let response = json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": blocks,
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {"input_tokens": 20, "output_tokens": 8, "cache_read_input_tokens": 10}
    });

    let output = ProtocolCodec::new(Protocol::AnthropicMessages)
        .extract_response(&response)
        .unwrap();
    assert_eq!(output.visible_text, "Need a tool");
    assert_eq!(
        output.items[0].value,
        json!({"role": "assistant", "content": blocks})
    );
    assert_eq!(output.tool_calls[0].id, "toolu_1");
    assert_eq!(output.tool_calls[0].name, "search");
    assert_eq!(output.tool_calls[0].arguments, json!({"query": "pigs"}));
    assert_eq!(request.tool_result_ids(), &["toolu_prev"]);
    assert_eq!(output.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(output.usage, Some(response["usage"].clone()));
}

#[test]
fn responses_extracts_all_native_items_tool_calls_status_and_request_tool_results() {
    let request = parse(
        Protocol::OpenAiResponses,
        json!({
            "model": "gpt-pig",
            "input": [
                {"type": "function_call", "call_id": "call_prev", "name": "read", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "call_prev", "output": "done"},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "continue"}]}
            ]
        }),
    );
    let native_items = json!([
        {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "summary"}], "encrypted_content": "opaque"},
        {"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [
            {"type": "output_text", "text": "Visible response", "annotations": [{"type": "url_citation", "url": "https://example.test"}]}
        ]},
        {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "edit", "arguments": "{\"path\":\"a\"}", "status": "completed", "future": true}
    ]);
    let response = json!({
        "id": "resp_1",
        "status": "incomplete",
        "incomplete_details": {"reason": "max_output_tokens"},
        "output": native_items,
        "usage": {"input_tokens": 30, "output_tokens": 12, "total_tokens": 42, "output_tokens_details": {"reasoning_tokens": 5}}
    });

    let output = ProtocolCodec::new(Protocol::OpenAiResponses)
        .extract_response(&response)
        .unwrap();
    assert_eq!(output.visible_text, "Visible response");
    assert_eq!(
        output
            .items
            .iter()
            .map(|item| item.value.clone())
            .collect::<Vec<_>>(),
        native_items.as_array().unwrap().clone()
    );
    assert_eq!(output.items[0].kind, NativeTranscriptKind::Reasoning);
    assert_eq!(output.items[1].kind, NativeTranscriptKind::Assistant);
    assert_eq!(output.items[2].kind, NativeTranscriptKind::ToolCall);
    assert_eq!(output.tool_calls[0].id, "call_1");
    assert_eq!(output.tool_calls[0].name, "edit");
    assert_eq!(output.tool_calls[0].arguments, json!("{\"path\":\"a\"}"));
    assert_eq!(request.tool_result_ids(), &["call_prev"]);
    assert_eq!(output.stop_reason.as_deref(), Some("max_output_tokens"));
    assert_eq!(output.status.as_deref(), Some("incomplete"));
    assert_eq!(output.usage, Some(response["usage"].clone()));
}
