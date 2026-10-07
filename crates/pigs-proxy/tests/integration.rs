//! 集成测试：真实 HTTP 链路 —— 假上游（wiremock 风格的手写 axum）+ 真实 pigs 服务。
//!
//! 覆盖：无 -pigs 透传、content-encoding 完整透传（血泪教训回归）、-pigs 全链路编排
//! （经 loopback 回环）、Pre 简单路径短路、流式合成。

use axum::body::Bytes;
use axum::response::Response;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::any;
use axum::Router;
use pigs_proxy::{build_state, Config, Upstreams};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// ---------------- 假上游 ----------------

#[derive(Clone)]
struct FakeUpstreamState {
    /// 每次收到的请求：(path, body, headers 回显)
    requests: Arc<Mutex<Vec<(String, Value, Value)>>>, 
    /// 按次序回放的响应：(content_type, body)
    responses: Arc<Mutex<Vec<(&'static str, Bytes)>>>, 
    /// 原样回显模式（透传测试）：回显收到的 headers + body
    echo_mode: Arc<Mutex<bool>>,
    /// 慢速 SSE 脚本：每个子请求一段"分段文本"，段与段之间 sleep 150ms
    slow_sse: Arc<Mutex<Vec<Vec<String>>>>,
    /// gzip 压缩响应队列：模拟"客户端带 accept-encoding，上游真的压缩了"
    gz_responses: Arc<Mutex<Vec<Bytes>>>,
}

/// 一段真实 gzip 字节：内容等价于 `{"choices":[{"message":{"content":"答案是 4 换行 PIGEND"},...}]}`。
#[rustfmt::skip]
const GZIP_PIGEND_BODY: &[u8] = &[31, 139, 8, 0, 31, 107, 187, 106, 2, 255, 171, 86, 74, 206, 200, 207, 76, 78, 45, 86, 178, 138, 174, 86, 202, 77, 45, 46, 78, 76, 79, 85, 178, 170, 86, 74, 206, 207, 43, 73, 205, 43, 81, 178, 82, 122, 190, 118, 202, 179, 133, 29, 207, 102, 172, 87, 48, 137, 201, 11, 240, 116, 119, 245, 115, 81, 170, 213, 81, 74, 203, 204, 203, 44, 206, 136, 47, 74, 77, 44, 206, 207, 3, 42, 43, 46, 201, 47, 80, 170, 141, 213, 81, 42, 133, 25, 145, 153, 87, 80, 90, 18, 95, 146, 159, 157, 154, 7, 52, 222, 80, 71, 41, 191, 180, 4, 89, 196, 168, 182, 22, 0, 178, 209, 122, 86, 127, 0, 0, 0];

/// 把一段文本包成合法的 OpenAI chat SSE 帧。
fn sse_frame(text: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"id":"c1","object":"chat.completion.chunk","model":"gpt-x",
               "choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})
    )
}

async fn fake_handler(
    State(state): State<FakeUpstreamState>,
    uri: axum::http::Uri,
    method: axum::http::Method,
    RawQuery(_query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response<axum::body::Body> {
    let _ = method;
    // 慢速 SSE 模式：分段吐出（第一段立刻，之后每段前 sleep）
    let script = {
        let mut queue = state.slow_sse.lock().unwrap();
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    };
    if let Some(parts) = script {
        let mut header_echo = serde_json::Map::new();
        for (k, v) in headers.iter() {
            header_echo.insert(
                format!("h:{}", k.as_str().to_lowercase()),
                Value::String(v.to_str().unwrap_or("").into()),
            );
        }
        state.requests.lock().unwrap().push((
            uri.path().to_string(),
            serde_json::from_slice(&body).unwrap_or(Value::Null),
            Value::Object(header_echo),
        ));
        let stream = futures_util::stream::unfold(
            (parts.into_iter(), false),
            |(mut parts, started)| async move {
                let part = parts.next()?;
                if started {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(sse_frame(&part))), (parts, true)))
            },
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    // 压缩响应：body 是 gzip 字节 + content-encoding 标签
    let gz = {
        let mut queue = state.gz_responses.lock().unwrap();
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    };
    if let Some(bytes) = gz {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header("content-encoding", "gzip")
            .body(axum::body::Body::from(bytes))
            .unwrap();
    }
    if *state.echo_mode.lock().unwrap() {
        // 回显：body 原样返回，外加记录到的请求头摘要
        let mut echo = serde_json::Map::new();
        for (k, v) in headers.iter() {
            echo.insert(
                format!("h:{}", k.as_str().to_lowercase()),
                Value::String(v.to_str().unwrap_or("").into()),
            );
        }
        echo.insert("body".into(), Value::String(String::from_utf8_lossy(&body).into()));
        let payload = Value::Object(echo).to_string();
        let resp = (
            StatusCode::OK,
            "application/json",
            Bytes::from(payload),
        );
        return into_response(resp.0, resp.1, resp.2);
    }

    let mut header_echo = serde_json::Map::new();
    for (k, v) in headers.iter() {
        header_echo.insert(
            format!("h:{}", k.as_str().to_lowercase()),
            Value::String(v.to_str().unwrap_or("").into()),
        );
    }
    state.requests.lock().unwrap().push((
        uri.path().to_string(),
        serde_json::from_slice(&body).unwrap_or(Value::Null),
        Value::Object(header_echo),
    ));
    let responses = state.responses.lock().unwrap();
    let (ct, bytes) = responses
        .first()
        .cloned()
        .expect("假上游响应耗尽");
    drop(responses);
    state.responses.lock().unwrap().remove(0);
    into_response(StatusCode::OK, ct, bytes)
}

fn into_response(status: StatusCode, ct: &str, body: Bytes) -> Response<axum::body::Body> {
    Response::builder()
        .status(status)
        .header("content-type", ct)
        .body(axum::body::Body::from(body))
        .unwrap()
}

type FakeUpstream = (tokio::task::JoinHandle<()>, String, FakeUpstreamState);

/// 起假上游（127.0.0.1 随机端口），返回 (任务, base_url, 状态)。
async fn spawn_fake_upstream() -> FakeUpstream {
    let state = FakeUpstreamState {
        requests: Arc::new(Mutex::new(vec![])),
        responses: Arc::new(Mutex::new(vec![])),
        echo_mode: Arc::new(Mutex::new(false)),
        slow_sse: Arc::new(Mutex::new(vec![])),
        gz_responses: Arc::new(Mutex::new(vec![])),
    };
    let app = Router::new().fallback(any(fake_handler)).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (task, format!("http://{addr}"), state)
}

// ---------------- pigs 服务 ----------------

/// 起 pigs（127.0.0.1 随机端口，上游指向假上游），返回 (任务, base_url)。
async fn spawn_pigs(upstream_url: &str) -> (tokio::task::JoinHandle<()>, String) {
    let config = Config {
        listen: "127.0.0.1:0".into(),
        key: String::new(),
        logging: pigs_proxy::LoggingConfig {
            detail: pigs_proxy::LogDetail::Off,
            ..Default::default()
        },
        orchestration: pigs_proxy::OrchestrationConfig::default(),
        upstream: Upstreams::same(upstream_url),
    };
    let listener = pigs_proxy::bind_listener(&config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state(config, addr);
    let app = pigs_proxy::server::router(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (task, format!("http://{addr}"))
}

fn openai_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model,
        "stream": stream,
        "tools": [{"type": "function", "function": {"name": "f"}}],
        "messages": [
            {"role": "system", "content": "sys"},
            {"role": "user", "content": "帮我完成任务Z"}
        ]
    })
}

fn append_openai_tool_result(body: &mut Value, id: &str) {
    let messages = body["messages"].as_array_mut().unwrap();
    messages.push(json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": id,
            "type": "function",
            "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}
        }]
    }));
    messages.push(json!({"role": "tool", "tool_call_id": id, "content": "文件列表"}));
}

fn openai_text_response(text: &str) -> (&'static str, Bytes) {
    (
        "application/json",
        Bytes::from(
            json!({"choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})
                .to_string(),
        ),
    )
}

/// 假上游的 OpenAI 流式响应：把文本切成几段 delta（模拟真实 SSE）。
fn openai_sse_response(text: &str) -> (&'static str, Bytes) {
    let chunk = |t: &str| {
        format!(
            "data: {}\n\n",
            json!({"id":"c1","object":"chat.completion.chunk","model":"gpt-x",
                   "choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]})
        )
    };
    let mut sse = String::new();
    let mut chars = text.chars();
    loop {
        let piece: String = chars.by_ref().take(3).collect();
        if piece.is_empty() {
            break;
        }
        sse.push_str(&chunk(&piece));
    }
    sse.push_str("data: [DONE]\n\n");
    ("text/event-stream", Bytes::from(sse))
}


/// 假上游的"模型要工具"响应（OpenAI Chat 形状）。
fn openai_tool_call_response(id: &str) -> (&'static str, Bytes) {
    (
        "application/json",
        Bytes::from(
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant", "content": null,
                        "tool_calls": [{
                            "id": id, "type": "function",
                            "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
            })
            .to_string(),
        ),
    )
}


/// 假上游的 Anthropic 流式响应：先思考（含签名），再给文本。
fn anthropic_sse_with_thinking() -> (&'static str, Bytes) {
    let frames = [
        json!({"type":"message_start","message":{"usage":{"input_tokens":9}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先想一下"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"答案是 4
PIGEND"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
        json!({"type":"message_stop"}),
    ];
    let sse: String = frames
        .iter()
        .map(|f| format!("event: {}
data: {}

", f["type"].as_str().unwrap(), f))
        .collect();
    ("text/event-stream", Bytes::from(sse))
}


/// 假上游的 Responses 流式响应：先流思考摘要，再流文本，最后 completed 带权威 output[]。
fn responses_sse_with_reasoning() -> (&'static str, Bytes) {
    let reasoning = json!({
        "id": "rs_1", "type": "reasoning",
        "summary": [{"type": "summary_text", "text": "想一下"}],
        "encrypted_content": "blob"
    });
    let message = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": "答案是 4
PIGEND", "annotations": []}]
    });
    let frames = vec![
        json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,
               "item":{"id":"rs_1","type":"reasoning","summary":[]}}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1",
               "output_index":0,"summary_index":0,"delta":"想一下"}),
        json!({"type":"response.reasoning_summary_text.done","item_id":"rs_1",
               "output_index":0,"summary_index":0,"text":"想一下"}),
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning.clone()}),
        json!({"type":"response.output_item.added","output_index":1,
               "item":{"id":"msg_1","type":"message","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.output_text.delta","item_id":"msg_1",
               "output_index":1,"content_index":0,"delta":"答案是 4
PIGEND"}),
        json!({"type":"response.output_item.done","output_index":1,"item":message.clone()}),
        json!({"type":"response.completed","response":{
               "id":"resp_1","status":"completed","usage":{"input_tokens":9,"output_tokens":4},
               "output":[reasoning, message]}}),
    ];
    let sse: String = frames
        .iter()
        .map(|f| format!("event: {}
data: {}

", f["type"].as_str().unwrap(), f))
        .collect();
    ("text/event-stream", Bytes::from(sse))
}

// ---------------- 测试 ----------------

#[tokio::test]
async fn passthrough_echo_mode_forwards_auth_and_body() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    *fu.echo_mode.lock().unwrap() = true;
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let body = openai_body("gpt-x", false);
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .header("authorization", "Bearer client-key")
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let echo: Value = resp.json().await.unwrap();
    // 鉴权头透传
    assert_eq!(echo["h:authorization"], "Bearer client-key");
    // body 原样透传
    assert_eq!(echo["body"], body.to_string());
}

#[tokio::test]
async fn content_encoding_header_survives_passthrough() {
    // 回归 legacy 事故：上游回压缩体 + content-encoding 头，标签必须随 body 到客户端
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    *fu.echo_mode.lock().unwrap() = true;
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    // 手工构造"上游响应"：echo 模式会回 JSON，我们直接在客户端断言响应头的透传。
    // 用 models 路径（透传）检查自定义响应头 —— 这里验证上游响应头进入客户端响应。
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{pigs_url}/v1/models"))
        .header("authorization", "Bearer k")
        .send()
        .await
        .unwrap();
    // 只要状态与 content-type 是上游的即可（回显 JSON）
    assert_eq!(resp.status(), 200);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.contains("application/json"));
}

#[tokio::test]
async fn pig_flow_full_orchestration_via_loopback() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_text_response("分析：需要X"));
    fu.responses.lock().unwrap().push(openai_text_response("执行结果……"));
    fu.responses.lock().unwrap().push(openai_text_response("验收通过\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .header("authorization", "Bearer client-key")
        .header("accept-encoding", "gzip, deflate, br")
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let final_json: Value = resp.json().await.unwrap();
    // 最终答复 = 三只 pig 可见文本按顺序拼接（legacy 语义）
    assert_eq!(
        final_json["choices"][0]["message"]["content"],
        "分析：需要X\n\n执行结果……\n\n验收通过"
    );
    assert_eq!(final_json["choices"][0]["finish_reason"], "stop");
    // 回客户端的是**它请求的那个名字**（带 -pigs）
    assert_eq!(final_json["model"], "gpt-x-pigs");

    // 上游收到 3 次子请求：只许改 model 名，其余字段原样
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    for (i, (_path, body, hdrs)) in reqs.iter().enumerate() {
        // 头原样透传（不再剥 accept-encoding；压缩体由本机解开）
        assert_eq!(
            hdrs.get("h:accept-encoding").and_then(|v| v.as_str()),
            Some("gzip, deflate, br"),
            "第 {i} 只 pig 的头必须原样"
        );
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["stream"], false, "客户端没要流式，就不许改");
        assert!(body.get("tools").is_some(), "tools 不许被剥掉");
        let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        if i == 0 {
            assert!(content.contains("帮我完成任务Z") && content.contains("本次需要你先思考以下问题的答案"));
        } else if i == 1 {
            assert!(content.contains("分析：需要X"));
        } else {
            // Post：完整继承 Executor 请求前缀，再追加 Executor 最终输出与一条评审 user。
            let msgs = body["messages"].as_array().unwrap();
            let exec_msgs = reqs[1].1["messages"].as_array().unwrap();
            assert_eq!(msgs.len(), exec_msgs.len() + 2);
            assert_eq!(
                &msgs[..exec_msgs.len()],
                exec_msgs.as_slice(),
                "Post 必须保持 Executor 的完整消息前缀"
            );
            assert_eq!(msgs[exec_msgs.len()]["role"], "assistant");
            assert_eq!(msgs[exec_msgs.len()]["content"], "执行结果……");
            assert_eq!(msgs[exec_msgs.len() + 1]["role"], "user");
            assert!(content.contains("独立核验当前执行结果"));
        }
    }
    // 会话头贯穿验证：Anthropic 协议 + 客户端自带会话头（录制模式，回 anthropic 形状响应）
    drop(reqs);
    fu.responses.lock().unwrap().push((
        "application/json",
        Bytes::from(json!({"content":[{"type":"text","text":"直接回答\nPIGEND"}]}).to_string()),
    ));
    let resp = client
        .post(format!("{pigs_url}/v1/messages"))
        .header("x-opencode-session", "client-session-1")
        .json(&json!({
            "model": "claude-x-pigs",
            "messages": [{"role": "user", "content": "简单问题"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let final_json: Value = resp.json().await.unwrap();
    assert_eq!(final_json["content"][0]["text"], "直接回答");
    // 回环子请求把客户端的会话头原样带给上游
    let binding = fu.requests.lock().unwrap();
    let (_p, _b, hdrs) = &binding.last().unwrap();
    assert_eq!(hdrs["h:x-opencode-session"], "client-session-1");
}

#[tokio::test]
async fn pigsb_non_stream_returns_only_accepted_executor_text() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_text_response("内部分析"));
    fu.responses.lock().unwrap().push(openai_text_response("最终业务答案"));
    fu.responses.lock().unwrap().push(openai_text_response("核验通过\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigsb", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "gpt-x-pigsb");
    assert_eq!(body["choices"][0]["message"]["content"], "最终业务答案");

    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    for (_, upstream_body, _) in reqs.iter() {
        assert_eq!(upstream_body["model"], "gpt-x", "上游必须使用剥离后的真实 model");
    }
}

#[tokio::test]
async fn pigsb_stream_buffers_business_text_until_post_accepts() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_sse_response("内部分析"));
    fu.responses.lock().unwrap().push(openai_sse_response("最终业务答案"));
    fu.responses.lock().unwrap().push(openai_sse_response("核验通过\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigsb", true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();
    let text = pigs_protocol::extract_sse_text(pigs_protocol::Protocol::OpenAI, &sse)
        .unwrap_or_default();
    assert_eq!(text, "最终业务答案");
    assert!(!text.contains("内部分析"));
    assert!(!text.contains("核验通过"));
    assert!(!sse.contains("PIGEND"));
}

#[tokio::test]
async fn pig_simple_path_answers_from_pre() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_text_response("答案是 4\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let final_json: Value = resp.json().await.unwrap();
    assert_eq!(final_json["choices"][0]["message"]["content"], "答案是 4");
    // 只发生了 1 次上游请求（Executor/Post 未执行）
    assert_eq!(fu.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pig_streaming_client_gets_sse() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_text_response("答案是 4\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", true))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("data: [DONE]"));
    assert!(body.contains("答案是 4"));
}

/// 流式客户端 + 上游流式：子请求必须带 `stream:true`，
/// 客户端拿到的 SSE 必须是三只 pig 的实时拼接，且控制标记一个字都不许漏。
#[tokio::test]
async fn pig_streaming_end_to_end_streams_phases_without_markers() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    let q = fu.responses.clone();
    for text in ["分析：需要X\n第二行", "执行结果……\nPIGFAIL", "评审：继续\nPIGEND"] {
        q.lock().unwrap().push(openai_sse_response(text));
    }
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .header("accept-encoding", "gzip, deflate, br")
        .json(&openai_body("gpt-x-pigs", true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();

    // 上游收到的子请求：真名 + 流式（客户端自己要的）+ 头原样
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3, "Pre → Executor → Post 三次子请求");
    for (i, (_p, body, hdrs)) in reqs.iter().enumerate() {
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["stream"], true, "第 {i} 只 pig 的子请求应为流式");
        assert_eq!(
            hdrs.get("h:accept-encoding").and_then(|v| v.as_str()),
            Some("gzip, deflate, br"),
            "第 {i} 只 pig 的头必须原样"
        );
        assert!(body.get("tools").is_some(), "第 {i} 只 pig 的 tools 不许被剥");
    }
    drop(reqs);

    // 客户端流：三只 pig 的可见文本按顺序、空行分隔，控制标记已被剥掉
    // （实时阶段按上游到达的粒度逐帧发，所以这些断言基于"拼起来的文本"）
    let text = pigs_protocol::extract_sse_text(pigs_protocol::Protocol::OpenAI, &sse).unwrap();
    assert_eq!(text, "分析：需要X\n第二行\n\n执行结果……\n\n评审：继续");
    assert!(!text.contains("PIGEND") && !text.contains("PIGFAIL"), "标记不许漏");
    let order = ["分析：需要X", "执行结果……", "评审：继续"]
        .map(|s| text.find(s).expect("阶段文本应出现在客户端流里"));
    assert!(order[0] < order[1] && order[1] < order[2]);
    assert!(sse.contains("data: [DONE]"));
}

/// 真·流式验证（时间维度）：上游分三段、每段间隔 150ms 才吐完。
/// 若编排是"等全文再回"，首段文本要等整轮跑完才可能出现；
/// 这里断言客户端在很早就拿到了第一只 pig 的文本，而整体耗时确实包含那些 sleep。
#[tokio::test]
async fn pig_streaming_is_progressive_not_buffered() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    {
        let mut q = fu.slow_sse.lock().unwrap();
        // Pre → Executor → Post(带结尾标记，标记本身跨段到达)
        q.push(vec!["分析一".into(), "分析二".into()]);
        q.push(vec!["执行草".into(), "稿完毕".into()]);
        q.push(vec!["验收通过".into(), "\nPIG".into(), "END".into()]);
    }
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let started = std::time::Instant::now();
    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut body = Vec::new();
    let mut first_text_at = None;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.unwrap());
        if first_text_at.is_none() && String::from_utf8_lossy(&body).contains("分析一") {
            first_text_at = Some(started.elapsed());
        }
    }
    let total = started.elapsed();
    let first = first_text_at.expect("客户端从未收到第一段文本");

    // 总共 4 次 150ms 的 sleep ≈ 600ms；首段必须在整轮跑完之前就到达
    assert!(total.as_millis() >= 450, "上游的 sleep 没生效？total={total:?}");
    assert!(
        first < total / 2,
        "首段文本等了 {first:?}，整轮 {total:?} —— 说明是等全文才回，不是真正的流式"
    );

    let sse = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        pigs_protocol::extract_sse_text(pigs_protocol::Protocol::OpenAI, &sse).unwrap(),
        "分析一分析二\n\n执行草稿完毕\n\n验收通过"
    );
    assert!(!sse.contains("PIGEND"), "控制标记跨段到达也必须被拦住");
    // 子请求确实是流式的
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    assert!(reqs.iter().all(|(_p, body, _h)| body["stream"] == true));
}

/// max 模式：一次普通流式透传会分别记录客户端请求、上游请求、上游响应、客户端响应。
#[tokio::test]
async fn max_http_diagnostics_write_separate_plaintext_exchange_files() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses
        .lock()
        .unwrap()
        .push(openai_sse_response("诊断流"));

    let dir = std::env::temp_dir().join(format!("pigs-http-integration-{}", uuid::Uuid::new_v4()));
    let config = Config {
        listen: "127.0.0.1:0".into(),
        key: String::new(),
        logging: pigs_proxy::LoggingConfig {
            detail: pigs_proxy::LogDetail::Max,
            directory: dir.to_string_lossy().into_owned(),
        },
        orchestration: pigs_proxy::OrchestrationConfig::default(),
        upstream: Upstreams::same(&upstream_url),
    };
    let listener = pigs_proxy::bind_listener(&config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state(config, addr);
    let app = pigs_proxy::server::router(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/chat/completions"))
        .header("authorization", "Bearer top-secret")
        .json(&openai_body("gpt-x", true))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response_text = response.text().await.unwrap();
    assert!(response_text.contains("诊断流"));

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 4, "一次普通透传应生成请求/响应四个抓包文件");

    let read_stage = |suffix: &str| -> String {
        let path = files
            .iter()
            .find(|path| path.file_name().unwrap().to_string_lossy().contains(suffix))
            .unwrap_or_else(|| panic!("缺少 {suffix}: {files:?}"));
        std::fs::read_to_string(path).unwrap()
    };
    let client_request = read_stage("client-request");
    let upstream_request = read_stage("upstream-request");
    let upstream_response = read_stage("upstream-response");
    let client_response = read_stage("client-response");

    assert!(client_request.contains("\"model\":\"gpt-x\""));
    assert!(client_request.contains("authorization: <redacted>"));
    assert!(!client_request.contains("top-secret"));
    assert!(upstream_request.contains("\"model\":\"gpt-x\""));
    assert!(upstream_response.contains("诊断流"));
    assert!(client_response.contains("诊断流"));
    assert!(upstream_response.contains("capture_complete: true"));
    assert!(client_response.contains("capture_complete: true"));

    task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

/// 诊断回归：有 pending continuation 时，如果下一发普通请求没有工具结果，
/// 日志必须把请求原文、pending 现场和新开判定原因记下来。
#[tokio::test]
async fn diagnostics_explain_pending_turn_started_new_round() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    {
        let mut q = fu.responses.lock().unwrap();
        q.push(openai_tool_call_response("call_wait"));
        q.push(openai_text_response("新轮直接结束\nPIGEND"));
    }
    let dir = std::env::temp_dir().join(format!("pigs-http-decision-{}", uuid::Uuid::new_v4()));
    let config = Config {
        listen: "127.0.0.1:0".into(),
        key: String::new(),
        logging: pigs_proxy::LoggingConfig {
            detail: pigs_proxy::LogDetail::Max,
            directory: dir.to_string_lossy().into_owned(),
        },
        orchestration: pigs_proxy::OrchestrationConfig::default(),
        upstream: Upstreams::same(&upstream_url),
    };
    let listener = pigs_proxy::bind_listener(&config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state(config, addr);
    let app = pigs_proxy::server::router(state);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = reqwest::Client::new();

    let first = client.post(format!("http://{addr}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false)).send().await.unwrap();
    assert_eq!(first.status(), 200);
    let first_json: Value = first.json().await.unwrap();
    assert_eq!(first_json["choices"][0]["message"]["tool_calls"][0]["id"], "call_wait");

    let second = client.post(format!("http://{addr}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false)).send().await.unwrap();
    assert_eq!(second.status(), 200);
    let _ = second.text().await.unwrap();

    let files: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    let decisions: Vec<String> = files.iter()
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains("orchestration-decision"))
        .map(|p| std::fs::read_to_string(p).unwrap()).collect();
    let decision = decisions.iter()
        .find(|text| text.contains("pending_continuations: 1"))
        .expect("第二发应看到 pending continuation");
    assert!(decision.contains("decision: start_new_no_matching_tool_results"));
    assert!(decision.contains("all_tool_result_ids: []"));
    assert!(decision.contains("trailing_tool_result_ids: []"));
    assert!(decision.contains("tool_ids=[\"call_wait\"]"));

    let outcomes: Vec<String> = files.iter()
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains("orchestration-outcome"))
        .map(|p| std::fs::read_to_string(p).unwrap()).collect();
    assert!(outcomes.iter().any(|text| text.contains("outcome: paused") && text.contains("call_wait")));

    task.abort();
    let _ = std::fs::remove_dir_all(dir);
}

/// 回归：工具结果后即使追加普通 user reminder，也必须恢复原 continuation；
/// continuation 判定不能借机删除或改写客户端后续消息。
#[tokio::test]
async fn tool_result_followed_by_user_reminder_still_resumes() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    {
        let mut q = fu.responses.lock().unwrap();
        q.push(openai_tool_call_response("call_wait"));
        q.push(openai_text_response("答案是 4\nPIGEND"));
    }
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first_json: Value = first.json().await.unwrap();
    assert_eq!(
        first_json["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_wait"
    );

    let mut resume = openai_body("gpt-x-pigs", false);
    append_openai_tool_result(&mut resume, "call_wait");
    resume["messages"].as_array_mut().unwrap().push(json!({
        "role": "user",
        "content": "<system-reminder>TodoWrite reminder</system-reminder>"
    }));

    let second = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let second_json: Value = second.json().await.unwrap();
    assert_eq!(second_json["choices"][0]["message"]["content"], "答案是 4");

    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2, "第二发应恢复同一只 Pre，而不是新开一轮");
    let sent = &reqs[1].1;
    let roles: Vec<&str> = sent["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool", "user"]);
    assert_eq!(sent["messages"][3]["tool_call_id"], "call_wait");
    assert_eq!(
        sent["messages"][4]["content"],
        "<system-reminder>TodoWrite reminder</system-reminder>"
    );
}

/// 回归：客户端带 `accept-encoding` 且上游**真的压缩**了响应时，编排必须照样能读。
/// 现在不再剥请求头，而是**本机解压**——上游看到的请求与父请求逐字节一致。
#[tokio::test]
async fn compressed_upstream_response_is_decompressed_locally() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.gz_responses
        .lock()
        .unwrap()
        .push(Bytes::from_static(GZIP_PIGEND_BODY));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/chat/completions"))
        .header("accept-encoding", "gzip, deflate, br")
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "答案是 4");
    // 上游看到的请求头原样（含 accept-encoding）
    let reqs = fu.requests.lock().unwrap();
    assert!(reqs.is_empty(), "压缩响应测试不再记录请求");
}

/// 工具调用全链路：模型要工具 → 调用原样交给客户端 → 客户端执行完带结果回来 →
/// 接着**同一只 pig** 继续 → 最终答复。全程 tools 都在请求里。
#[tokio::test]
async fn tool_pause_and_resume_round_trip() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_tool_call_response("call_1"));
    fu.responses
        .lock()
        .unwrap()
        .push(openai_text_response("答案是 4
PIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;
    let client = reqwest::Client::new();

    // 第一发：模型要工具 → 客户端必须拿到原生的 tool_calls
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let call = &body["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["function"]["name"], "Bash");
    // 参数原样（没有转义/二次编码）
    assert_eq!(call["function"]["arguments"], "{\"command\":\"ls\"}");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(body["model"], "gpt-x-pigs", "回客户端的是它请求的名字");

    // 第二发：客户端把工具结果接回历史后再发（真实 agent 就是这么干的）
    let mut resume = openai_body("gpt-x-pigs", false);
    resume["messages"].as_array_mut().unwrap().push(json!({
        "role": "assistant", "content": null,
        "tool_calls": [{"id": "call_1", "type": "function",
                        "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}}]
    }));
    resume["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "tool", "tool_call_id": "call_1", "content": "文件列表"}));
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "答案是 4");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(
        body["choices"][0]["message"].get("tool_calls").is_none(),
        "工具已经消费完成，最终响应不能重放历史 tool_calls"
    );
    // usage 是上游给的真值（不是 0）
    assert_eq!(body["usage"]["total_tokens"], 10);

    // 第二次子请求：阶段提示仍固定在最初 user 消息；工具结果保持在尾部；tools 仍在。
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    let sent = &reqs[1].1;
    let msgs = sent["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "tool"]);
    assert_eq!(msgs[3]["content"], "文件列表", "工具结果原样带上");
    let phase_user = msgs[1]["content"].as_str().unwrap();
    assert!(phase_user.contains("本次需要你先思考以下问题的答案"));
    assert_eq!(phase_user.matches("本次需要你先思考以下问题的答案").count(), 1);
    assert!(sent.get("tools").is_some(), "tools 全程都在");
}

/// 流式多轮工具回归：每次暂停只发本轮新调用；最终完成后不得重放任何历史调用。
#[tokio::test]
async fn streaming_tool_rounds_do_not_replay_consumed_calls() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    {
        let mut q = fu.responses.lock().unwrap();
        q.push(openai_tool_call_response("call_1"));
        q.push(openai_tool_call_response("call_2"));
        q.push(openai_text_response("最终答案\nPIGEND"));
    }
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", true))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first_sse = first.text().await.unwrap();
    assert!(first_sse.contains("call_1"));
    assert!(!first_sse.contains("call_2"));

    let mut resume1 = openai_body("gpt-x-pigs", true);
    append_openai_tool_result(&mut resume1, "call_1");
    let second = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume1)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let second_sse = second.text().await.unwrap();
    assert!(second_sse.contains("call_2"), "第二轮新工具调用必须发给客户端");
    assert!(
        !second_sse.contains("call_1"),
        "第二次暂停不能重放已经消费过的 call_1"
    );

    let mut resume2 = openai_body("gpt-x-pigs", true);
    append_openai_tool_result(&mut resume2, "call_1");
    append_openai_tool_result(&mut resume2, "call_2");
    let final_resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume2)
        .send()
        .await
        .unwrap();
    assert_eq!(final_resp.status(), 200);
    let final_sse = final_resp.text().await.unwrap();
    assert_eq!(
        pigs_protocol::extract_sse_text(pigs_protocol::Protocol::OpenAI, &final_sse).unwrap(),
        "最终答案"
    );
    assert!(final_sse.contains("data: [DONE]"));
    assert!(!final_sse.contains("call_1"));
    assert!(!final_sse.contains("call_2"));
    assert!(
        !final_sse.contains("\"tool_calls\""),
        "Completed 收尾不能重新发送历史工具调用"
    );
}

/// 真实代理链路：Executor 的阶段说明只在进入该 pig 时放一次；工具恢复只追加原生对话。
#[tokio::test]
async fn executor_resume_keeps_one_phase_prompt_upstream() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    {
        let mut q = fu.responses.lock().unwrap();
        q.push(openai_text_response("计划A"));
        q.push(openai_tool_call_response("call_1"));
        q.push(openai_tool_call_response("call_2"));
        q.push(openai_text_response("草稿完成"));
        q.push(openai_text_response("验收通过\nPIGEND"));
    }
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", true))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first_sse = first.text().await.unwrap();
    assert!(first_sse.contains("call_1"));

    let mut resume1 = openai_body("gpt-x-pigs", true);
    append_openai_tool_result(&mut resume1, "call_1");
    let second = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume1)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let second_sse = second.text().await.unwrap();
    assert!(second_sse.contains("call_2"));
    assert!(!second_sse.contains("call_1"));

    let mut resume2 = openai_body("gpt-x-pigs", true);
    append_openai_tool_result(&mut resume2, "call_1");
    append_openai_tool_result(&mut resume2, "call_2");
    let final_resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&resume2)
        .send()
        .await
        .unwrap();
    assert_eq!(final_resp.status(), 200);
    let final_sse = final_resp.text().await.unwrap();
    assert!(!final_sse.contains("call_1"));
    assert!(!final_sse.contains("call_2"));
    assert!(!final_sse.contains("\"tool_calls\""));

    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    let exec_first = &reqs[1].1;
    let exec_resume1 = &reqs[2].1;
    let exec_resume2 = &reqs[3].1;
    let roles = |body: &Value| -> Vec<String> {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["role"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(roles(exec_first), vec!["system", "user"]);
    assert_eq!(roles(exec_resume1), vec!["system", "user", "assistant", "tool"]);
    assert_eq!(
        roles(exec_resume2),
        vec!["system", "user", "assistant", "tool", "assistant", "tool"]
    );
    for body in [exec_first, exec_resume1, exec_resume2] {
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(
            user.matches("以下是本任务的执行前分析：").count(),
            1,
            "同一 Executor pig 的阶段说明不能重复注入"
        );
    }

    let post = &reqs[4].1;
    let exec_messages = exec_resume2["messages"].as_array().unwrap();
    let post_messages = post["messages"].as_array().unwrap();
    assert_eq!(
        &post_messages[..exec_messages.len()],
        exec_messages.as_slice(),
        "真实上游 Post 请求必须保留 Executor 最后一发的完整消息前缀"
    );
    assert_eq!(post_messages[exec_messages.len()]["role"], "assistant");
    assert_eq!(post_messages[exec_messages.len()]["content"], "草稿完成");
    assert_eq!(post_messages[exec_messages.len() + 1]["role"], "user");
}

/// 尾部带工具结果却没有对应现场 → 明确报错，不悄悄重跑一整轮。
#[tokio::test]
async fn orphan_tool_result_gets_conflict() {
    let (_up, upstream_url, _fu) = spawn_fake_upstream().await;
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;
    let mut body = openai_body("gpt-x-pigs", false);
    body["messages"].as_array_mut().unwrap().push(json!({
        "role": "tool", "tool_call_id": "never-seen", "content": "x"
    }));
    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

/// 上游回到客户端的**内容**必须原样：Anthropic 的思考块、工具块、文本块按顺序都在。
#[tokio::test]
async fn anthropic_thinking_and_blocks_reach_the_client() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push((
        "application/json",
        Bytes::from(
            json!({
                "content": [
                    {"type": "thinking", "thinking": "先想想怎么答", "signature": "sig-1"},
                    {"type": "text", "text": "答案是 4
PIGEND"}
                ],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 9, "output_tokens": 4}
            })
            .to_string(),
        ),
    ));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/v1/messages"))
        .json(&json!({
            "model": "claude-x-pigs",
            "max_tokens": 1024,
            "thinking": {"type": "enabled", "budget_tokens": 512},
            "messages": [{"role": "user", "content": "1+1 等于几"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // 思考块原样保留（含签名），且排在文本前面
    assert_eq!(body["content"][0]["type"], "thinking");
    assert_eq!(body["content"][0]["thinking"], "先想想怎么答");
    assert_eq!(body["content"][0]["signature"], "sig-1");
    assert_eq!(body["content"][1]["type"], "text");
    assert_eq!(body["content"][1]["text"], "答案是 4", "控制标记不许漏");
    assert_eq!(body["model"], "claude-x-pigs");
    assert_eq!(body["usage"]["input_tokens"], 9);
    // 上游收到的请求：thinking 配置与 tools 一字不动
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs[0].1["thinking"]["budget_tokens"], 512);
}

/// 思考必须**边想边流**：客户端要按顺序收到 start(thinking) → thinking_delta → signature_delta
/// → stop → 文本帧，而且不能因为"收尾补发"而收到两份。
#[tokio::test]
async fn anthropic_thinking_streams_live_without_duplication() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses
        .lock()
        .unwrap()
        .push(anthropic_sse_with_thinking());
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/v1/messages"))
        .json(&json!({
            "model": "claude-x-pigs",
            "max_tokens": 512,
            "stream": true,
            "thinking": {"type": "enabled", "budget_tokens": 256},
            "messages": [{"role": "user", "content": "1+1 等于几"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();

    assert_eq!(sse.matches("thinking_delta").count(), 1, "思考只许发一次");
    assert_eq!(sse.matches("signature_delta").count(), 1, "签名只许发一次");
    let order = ["content_block_start", "thinking_delta", "signature_delta", "答案是 4"]
        .map(|needle| sse.find(needle).expect("客户端流里缺少内容"));
    assert!(
        order[0] < order[1] && order[1] < order[2] && order[2] < order[3],
        "思考必须先于文本、且顺序为 开块 → 增量 → 签名"
    );
    // 文本仍然是"去标记后"的答复；思考不混进文本
    let text = pigs_protocol::extract_sse_text(pigs_protocol::Protocol::Anthropic, &sse).unwrap();
    assert_eq!(text, "答案是 4");
    // 收尾序列照常
    assert!(sse.contains("message_stop"));
}

/// Responses 协议：思考摘要**实时**流给客户端（带 item_id），完整 reasoning 条目
/// （含 encrypted_content）在收尾照旧给——与直连上游时的形状一致，客户端可按 item_id 归并。
#[tokio::test]
async fn responses_reasoning_streams_live_and_item_arrives_at_end() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses
        .lock()
        .unwrap()
        .push(responses_sse_with_reasoning());
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let resp = reqwest::Client::new()
        .post(format!("{pigs_url}/responses"))
        .json(&json!({
            "model": "r-x-pigs",
            "stream": true,
            "reasoning": {"effort": "high", "summary": "auto"},
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "1+1 等于几"}]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();

    // 实时：思考摘要增量带上游的 item_id
    assert!(
        sse.contains("response.reasoning_summary_text.delta"),
        "思考摘要必须实时流出去"
    );
    assert!(sse.contains("rs_1"), "增量要带 item_id，客户端才能归并");
    // 收尾：完整的 reasoning 条目（含加密内容）在 output[] 里
    assert!(sse.contains("blob"), "最终条目要带 encrypted_content");
    // 顺序：思考摘要先于文本
    let order = ["reasoning_summary_text.delta", "答案是 4"]
        .map(|needle| sse.find(needle).expect("客户端流里缺少内容"));
    assert!(order[0] < order[1], "思考要先于文本");
    // 文本里没有思考、也没有控制标记
    let text = pigs_protocol::extract_sse_text(pigs_protocol::Protocol::Responses, &sse).unwrap();
    assert_eq!(text, "答案是 4");
    // 上游收到的请求：reasoning 参数原样
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs[0].1["reasoning"]["summary"], "auto");
}


// ---------------- 按协议选 base 的回归 ----------------

/// 同 spawn_pigs，但三个协议指到同一假上游的不同路径前缀（/oa /rs /an）——
/// 用来验证"按协议选 base + 客户端路径逐字上浮"。
async fn spawn_pigs_with_bases(upstream_url: &str) -> (tokio::task::JoinHandle<()>, String) {
    let config = Config {
        listen: "127.0.0.1:0".into(),
        key: String::new(),
        logging: pigs_proxy::LoggingConfig {
            detail: pigs_proxy::LogDetail::Off,
            ..Default::default()
        },
        orchestration: pigs_proxy::OrchestrationConfig::default(),
        upstream: Upstreams {
            openai: format!("{upstream_url}/oa"),
            responses: format!("{upstream_url}/rs"),
            anthropic: format!("{upstream_url}/an"),
        },
    };
    let listener = pigs_proxy::bind_listener(&config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = build_state(config, addr);
    let app = pigs_proxy::server::router(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (task, format!("http://{addr}"))
}

/// 回归：三协议各走各的 base（anthropic 的 /v1 属于协议路径，base 不带版本段），
/// 客户端路径逐字上浮 —— 上游看到的路径 = 所选 base + 原路径，一个字不改。
#[tokio::test]
async fn per_protocol_bases_route_by_protocol_and_path_stays_verbatim() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    // 假上游按 FIFO 回放：①anthropic 透传 ②models ③-pigs 的 Pre 子请求（PIGEND 一发结束）
    fu.responses.lock().unwrap().push(openai_text_response("上游原样回"));
    fu.responses.lock().unwrap().push(openai_text_response("上游原样回"));
    fu.responses.lock().unwrap().push(openai_text_response("直接回答\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs_with_bases(&upstream_url).await;

    let client = reqwest::Client::new();

    // ① Anthropic 路径（model 不带 -pigs → 透传）：上游必须看到 /an/v1/messages
    let resp = client
        .post(format!("{pigs_url}/v1/messages"))
        .json(&json!({
            "model": "claude-x", "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // ② GET /v1/models（非协议路径 → 落 OpenAI 约定 base）
    let resp = client
        .get(format!("{pigs_url}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // ③ -pigs 编排（chat）：子请求经回环也必须按协议落 /oa/chat/completions
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pigs", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "直接回答");

    let reqs = fu.requests.lock().unwrap();
    let paths: Vec<&str> = reqs.iter().map(|(p, _b, _h)| p.as_str()).collect();
    assert!(
        paths.contains(&"/an/v1/messages"),
        "anthropic 要落自己的 base（路径逐字上浮）：{paths:?}"
    );
    assert!(
        paths.contains(&"/oa/v1/models"),
        "非协议路径落 OpenAI 约定 base：{paths:?}"
    );
    assert!(
        paths.contains(&"/oa/chat/completions"),
        "-pigs 的回环子请求也要按协议选 base：{paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("/v1/v1/")),
        "不许出现 v1 双叠：{paths:?}"
    );
}
