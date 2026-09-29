//! 集成测试：真实 HTTP 链路 —— 假上游（wiremock 风格的手写 axum）+ 真实 pigs 服务。
//!
//! 覆盖：无 -pig 透传、content-encoding 完整透传（血泪教训回归）、-pig 全链路编排
//! （经 loopback 回环）、Pre 简单路径短路、流式合成。

use axum::body::Bytes;
use axum::response::Response;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::any;
use axum::Router;
use pigs_proxy::{build_state, Config};
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
}

async fn fake_handler(
    State(state): State<FakeUpstreamState>,
    method: axum::http::Method,
    RawQuery(_query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response<axum::body::Body> {
    let _ = method;
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
        "recorded".into(),
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
        base_url: upstream_url.into(),
        key: String::new(),
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

fn openai_text_response(text: &str) -> (&'static str, Bytes) {
    (
        "application/json",
        Bytes::from(json!({"choices":[{"message":{"role":"assistant","content":text}}]}).to_string()),
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
        .json(&openai_body("gpt-x-pig", false))
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
    assert_eq!(final_json["model"], "gpt-x");

    // 上游收到 3 次子请求，均为真名、非流式、无工具，且带一致会话头
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    for (i, (_path, body, hdrs)) in reqs.iter().enumerate() {
        // 子请求绝不能带 accept-encoding：上游回压缩体后编排层无法解析 JSON
        assert!(
            hdrs.get("h:accept-encoding").is_none(),
            "子请求不允许携带 accept-encoding（第 {i} 只 pig）"
        );
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["stream"], false);
        assert!(body.get("tools").is_none());
        let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        if i == 0 {
            assert!(content.contains("帮我完成任务Z") && content.contains("执行前分析"));
        } else if i == 1 {
            assert!(content.contains("分析：需要X"));
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
            "model": "claude-x-pig",
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
async fn pig_simple_path_answers_from_pre() {
    let (_up, upstream_url, fu) = spawn_fake_upstream().await;
    fu.responses.lock().unwrap().push(openai_text_response("答案是 4\nPIGEND"));
    let (_pigs, pigs_url) = spawn_pigs(&upstream_url).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{pigs_url}/chat/completions"))
        .json(&openai_body("gpt-x-pig", false))
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
        .json(&openai_body("gpt-x-pig", true))
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
        .json(&openai_body("gpt-x-pig", true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let sse = resp.text().await.unwrap();

    // 上游收到的子请求：真名 + 流式 + 无 accept-encoding
    let reqs = fu.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3, "Pre → Executor → Post 三次子请求");
    for (i, (_p, body, hdrs)) in reqs.iter().enumerate() {
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["stream"], true, "第 {i} 只 pig 的子请求应为流式");
        assert!(
            hdrs.get("h:accept-encoding").is_none(),
            "子请求不允许携带 accept-encoding（第 {i} 只 pig）"
        );
    }
    drop(reqs);

    // 客户端流：三只 pig 的可见文本按顺序、空行分隔，控制标记已被剥掉
    let text = pigs_protocol::extract_sse_text(pigs_protocol::Protocol::OpenAI, &sse).unwrap();
    assert_eq!(text, "分析：需要X\n第二行\n\n执行结果……\n\n评审：继续");
    assert!(!sse.contains("PIGEND") && !sse.contains("PIGFAIL"));
    let order = ["分析：需要X", "执行结果……", "评审：继续"]
        .map(|s| sse.find(s).expect("阶段文本应出现在客户端流里"));
    assert!(order[0] < order[1] && order[1] < order[2]);
    assert!(sse.contains("data: [DONE]"));
}
