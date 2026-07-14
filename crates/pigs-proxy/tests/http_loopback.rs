#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, Response, StatusCode};
use axum::routing::any;
use axum::Router;
use pigs_api::protocol::{HeaderPair, Protocol, ProtocolCodec};
use pigs_api::transport::PhaseTransport;
use pigs_proxy::loopback::{LoopbackPhaseTransport, INTERNAL_PHASE_HEADER};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct CapturedRequest {
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Value,
}

type Captures = Arc<Mutex<Vec<CapturedRequest>>>;

async fn capture_handler(
    State(captures): State<Captures>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    captures.lock().unwrap().push(CapturedRequest {
        method,
        uri: uri.to_string(),
        headers,
        body,
    });
    json_response(json!({
        "id": "chatcmpl_mock",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
}

async fn phase_provider_handler(
    State(captures): State<Captures>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    let index = captures.lock().unwrap().len();
    captures.lock().unwrap().push(CapturedRequest {
        method,
        uri: uri.to_string(),
        headers,
        body,
    });
    let text = match index {
        0 => "pre plan",
        1 => "executor work",
        _ => "post accepted\nPIGEND",
    };
    json_response(json!({
        "id": format!("chatcmpl_{index}"),
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
}

async fn native_phase_provider_handler(
    State(captures): State<Captures>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    let uri_text = uri.to_string();
    let phase_index = {
        let mut captures = captures.lock().unwrap();
        let phase_index = captures
            .iter()
            .filter(|capture| capture.uri == uri_text)
            .count();
        captures.push(CapturedRequest {
            method,
            uri: uri_text.clone(),
            headers,
            body,
        });
        phase_index
    };
    let text = match phase_index {
        0 => "pre plan",
        1 => "executor work",
        _ => "post accepted\nPIGEND",
    };
    match uri.path() {
        "/chat/completions" => json_response(json!({
            "id": format!("chatcmpl_{phase_index}"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })),
        "/v1/messages" => json_response(json!({
            "id": format!("msg_{phase_index}"),
            "type": "message",
            "role": "assistant",
            "model": "gpt",
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })),
        "/responses" => json_response(json!({
            "id": format!("resp_{phase_index}"),
            "object": "response",
            "status": "completed",
            "model": "gpt",
            "output": [{
                "type": "message",
                "id": format!("msg_{phase_index}"),
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": []}]
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        })),
        other => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(format!("unexpected provider path: {other}")))
            .unwrap(),
    }
}

async fn malformed_streaming_phase_provider_handler(
    State(captures): State<Captures>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    captures.lock().unwrap().push(CapturedRequest {
        method,
        uri: uri.to_string(),
        headers,
        body,
    });
    let frames = [
        format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl_bad",
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"content": "partial"}, "finish_reason": null}]
            })
        ),
        format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl_bad",
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            })
        ),
    ];
    let stream = futures_util::stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<Bytes, std::io::Error>(Bytes::from(frame))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn streaming_phase_provider_handler(
    State(captures): State<Captures>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    let index = {
        let mut captures = captures.lock().unwrap();
        let index = captures.len();
        captures.push(CapturedRequest {
            method,
            uri: uri.to_string(),
            headers,
            body,
        });
        index
    };
    let deltas: Vec<&str> = match index {
        0 => vec!["pre plan"],
        1 => vec!["executor work"],
        _ => vec!["post accepted\nPI", "GEND"],
    };
    let mut frames = vec![format!(
        "data: {}\n\n",
        json!({
            "id": format!("chatcmpl_{index}"),
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
        })
    )];
    for delta in deltas {
        frames.push(format!(
            "data: {}\n\n",
            json!({
                "id": format!("chatcmpl_{index}"),
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]
            })
        ));
    }
    frames.push(format!(
        "data: {}\n\n",
        json!({
            "id": format!("chatcmpl_{index}"),
            "object": "chat.completion.chunk",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
    ));
    frames.push("data: [DONE]\n\n".into());
    let stream = futures_util::stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<Bytes, std::io::Error>(Bytes::from(frame))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn proxy_config(provider_addr: SocketAddr, proxy_port: u16) -> pigs_proxy::config::Config {
    let config_text = format!(
        r#"
[server]
listen = "127.0.0.1:{proxy_port}"
clean_empty_content = false

[log]
to_stdout = false

language = "zh"

[[provider]]
name = "mock"

[provider.openai]
base_url = "http://{provider_addr}"
models = ["gpt"]
max_retries = 0
path_mode = "append"
key_mode = "passthrough"
thinking_effort = "passthrough"
"#
    );
    toml::from_str(&config_text).unwrap()
}

fn all_protocols_proxy_config(
    provider_addr: SocketAddr,
    proxy_port: u16,
) -> pigs_proxy::config::Config {
    let config_text = format!(
        r#"
[server]
listen = "127.0.0.1:{proxy_port}"
clean_empty_content = false

[log]
to_stdout = false

language = "zh"

[[provider]]
name = "mock"

[provider.openai]
base_url = "http://{provider_addr}"
models = ["gpt"]
max_retries = 0
path_mode = "append"
key_mode = "passthrough"
thinking_effort = "passthrough"

[provider.anthropic]
base_url = "http://{provider_addr}"
models = ["gpt"]
max_retries = 0
path_mode = "append"
key_mode = "passthrough"
thinking_effort = "passthrough"

[provider.responses]
base_url = "http://{provider_addr}"
models = ["gpt"]
max_retries = 0
path_mode = "append"
key_mode = "passthrough"
thinking_effort = "passthrough"
"#
    );
    toml::from_str(&config_text).unwrap()
}

fn visible_sse_text(stream: &str) -> String {
    stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter_map(|value| {
            value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn json_response(value: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap()
}

async fn spawn_mock(
    handler: axum::routing::MethodRouter<Captures>,
) -> (SocketAddr, Captures, JoinHandle<()>) {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/", handler.clone())
        .route("/*path", handler)
        .with_state(captures.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, captures, task)
}

fn unused_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn loopback_transport_preserves_method_path_query_headers_and_body() {
    let (addr, captures, task) = spawn_mock(any(capture_handler)).await;
    let transport = LoopbackPhaseTransport::new(addr, "internal-secret".into()).unwrap();
    let request = ProtocolCodec::new(Protocol::OpenAiChat)
        .parse_request(
            "POST",
            "/v1/chat/completions?beta=true&trace=one",
            vec![
                HeaderPair::new("authorization", "Bearer client"),
                HeaderPair::new("x-extension", [0x80, 0x81]),
            ],
            json!({
                "model": "gpt",
                "messages": [{"role": "user", "content": "hello"}],
                "unknown": {"preserve": true}
            }),
        )
        .unwrap();

    transport.send(request).await.unwrap();
    let captured = captures.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, Method::POST);
    assert_eq!(captured[0].uri, "/v1/chat/completions?beta=true&trace=one");
    assert_eq!(captured[0].headers["x-extension"].as_bytes(), &[0x80, 0x81]);
    assert_eq!(
        captured[0].headers[INTERNAL_PHASE_HEADER],
        "internal-secret"
    );
    assert_eq!(captured[0].body["unknown"], json!({"preserve": true}));
    task.abort();
}

#[tokio::test]
async fn all_three_pig_protocols_reenter_proxy_natively() {
    let (provider_addr, captures, provider_task) =
        spawn_mock(any(native_phase_provider_handler)).await;
    let proxy_port = unused_port();
    let config = all_protocols_proxy_config(provider_addr, proxy_port);
    let proxy_task = tokio::spawn(async move {
        pigs_proxy::serve(config).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://127.0.0.1:{proxy_port}");
    let cases = [
        (
            "/v1/chat/completions?trace=chat",
            json!({
                "model": "gpt-pig",
                "messages": [
                    {"role": "system", "content": "system"},
                    {"role": "user", "content": "task"}
                ],
                "metadata": {"protocol": "chat"},
                "unknown": "chat"
            }),
            "/chat/completions",
            "/choices/0/message/content",
            "chat",
        ),
        (
            "/v1/messages?trace=anthropic",
            json!({
                "model": "gpt-pig",
                "system": [{"type": "text", "text": "system", "cache_control": {"type": "ephemeral"}}],
                "messages": [{"role": "user", "content": "task"}],
                "metadata": {"protocol": "anthropic"},
                "unknown": "anthropic"
            }),
            "/v1/messages",
            "/content/0/text",
            "anthropic",
        ),
        (
            "/responses?trace=responses",
            json!({
                "model": "gpt-pig",
                "instructions": {"policy": "system"},
                "input": "task",
                "metadata": {"protocol": "responses"},
                "unknown": "responses"
            }),
            "/responses",
            "/output/0/content/0/text",
            "responses",
        ),
    ];

    for (request_path, body, provider_path, output_pointer, marker) in cases {
        let url = format!("{base}{request_path}");
        let mut response = None;
        for _ in 0..50 {
            match client
                .post(&url)
                .header("authorization", "Bearer client")
                .header("x-client-extension", marker)
                .json(&body)
                .send()
                .await
            {
                Ok(value) => {
                    response = Some(value);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        let response = response.expect("proxy should start");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let response: Value = response.json().await.unwrap();
        assert_eq!(response["model"], "gpt-pig");
        assert_eq!(
            response.pointer(output_pointer).and_then(Value::as_str),
            Some("pre plan\n\nexecutor work\n\npost accepted")
        );

        let protocol_captures: Vec<CapturedRequest> = captures
            .lock()
            .unwrap()
            .iter()
            .filter(|capture| capture.uri == provider_path)
            .cloned()
            .collect();
        assert_eq!(protocol_captures.len(), 3);
        for capture in protocol_captures {
            assert_eq!(capture.method, Method::POST);
            assert_eq!(capture.body["model"], "gpt");
            assert_eq!(capture.body["metadata"], body["metadata"]);
            assert_eq!(capture.body["unknown"], body["unknown"]);
            assert_eq!(capture.headers["x-client-extension"], marker);
            assert!(!capture.headers.contains_key(INTERNAL_PHASE_HEADER));
        }
    }

    assert_eq!(captures.lock().unwrap().len(), 9);

    let unknown_results = [
        (
            "/v1/chat/completions",
            json!({
                "model": "gpt-pig",
                "messages": [
                    {"role": "user", "content": "task"},
                    {"role": "assistant", "content": null, "tool_calls": [{
                        "id": "missing", "type": "function",
                        "function": {"name": "read", "arguments": "{}"}
                    }]},
                    {"role": "tool", "tool_call_id": "missing", "content": "done"}
                ]
            }),
            "/error/type",
            "pigs_phase_error",
        ),
        (
            "/v1/messages",
            json!({
                "model": "gpt-pig",
                "messages": [
                    {"role": "user", "content": "task"},
                    {"role": "assistant", "content": [{
                        "type": "tool_use", "id": "missing", "name": "read", "input": {}
                    }]},
                    {"role": "user", "content": [{
                        "type": "tool_result", "tool_use_id": "missing", "content": "done"
                    }]}
                ]
            }),
            "/error/type",
            "pigs_phase_error",
        ),
        (
            "/responses",
            json!({
                "model": "gpt-pig",
                "input": [
                    {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "task"}]},
                    {"type": "function_call", "call_id": "missing", "name": "read", "arguments": "{}"},
                    {"type": "function_call_output", "call_id": "missing", "output": "done"}
                ]
            }),
            "/error/type",
            "pigs_phase_error",
        ),
    ];
    for (path, body, error_pointer, expected_type) in unknown_results {
        let response = client
            .post(format!("{base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        let error: Value = response.json().await.unwrap();
        assert_eq!(
            error.pointer(error_pointer).and_then(Value::as_str),
            Some(expected_type)
        );
    }
    assert_eq!(captures.lock().unwrap().len(), 9);
    proxy_task.abort();
    provider_task.abort();
}

#[tokio::test]
async fn invalid_internal_token_is_rejected_before_provider_dispatch() {
    let (provider_addr, captures, provider_task) = spawn_mock(any(capture_handler)).await;
    let proxy_port = unused_port();
    let config = proxy_config(provider_addr, proxy_port);
    let proxy_task = tokio::spawn(async move {
        pigs_proxy::serve(config).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{proxy_port}/v1/chat/completions");
    let body = json!({
        "model": "gpt",
        "messages": [{"role": "user", "content": "task"}]
    });
    let mut response = None;
    for _ in 0..50 {
        match client
            .post(&url)
            .header(INTERNAL_PHASE_HEADER, "forged")
            .json(&body)
            .send()
            .await
        {
            Ok(value) => {
                response = Some(value);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let response = response.expect("proxy should start");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(captures.lock().unwrap().is_empty());
    proxy_task.abort();
    provider_task.abort();
}

#[tokio::test]
async fn malformed_provider_stream_emits_only_an_outer_error_terminal() {
    let (provider_addr, captures, provider_task) =
        spawn_mock(any(malformed_streaming_phase_provider_handler)).await;
    let proxy_port = unused_port();
    let config = proxy_config(provider_addr, proxy_port);
    let proxy_task = tokio::spawn(async move {
        pigs_proxy::serve(config).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{proxy_port}/v1/chat/completions");
    let body = json!({
        "model": "gpt-pig",
        "stream": true,
        "messages": [{"role": "user", "content": "task"}]
    });
    let mut response = None;
    for _ in 0..50 {
        match client.post(&url).json(&body).send().await {
            Ok(value) => {
                response = Some(value);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let response = response.expect("proxy should start");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(stream.contains("pigs_phase_error"));
    assert!(stream.contains("without [DONE]"));
    assert!(!stream.contains("data: [DONE]"));
    assert!(!stream.contains("\"finish_reason\":\"stop\""));
    assert_eq!(captures.lock().unwrap().len(), 1);
    proxy_task.abort();
    provider_task.abort();
}

#[tokio::test]
async fn streaming_pig_request_forwards_deltas_and_hides_split_markers() {
    let (provider_addr, captures, provider_task) =
        spawn_mock(any(streaming_phase_provider_handler)).await;
    let proxy_port = unused_port();
    let config = proxy_config(provider_addr, proxy_port);
    let proxy_task = tokio::spawn(async move {
        pigs_proxy::serve(config).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{proxy_port}/v1/chat/completions");
    let body = json!({
        "model": "gpt-pig",
        "stream": true,
        "messages": [{"role": "user", "content": "do task"}]
    });
    let mut response = None;
    for _ in 0..50 {
        match client.post(&url).json(&body).send().await {
            Ok(value) => {
                response = Some(value);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let response = response.expect("proxy should start");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    let stream = response.text().await.unwrap();
    assert_eq!(
        visible_sse_text(&stream),
        "pre plan\n\nexecutor work\n\npost accepted"
    );
    assert!(!stream.contains("PIGEND"));
    assert!(!stream.contains("\\nPI"));
    assert_eq!(stream.matches("[DONE]").count(), 1);
    assert_eq!(stream.matches("\"finish_reason\":\"stop\"").count(), 1);

    let captured = captures.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert!(captured
        .iter()
        .all(|request| request.body["stream"] == true));
    proxy_task.abort();
    provider_task.abort();
}

#[tokio::test]
async fn pig_request_reenters_proxy_and_reaches_provider_protocol_natively() {
    let (provider_addr, captures, provider_task) = spawn_mock(any(phase_provider_handler)).await;
    let proxy_port = unused_port();
    let config = proxy_config(provider_addr, proxy_port);
    let proxy_task = tokio::spawn(async move {
        pigs_proxy::serve(config).await.unwrap();
    });

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{proxy_port}/v1/chat/completions?trace=client");
    let body = json!({
        "model": "gpt-pig",
        "stream": false,
        "messages": [
            {"role": "system", "content": "preserve system"},
            {"role": "user", "content": "do task"}
        ],
        "tools": [{"type": "function", "function": {"name": "run", "parameters": {"type": "object"}}}],
        "metadata": {"keep": true},
        "unknown": [1, 2, 3]
    });
    let mut response = None;
    for _ in 0..50 {
        match client
            .post(&url)
            .header("authorization", "Bearer client")
            .header("x-client-extension", "preserved")
            .json(&body)
            .send()
            .await
        {
            Ok(value) => {
                response = Some(value);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let response = response.expect("proxy should start");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["model"], "gpt-pig");
    assert_eq!(
        response["choices"][0]["message"]["content"],
        "pre plan\n\nexecutor work\n\npost accepted"
    );

    let captured = captures.lock().unwrap();
    assert_eq!(captured.len(), 3);
    for request in captured.iter() {
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.uri, "/chat/completions");
        assert_eq!(request.body["model"], "gpt");
        assert_eq!(request.body["stream"], false);
        assert_eq!(request.body["metadata"], json!({"keep": true}));
        assert_eq!(request.body["unknown"], json!([1, 2, 3]));
        assert_eq!(request.headers["x-client-extension"], "preserved");
        assert!(!request.headers.contains_key(INTERNAL_PHASE_HEADER));
    }

    proxy_task.abort();
    provider_task.abort();
}
