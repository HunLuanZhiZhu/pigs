#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pigs_api::continuation::ContinuationConfig;
use pigs_api::http_runtime::{
    HttpPhasedRuntime, HttpRuntimeConfig, HttpTurnStatus, PhaseProgress, RuntimeError,
};
use pigs_api::orchestration::OrchestrationLimits;
use pigs_api::protocol::{HeaderPair, HttpRequestEnvelope, Protocol, ProtocolCodec};
use pigs_api::transport::{PhaseTransport, TransportError, TransportResponse};
use pigs_config::Language;
use serde_json::{json, Value};

#[derive(Default)]
struct ScriptedTransport {
    responses: Mutex<VecDeque<Value>>,
    requests: Mutex<Vec<HttpRequestEnvelope>>,
}

impl ScriptedTransport {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<HttpRequestEnvelope> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl PhaseTransport for ScriptedTransport {
    async fn send(
        &self,
        request: HttpRequestEnvelope,
    ) -> Result<TransportResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        let body = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted response");
        Ok(TransportResponse { body })
    }
}

fn chat_request(body: Value) -> HttpRequestEnvelope {
    ProtocolCodec::new(Protocol::OpenAiChat)
        .parse_request(
            "POST",
            "/v1/chat/completions?trace=one",
            vec![
                HeaderPair::new("authorization", "Bearer client"),
                HeaderPair::new("x-client-extension", "keep"),
            ],
            body,
        )
        .unwrap()
}

fn chat_text(text: &str) -> Value {
    json!({
        "id": "chatcmpl_phase",
        "object": "chat.completion",
        "model": "real",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text, "unknown": true},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
    })
}

fn anthropic_text(text: &str) -> Value {
    json!({
        "id": "msg_phase",
        "type": "message",
        "role": "assistant",
        "model": "claude",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 2}
    })
}

fn responses_text(text: &str) -> Value {
    json!({
        "id": "resp_phase",
        "object": "response",
        "status": "completed",
        "model": "gpt",
        "output": [{
            "type": "message",
            "id": "msg_phase",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}]
        }],
        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
    })
}

fn native_request(protocol: Protocol, body: Value) -> HttpRequestEnvelope {
    ProtocolCodec::new(protocol)
        .parse_request(
            "POST",
            match protocol {
                Protocol::OpenAiChat => "/chat/completions",
                Protocol::AnthropicMessages => "/v1/messages?beta=true",
                Protocol::OpenAiResponses => "/responses?trace=true",
            },
            vec![HeaderPair::new("x-native", "keep")],
            body,
        )
        .unwrap()
}

fn runtime(transport: Arc<dyn PhaseTransport>) -> HttpPhasedRuntime {
    HttpPhasedRuntime::new(
        transport,
        HttpRuntimeConfig {
            language: Language::Zh,
            orchestration: OrchestrationLimits {
                max_post_iterations: 3,
                max_pre_replans: 2,
            },
            continuation: ContinuationConfig::default(),
        },
    )
}

#[tokio::test]
async fn anthropic_and_responses_run_natively_end_to_end() {
    let cases = [
        (
            Protocol::AnthropicMessages,
            json!({
                "model": "claude-pig",
                "stream": false,
                "system": [{"type": "text", "text": "system", "cache_control": {"type": "ephemeral"}}],
                "messages": [{"role": "user", "content": [{"type": "text", "text": "task"}, {"type": "image", "source": {"type": "url", "url": "https://example.test/a"}}]}],
                "tools": [{"name": "run", "input_schema": {"type": "object"}}],
                "metadata": {"keep": true},
                "unknown": "anthropic"
            }),
            vec![
                anthropic_text("pre"),
                anthropic_text("executor"),
                anthropic_text("post\nPIGEND"),
            ],
            "claude",
        ),
        (
            Protocol::OpenAiResponses,
            json!({
                "model": "gpt-pig",
                "stream": false,
                "instructions": [{"type": "policy", "text": "system"}],
                "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "task"}, {"type": "input_image", "image_url": "data:image/png;base64,AA=="}]}],
                "tools": [{"type": "function", "name": "run", "parameters": {"type": "object"}}],
                "metadata": {"keep": true},
                "unknown": "responses"
            }),
            vec![
                responses_text("pre"),
                responses_text("executor"),
                responses_text("post\nPIGEND"),
            ],
            "gpt",
        ),
    ];

    for (protocol, body, responses, real_model) in cases {
        let transport = Arc::new(ScriptedTransport::new(responses));
        let result = runtime(transport.clone())
            .run(native_request(protocol, body.clone()))
            .await
            .unwrap();
        assert_eq!(result.visible_text, "pre\n\nexecutor\n\npost");
        assert!(matches!(result.status, HttpTurnStatus::Complete));
        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.protocol, protocol);
            assert_eq!(request.body["model"], real_model);
            assert_eq!(request.body["stream"], false);
            assert_eq!(request.body["metadata"], body["metadata"]);
            assert_eq!(request.body["unknown"], body["unknown"]);
            match protocol {
                Protocol::AnthropicMessages => assert_eq!(request.body["system"], body["system"]),
                Protocol::OpenAiResponses => {
                    assert_eq!(request.body["instructions"], body["instructions"])
                }
                Protocol::OpenAiChat => unreachable!(),
            }
        }
    }
}

#[tokio::test]
async fn runs_protocol_native_pre_executor_post_and_repeated_post() {
    let transport = Arc::new(ScriptedTransport::new(vec![
        chat_text("pre plan"),
        chat_text("executor work"),
        chat_text("post progress"),
        chat_text("post accepted\nPIGEND"),
    ]));
    let request_body = json!({
        "model": "gpt-5-pig",
        "stream": true,
        "messages": [
            {"role": "system", "content": "preserve system", "cache_control": {"type": "ephemeral"}},
            {"role": "user", "content": "old"},
            {"role": "assistant", "content": "old answer"},
            {"role": "user", "content": [{"type": "text", "text": "do task"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}]}
        ],
        "tools": [{"type": "function", "function": {"name": "run", "parameters": {"type": "object"}}}],
        "tool_choice": "auto",
        "reasoning_effort": "high",
        "metadata": {"preserve": true},
        "unknown": [1, 2, 3]
    });

    let result = runtime(transport.clone())
        .run(chat_request(request_body.clone()))
        .await
        .unwrap();

    assert!(matches!(result.status, HttpTurnStatus::Complete));
    assert_eq!(
        result.visible_text,
        "pre plan\n\nexecutor work\n\npost progress\n\npost accepted"
    );
    assert!(!result.visible_text.contains("PIGEND"));
    assert_eq!(result.usage["prompt_tokens"], 4);
    assert_eq!(result.usage["completion_tokens"], 8);

    let requests = transport.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path_and_query, "/v1/chat/completions?trace=one");
        assert_eq!(
            request.headers[1],
            HeaderPair::new("x-client-extension", "keep")
        );
        assert_eq!(request.body["model"], "gpt-5");
        assert_eq!(request.body["stream"], false);
        assert_eq!(request.body["metadata"], json!({"preserve": true}));
        assert_eq!(request.body["unknown"], json!([1, 2, 3]));
        assert_eq!(request.body["messages"][0], request_body["messages"][0]);
        assert_eq!(request.body["tools"], request_body["tools"]);
    }

    let second_post = &requests[3].body["messages"];
    assert_eq!(
        second_post
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["content"] == "executor work")
            .count(),
        1
    );
    assert_eq!(
        second_post
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["content"] == "post progress")
            .count(),
        1
    );
    assert_eq!(
        second_post.as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .trim_end(),
        pigs_prompts::post_user_payload(Language::Zh, "", "").trim_end()
    );
}

#[tokio::test]
async fn pauses_for_native_tool_calls_and_resumes_the_same_phase() {
    let tool_call = json!({
        "id": "chatcmpl_tool",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "need tool",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "run", "arguments": "{\"value\":1}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
    });
    let transport = Arc::new(ScriptedTransport::new(vec![
        chat_text("pre plan"),
        tool_call.clone(),
        chat_text("executor complete"),
        chat_text("accepted\nPIGEND"),
    ]));
    let runtime = runtime(transport.clone());
    let original = json!({
        "model": "gpt-5-pig",
        "messages": [{"role": "user", "content": "do task"}],
        "tools": [{"type": "function", "function": {"name": "run", "parameters": {"type": "object"}}}],
        "unknown": "keep"
    });

    let paused = runtime.run(chat_request(original.clone())).await.unwrap();
    let HttpTurnStatus::ToolPause {
        continuation_id,
        tool_calls,
    } = paused.status
    else {
        panic!("expected tool pause");
    };
    assert!(!continuation_id.is_empty());
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(paused.visible_text, "pre plan\n\nneed tool");

    let resumed_request = ProtocolCodec::new(Protocol::OpenAiChat)
        .parse_request(
            "POST",
            "/v1/chat/completions?trace=one",
            vec![HeaderPair::new("authorization", "Bearer resumed")],
            json!({
                "model": "gpt-5-pig",
                "messages": [
                    {"role": "user", "content": "do task"},
                    tool_call["choices"][0]["message"].clone(),
                    {"role": "tool", "tool_call_id": "call_1", "content": "tool result", "unknown": true}
                ],
                "tools": original["tools"].clone(),
                "unknown": "keep"
            }),
        )
        .unwrap();
    let progress = Arc::new(Mutex::new(Vec::<String>::new()));
    let progress_sink = Arc::clone(&progress);
    let completed = runtime
        .run_with_progress(
            resumed_request,
            Arc::new(move |event| {
                if let PhaseProgress::TextDelta(text) = event {
                    progress_sink.lock().unwrap().push(text);
                }
            }),
        )
        .await
        .unwrap();

    assert!(matches!(completed.status, HttpTurnStatus::Complete));
    assert_eq!(
        completed.visible_text,
        "pre plan\n\nneed tool\n\nexecutor complete\n\naccepted"
    );
    assert_eq!(
        progress.lock().unwrap().concat(),
        "executor completeaccepted"
    );
    let requests = transport.requests();
    assert_eq!(requests.len(), 4);
    let resumed_phase = requests[2].body["messages"].as_array().unwrap();
    assert_eq!(
        resumed_phase[resumed_phase.len() - 2]["tool_calls"][0]["id"],
        "call_1"
    );
    assert_eq!(resumed_phase.last().unwrap()["tool_call_id"], "call_1");
    assert_eq!(
        requests[2].headers[0],
        HeaderPair::new("authorization", "Bearer resumed")
    );
}

#[tokio::test]
async fn anthropic_and_responses_pause_and_resume_native_tools() {
    let anthropic_call = json!({
        "id": "msg_tool",
        "type": "message",
        "role": "assistant",
        "model": "claude",
        "content": [
            {"type": "text", "text": "need tool"},
            {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a"}, "unknown": true}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 1, "output_tokens": 2}
    });
    let anthropic_result = json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": "toolu_1",
            "content": [{"type": "text", "text": "done"}],
            "unknown": true
        }]
    });
    let responses_call = json!({
        "id": "resp_tool",
        "object": "response",
        "status": "completed",
        "model": "gpt",
        "output": [
            {
                "type": "message", "id": "msg_tool", "role": "assistant", "status": "completed",
                "content": [{"type": "output_text", "text": "need tool", "annotations": []}]
            },
            {
                "type": "function_call", "id": "fc_1", "call_id": "call_1",
                "name": "read", "arguments": "{\"path\":\"a\"}", "status": "completed", "unknown": true
            }
        ],
        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
    });
    let responses_result = json!({
        "type": "function_call_output",
        "call_id": "call_1",
        "output": [{"type": "input_text", "text": "done"}],
        "unknown": true
    });
    let cases = [
        (
            Protocol::AnthropicMessages,
            json!({
                "model": "claude-pig",
                "messages": [{"role": "user", "content": "task"}],
                "tools": [{"name": "read", "input_schema": {"type": "object"}}]
            }),
            anthropic_call.clone(),
            json!({
                "model": "claude-pig",
                "messages": [
                    {"role": "user", "content": "task"},
                    {"role": "assistant", "content": anthropic_call["content"].clone()},
                    anthropic_result.clone()
                ]
            }),
            anthropic_text("pre after tool"),
            anthropic_text("executor"),
            anthropic_text("accepted\nPIGEND"),
            "toolu_1",
        ),
        (
            Protocol::OpenAiResponses,
            json!({
                "model": "gpt-pig",
                "input": [{
                    "type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "task"}]
                }],
                "tools": [{"type": "function", "name": "read", "parameters": {"type": "object"}}]
            }),
            responses_call.clone(),
            json!({
                "model": "gpt-pig",
                "input": [
                    {
                        "type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": "task"}]
                    },
                    responses_call["output"][1].clone(),
                    responses_result.clone()
                ]
            }),
            responses_text("pre after tool"),
            responses_text("executor"),
            responses_text("accepted\nPIGEND"),
            "call_1",
        ),
    ];

    for (protocol, original, tool_call, resumed, pre, executor, post, expected_id) in cases {
        let transport = Arc::new(ScriptedTransport::new(vec![tool_call, pre, executor, post]));
        let runtime = runtime(transport.clone());
        let paused = runtime
            .run(native_request(protocol, original))
            .await
            .unwrap();
        let HttpTurnStatus::ToolPause { tool_calls, .. } = paused.status else {
            panic!("expected native tool pause");
        };
        assert_eq!(tool_calls[0].id, expected_id);
        assert_eq!(paused.visible_text, "need tool");

        let completed = runtime
            .run(native_request(protocol, resumed))
            .await
            .unwrap();
        assert!(matches!(completed.status, HttpTurnStatus::Complete));
        assert_eq!(
            completed.visible_text,
            "need tool\n\npre after tool\n\nexecutor\n\naccepted"
        );
        let resumed_body = &transport.requests()[1].body;
        assert!(resumed_body.to_string().contains(expected_id));
        assert!(resumed_body.to_string().contains("\"unknown\":true"));
    }
}

#[tokio::test]
async fn parallel_tool_results_can_arrive_in_separate_requests() {
    let tool_call = json!({
        "id": "chatcmpl_tools",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "need tools",
                "tool_calls": [
                    {"id": "call_a", "type": "function", "function": {"name": "a", "arguments": "{}"}},
                    {"id": "call_b", "type": "function", "function": {"name": "b", "arguments": "{}"}}
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let transport = Arc::new(ScriptedTransport::new(vec![
        chat_text("pre"),
        tool_call.clone(),
        chat_text("executor"),
        chat_text("accepted\nPIGEND"),
    ]));
    let runtime = runtime(transport.clone());
    runtime
        .run(chat_request(json!({
            "model": "gpt-pig",
            "messages": [{"role": "user", "content": "task"}]
        })))
        .await
        .unwrap();

    let first = chat_request(json!({
        "model": "gpt-pig",
        "messages": [
            {"role": "user", "content": "task"},
            tool_call["choices"][0]["message"].clone(),
            {"role": "tool", "tool_call_id": "call_a", "content": "A"}
        ]
    }));
    assert!(matches!(
        runtime.run(first).await,
        Err(RuntimeError::MissingToolResults { missing_ids }) if missing_ids == vec!["call_b"]
    ));
    assert_eq!(transport.requests().len(), 2);

    let second = chat_request(json!({
        "model": "gpt-pig",
        "messages": [
            {"role": "user", "content": "task"},
            tool_call["choices"][0]["message"].clone(),
            {"role": "tool", "tool_call_id": "call_a", "content": "A"},
            {"role": "tool", "tool_call_id": "call_b", "content": "B"}
        ]
    }));
    let completed = runtime.run(second).await.unwrap();
    assert!(matches!(completed.status, HttpTurnStatus::Complete));
    let requests = transport.requests();
    let resumed_messages = requests[2].body["messages"].as_array().unwrap();
    let result_ids: Vec<&str> = resumed_messages
        .iter()
        .filter_map(|message| message.get("tool_call_id").and_then(Value::as_str))
        .collect();
    assert_eq!(result_ids, ["call_a", "call_b"]);
}

#[tokio::test]
async fn tool_resumed_phase_submits_its_complete_visible_output() {
    let tool_call = json!({
        "id": "chatcmpl_tool",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "pre before tool",
                "tool_calls": [{
                    "id": "call_pre",
                    "type": "function",
                    "function": {"name": "read", "arguments": "{}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let transport = Arc::new(ScriptedTransport::new(vec![
        tool_call.clone(),
        chat_text("pre after tool"),
        chat_text("executor"),
        chat_text("accepted\nPIGEND"),
    ]));
    let runtime = runtime(transport.clone());
    runtime
        .run(chat_request(json!({
            "model": "gpt-pig",
            "messages": [{"role": "user", "content": "task"}]
        })))
        .await
        .unwrap();
    runtime
        .run(chat_request(json!({
            "model": "gpt-pig",
            "messages": [
                {"role": "user", "content": "task"},
                tool_call["choices"][0]["message"].clone(),
                {"role": "tool", "tool_call_id": "call_pre", "content": "done"}
            ]
        })))
        .await
        .unwrap();

    let requests = transport.requests();
    let executor_user = requests[2].body["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(executor_user.contains("pre before tool\n\npre after tool"));
}

#[tokio::test]
async fn concurrent_duplicate_tool_results_resume_exactly_once() {
    let tool_call = json!({
        "id": "chatcmpl_tool",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "need tool",
                "tool_calls": [{
                    "id": "call_concurrent",
                    "type": "function",
                    "function": {"name": "run", "arguments": "{}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let transport = Arc::new(ScriptedTransport::new(vec![
        chat_text("pre"),
        tool_call.clone(),
        chat_text("executor"),
        chat_text("accepted\nPIGEND"),
    ]));
    let runtime = Arc::new(runtime(transport.clone()));
    runtime
        .run(chat_request(json!({
            "model": "gpt-pig",
            "messages": [{"role": "user", "content": "task"}]
        })))
        .await
        .unwrap();
    let resume = || {
        chat_request(json!({
            "model": "gpt-pig",
            "messages": [
                {"role": "user", "content": "task"},
                tool_call["choices"][0]["message"].clone(),
                {"role": "tool", "tool_call_id": "call_concurrent", "content": "done"}
            ]
        }))
    };

    let first_runtime = Arc::clone(&runtime);
    let second_runtime = Arc::clone(&runtime);
    let (first, second) = tokio::join!(first_runtime.run(resume()), second_runtime.run(resume()));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(transport.requests().len(), 4);
}

#[tokio::test]
async fn streams_each_marker_free_phase_output_in_execution_order() {
    let transport = Arc::new(ScriptedTransport::new(vec![
        chat_text("pre plan"),
        chat_text("executor work"),
        chat_text("post progress"),
        chat_text("post accepted\nPIGEND"),
    ]));
    let progress = Arc::new(Mutex::new(Vec::<String>::new()));
    let progress_sink = progress.clone();
    let request = chat_request(json!({
        "model": "gpt-pig",
        "stream": true,
        "messages": [{"role": "user", "content": "task"}]
    }));

    runtime(transport)
        .run_with_progress(
            request,
            Arc::new(move |event| {
                if let PhaseProgress::TextDelta(text) = event {
                    progress_sink.lock().unwrap().push(text);
                }
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        *progress.lock().unwrap(),
        [
            "pre plan",
            "executor work",
            "post progress",
            "post accepted"
        ]
    );
}

#[tokio::test]
async fn unknown_tool_result_is_an_explicit_continuation_error() {
    let transport = Arc::new(ScriptedTransport::default());
    let request = chat_request(json!({
        "model": "gpt-5-pig",
        "messages": [
            {"role": "user", "content": "do task"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "missing", "type": "function", "function": {"name": "run", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "missing", "content": "done"}
        ]
    }));

    assert!(matches!(
        runtime(transport).run(request).await,
        Err(RuntimeError::UnknownContinuation { .. })
    ));
}
