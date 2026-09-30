//! axum 服务：fallback 接住所有路径与方法，按需分流。
//!
//! -pigs 请求的两种情形：
//! - 客户端的第一发 → 新开一轮编排；
//! - 请求中包含当前 pending continuation 所等待的工具结果 → 接着被暂停的相位继续。

use crate::config::Config;
use crate::diagnostics::{header_pairs, ExchangeLog, HttpDiagnostics};
use crate::upstream::Upstream;
use axum::body::Body;
use axum::extract::{OriginalUri, RawQuery, State};
use axum::http::{HeaderMap, Method, Response, StatusCode};
use axum::Router;
use bytes::Bytes;
use futures_util::StreamExt;
use pigs_orchestrator as orch;
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub diagnostics: Arc<HttpDiagnostics>,
    pub config: Arc<Config>,
    pub upstream: Arc<Upstream>,
    /// 回环内部令牌（进程启动时随机生成）。
    pub loopback_token: Arc<String>,
    /// 回环目标（http://127.0.0.1:port）。
    pub self_url: Arc<String>,
    /// 工具调用暂停的现场（跨请求保留：客户端执行完工具会带结果回来）。
    pub store: Arc<Mutex<orch::state::ContinuationStore>>,
}

pub fn router(state: AppState) -> Router {
    Router::new().fallback(handle).with_state(state)
}

/// 单一入口：所有路径、所有方法。
/// - 回环子请求（带内部令牌）→ 直接透传（防递归）；
/// - POST 三协议路径 + model 带 `-pigs` → 编排（新开或恢复）；
/// - 其余一切 → 原样透传。
async fn handle(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let path = uri.path().to_string();
    let query = query.as_deref();

    let internal = headers
        .get(orch::LOOPBACK_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|t| t == state.loopback_token.as_str())
        .unwrap_or(false);

    let exchange =
        state
            .diagnostics
            .begin_exchange(internal, method.as_str(), &path, query, &headers, &body);

    // 编排分支：POST 协议路径 + model 带 -pigs（回环子请求不走此分支）
    if !internal && method == Method::POST {
        if let Some(protocol) = pigs_protocol::protocol_from_path(&path) {
            match serde_json::from_slice::<Value>(&body) {
                Ok(mut parsed) => {
                    let model = parsed
                        .get("model")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(real_model) = pigs_protocol::strip_pigs_suffix(&model) {
                        // 唯一允许的字段改动：发给上游用真名，回客户端用原名
                        pigs_protocol::set_model(&mut parsed, &real_model);
                        return orchestrate(
                            &state,
                            &headers,
                            parsed,
                            protocol,
                            &model,
                            &real_model,
                            &path,
                            query,
                            exchange.clone(),
                        )
                        .await;
                    }
                    // 无 -pigs → 落到下面的透传
                }
                Err(e) => {
                    tracing::warn!(error = %e, "请求 body 不是合法 JSON");
                    return logged_error_response(
                        &exchange,
                        StatusCode::BAD_REQUEST,
                        "请求 body 不是合法 JSON",
                    );
                }
            }
        }
    }

    // 透传（含回环子请求、非 -pigs 主请求、models 等其他路径）
    passthrough(&state, method, &path, query, &headers, body, exchange).await
}

/// 编排：判断这一发是新开一轮还是恢复被工具调用暂停的相位，然后交给状态机。
#[allow(clippy::too_many_arguments)]
async fn orchestrate(
    state: &AppState,
    headers: &HeaderMap,
    parsed: Value,
    protocol: pigs_protocol::Protocol,
    client_model: &str,
    real_model: &str,
    path: &str,
    query: Option<&str>,
    exchange: ExchangeLog,
) -> Response<Body> {
    let client_wants_stream = pigs_protocol::has_client_stream(&parsed);
    let base_headers = state.upstream.forward_headers(headers);
    let tool_count = parsed
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let input = orch::TurnInput {
        protocol,
        body: parsed.clone(),
        path: path.to_string(),
        query: query.map(String::from),
        base_headers,
        client_session: orch::find_client_session(&state.upstream.forward_headers(headers)),
    };

    // continuation 按请求中出现的全部工具结果匹配；尾部连续结果只保留作诊断，
    // 用来解释客户端是否在工具结果之后又追加了 reminder / 普通消息。
    let all_result_ids = pigs_protocol::all_tool_result_ids(protocol, &parsed);
    let trailing_result_ids = pigs_protocol::trailing_tool_result_ids(protocol, &parsed);
    let client_session = input
        .client_session
        .clone()
        .unwrap_or_else(|| "<none>".into());

    let mut decision_fields = vec![
        format!("model: {real_model}"),
        format!("protocol: {protocol:?}"),
        format!("client_session: {client_session}"),
        format!("all_tool_result_ids: {all_result_ids:?}"),
        format!("trailing_tool_result_ids: {trailing_result_ids:?}"),
    ];

    let continuation = match state.store.lock() {
        Ok(mut store) => {
            let summaries = store.summaries();
            decision_fields.push(format!("pending_continuations: {}", summaries.len()));
            for summary in &summaries {
                decision_fields.push(format!(
                    "pending: id={} session={} phase={} tool_ids={:?} age_ms={}",
                    summary.id,
                    summary.session,
                    summary.phase.as_str(),
                    summary.pending,
                    summary.age_ms
                ));
            }

            if let Some(continuation) = store.take_match(&all_result_ids) {
                let matched = continuation.pending.clone();
                decision_fields.push(format!("matched_tool_result_ids: {matched:?}"));
                decision_fields.push(format!("decision: resume {}", continuation.id));
                tracing::info!(
                    model = %real_model,
                    continuation = %continuation.id,
                    results = matched.len(),
                    "继续被工具调用暂停的相位"
                );
                Some(continuation)
            } else if trailing_result_ids.is_empty() {
                decision_fields.push("decision: start_new_no_matching_tool_results".into());
                None
            } else {
                decision_fields.push("decision: conflict_no_matching_continuation".into());
                exchange.write_event("orchestration-decision", &decision_fields);
                tracing::warn!(
                    model = %real_model,
                    results = ?trailing_result_ids,
                    "带工具结果，但没有匹配的编排现场"
                );
                return logged_error_response(
                    &exchange,
                    StatusCode::CONFLICT,
                    "找不到与这批工具结果对应的编排现场（可能已过期或服务重启过），请重新发起该轮请求",
                );
            }
        }
        Err(_) => {
            decision_fields.push("decision: conflict_continuation_store_unavailable".into());
            exchange.write_event("orchestration-decision", &decision_fields);
            return logged_error_response(
                &exchange,
                StatusCode::INTERNAL_SERVER_ERROR,
                "continuation 存储不可用",
            );
        }
    };
    exchange.write_event("orchestration-decision", &decision_fields);

    tracing::info!(
        model = %real_model,
        protocol = ?protocol,
        streaming = client_wants_stream,
        tools = tool_count,
        resume = continuation.is_some(),
        "进入编排"
    );

    let store = Arc::clone(&state.store);
    let transport: Arc<dyn orch::transport::Transport> = Arc::new(LoopbackTransport {
        client: reqwest::Client::new(),
        self_url: state.self_url.as_str().to_string(),
        token: state.loopback_token.as_str().to_string(),
    });

    if client_wants_stream {
        return orchestrate_streaming(
            transport,
            store,
            input,
            continuation,
            protocol,
            client_model.to_string(),
            exchange,
        );
    }

    let rt = orch::Runtime {
        transport,
        store,
        progress: None,
    };
    let orchestrator = orch::Orchestrator::new();
    let outcome = match continuation {
        Some(continuation) => orchestrator.resume(input, rt, continuation).await,
        None => orchestrator.run(input, rt).await,
    };

    match outcome {
        Ok(outcome) => {
            let content = final_content(client_model, &outcome);
            log_outcome(&outcome, &exchange);
            let body = pigs_protocol::synthesize_json(protocol, &content).to_string();
            let response_headers = vec![("content-type".into(), "application/json".into())];
            exchange.write_response(
                exchange.client_response_stage(),
                StatusCode::OK.as_u16(),
                &response_headers,
                body.as_bytes(),
            );
            let mut resp = Response::new(Body::from(body));
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::warn!(error = %e, "编排失败");
            let status = if matches!(e, orch::Error::Budget(_)) {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::BAD_GATEWAY
            };
            logged_error_response(&exchange, status, &format!("编排失败: {e}"))
        }
    }
}

/// 流式编排：起后台任务跑状态机，帧通过 channel 推给客户端。
///
/// 文本边到边转；模型要工具时把原生调用作为终止帧发出（相位到此暂停，等客户端执行）。
#[allow(clippy::too_many_arguments)]
fn orchestrate_streaming(
    transport: Arc<dyn orch::transport::Transport>,
    store: Arc<Mutex<orch::state::ContinuationStore>>,
    input: orch::TurnInput,
    continuation: Option<orch::state::Continuation>,
    protocol: pigs_protocol::Protocol,
    client_model: String,
    exchange: ExchangeLog,
) -> Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    let encoder = Arc::new(Mutex::new(pigs_protocol::StreamEncoder::new(
        protocol,
        client_model.to_string(),
    )));
    // 起始帧：让客户端立刻拿到合法的流开头
    if let Ok(mut encoder) = encoder.lock() {
        let _ = tx.send(Ok(Bytes::from(encoder.start())));
    }

    let progress: orch::ProgressSink = {
        let encoder = Arc::clone(&encoder);
        let tx = tx.clone();
        Arc::new(move |event: orch::PigEvent| {
            let Ok(mut encoder) = encoder.lock() else {
                return;
            };
            let frames = match &event {
                orch::PigEvent::Delta(text) => encoder.push_text(text),
                // 思考也边想边流（Anthropic 需要块生命周期，编码器内部管）
                orch::PigEvent::Thought(text) => encoder.push_reasoning(text),
                orch::PigEvent::ThoughtSummary { item_id, text } => {
                    encoder.push_reasoning_summary(item_id, text)
                }
                orch::PigEvent::ThoughtSignature(signature) => {
                    encoder.push_reasoning_signature(signature)
                }
                orch::PigEvent::End(_) => encoder.end_pig(),
                orch::PigEvent::Start(_) => String::new(),
            };
            if !frames.is_empty() {
                let _ = tx.send(Ok(Bytes::from(frames)));
            }
        })
    };

    let outcome_exchange = exchange.clone();
    tokio::spawn(async move {
        let rt = orch::Runtime {
            transport,
            store,
            progress: Some(progress),
        };
        let orchestrator = orch::Orchestrator::new();
        let outcome = match continuation {
            Some(continuation) => orchestrator.resume(input, rt, continuation).await,
            None => orchestrator.run(input, rt).await,
        };
        let frames = match encoder.lock() {
            Ok(mut encoder) => match &outcome {
                Ok(outcome) => {
                    log_outcome(outcome, &outcome_exchange);
                    let content = final_content(&client_model, outcome);
                    encoder.set_finish(
                        content.stop_reason.map(String::from),
                        content.usage.cloned(),
                    );
                    // 实时阶段已经流过的内容不要再发一遍（文本、思考增量），
                    // 这里只补"没法边到边流"的原生内容：工具调用、redacted_thinking 等完整块
                    let tail: Vec<pigs_protocol::Part> = content
                        .parts
                        .iter()
                        .filter(|part| !already_streamed(part))
                        .cloned()
                        .collect();
                    let mut frames = encoder.push_parts(&tail);
                    frames.push_str(&encoder.finish());
                    frames
                }
                Err(e) => {
                    tracing::warn!(error = %e, "编排失败（流式，已发部分内容）");
                    encoder.error(&format!("编排失败: {e}"))
                }
            },
            Err(_) => String::new(),
        };
        if !frames.is_empty() {
            let _ = tx.send(Ok(Bytes::from(frames)));
        }
        // tx 在此 drop：channel 关闭，客户端流正常收尾
    });

    let response_headers = vec![
        ("content-type".into(), "text/event-stream".into()),
        ("cache-control".into(), "no-cache".into()),
    ];
    let capture = exchange.capture_response(
        exchange.client_response_stage(),
        StatusCode::OK.as_u16(),
        &response_headers,
    );
    let stream = futures_util::stream::unfold((rx, capture), |(mut rx, mut capture)| async move {
        match rx.recv().await {
            Some(item) => {
                if let (Some(capture), Ok(bytes)) = (capture.as_mut(), &item) {
                    capture.push(bytes);
                }
                Some((item, (rx, capture)))
            }
            None => {
                if let Some(capture) = capture {
                    capture.finish();
                }
                None
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// 这条内容在实时阶段是否已经流给客户端了（避免收尾重复发）。
///
/// - 文本：增量阶段逐段流过；
/// - `reasoning_content`（Chat 思考字段）：增量阶段流过；
/// - `thinking` 原生块：增量阶段按 thinking_delta 流过（签名单发）；
/// - `redacted_thinking` 等完整块、以及工具调用：没法边到边流，收尾时给。
fn already_streamed(part: &pigs_protocol::Part) -> bool {
    match part {
        pigs_protocol::Part::Text(_) | pigs_protocol::Part::Reasoning(_) => true,
        pigs_protocol::Part::Native(block) => {
            block.get("type").and_then(|t| t.as_str()) == Some("thinking")
        }
        pigs_protocol::Part::ToolCall(_) => false,
    }
}

/// 把编排产出变成回客户端的内容（model 名用客户端请求的那个，usage/stop_reason 原样）。
fn final_content<'a>(
    client_model: &'a str,
    outcome: &'a orch::Outcome,
) -> pigs_protocol::ResponseContent<'a> {
    match outcome {
        orch::Outcome::Completed(turn) => pigs_protocol::ResponseContent {
            model: client_model,
            parts: &turn.parts,
            stop_reason: turn.stop_reason.as_deref(),
            usage: turn.usage.as_ref(),
        },
        orch::Outcome::Paused(paused) => pigs_protocol::ResponseContent {
            model: client_model,
            parts: &paused.parts,
            // 上游这一轮怎么停的就怎么说（工具暂停通常是 tool_calls / tool_use）
            stop_reason: paused.stop_reason.as_deref(),
            usage: None,
        },
    }
}

fn log_outcome(outcome: &orch::Outcome, exchange: &ExchangeLog) {
    match outcome {
        orch::Outcome::Completed(turn) => {
            exchange.write_event(
                "orchestration-outcome",
                &[
                    "outcome: completed".into(),
                    format!("ended_with: {}", turn.ended_with.as_str()),
                    format!("session: {}", turn.session),
                    format!("path: {:?}", turn.path),
                ],
            );
            tracing::info!(
                ended_with = turn.ended_with.as_str(),
                pigs = turn.path.len(),
                session = %turn.session,
                chars = turn.text.chars().count(),
                "编排完成"
            );
        }
        orch::Outcome::Paused(paused) => {
            exchange.write_event(
                "orchestration-outcome",
                &[
                    "outcome: paused".into(),
                    format!("continuation: {}", paused.continuation_id),
                    format!(
                        "tool_call_ids: {:?}",
                        paused
                            .tool_calls
                            .iter()
                            .map(|call| call.id.as_str())
                            .collect::<Vec<_>>()
                    ),
                    format!("stop_reason: {:?}", paused.stop_reason),
                ],
            );
            tracing::info!(
                continuation = %paused.continuation_id,
                calls = paused.tool_calls.len(),
                "编排暂停：等待客户端执行工具"
            );
        }
    }
}

/// 原样透传：转发请求，响应流式回传（content-encoding 跟 body 一起走）。
async fn passthrough(
    state: &AppState,
    method: Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
    exchange: ExchangeLog,
) -> Response<Body> {
    let fwd_headers = state.upstream.forward_headers(headers);
    let has_body = method == Method::POST || method == Method::PUT || method == Method::PATCH;
    let method_name = method.as_str().to_string();
    let upstream_target = state.upstream.url(path, query);
    exchange.write_upstream_request(
        &method_name,
        &upstream_target,
        None,
        &fwd_headers,
        if has_body { &body } else { &[] },
    );

    match state
        .upstream
        .send(
            method,
            path,
            query,
            fwd_headers,
            has_body.then(|| body.clone()),
        )
        .await
    {
        Ok((status, resp_headers, resp)) => {
            let upstream_headers = header_pairs(&resp_headers);
            let mut builder = Response::builder().status(status);
            let mut client_headers = Vec::new();
            for (name, value) in resp_headers.iter() {
                let lower = name.as_str().to_lowercase();
                // 只跳过逐跳头；content-encoding 必须跟着 body 走（血泪教训）
                if matches!(
                    lower.as_str(),
                    "connection" | "transfer-encoding" | "content-length"
                ) {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    axum::http::HeaderName::from_bytes(name.as_ref()),
                    axum::http::HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    client_headers.push((
                        name.as_str().to_string(),
                        value.to_str().unwrap_or("<non-utf8>").to_string(),
                    ));
                    builder = builder.header(n, v);
                }
            }
            let upstream_capture =
                exchange.capture_response("upstream-response", status.as_u16(), &upstream_headers);
            let client_capture = exchange.capture_response(
                exchange.client_response_stage(),
                status.as_u16(),
                &client_headers,
            );
            let stream = futures_util::stream::unfold(
                (resp.bytes_stream(), upstream_capture, client_capture),
                |(mut stream, mut upstream_capture, mut client_capture)| async move {
                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            if let Some(capture) = upstream_capture.as_mut() {
                                capture.push(&bytes);
                            }
                            if let Some(capture) = client_capture.as_mut() {
                                capture.push(&bytes);
                            }
                            Some((Ok(bytes), (stream, upstream_capture, client_capture)))
                        }
                        Some(Err(error)) => Some((
                            Err(std::io::Error::new(std::io::ErrorKind::Other, error)),
                            (stream, upstream_capture, client_capture),
                        )),
                        None => {
                            if let Some(capture) = upstream_capture {
                                capture.finish();
                            }
                            if let Some(capture) = client_capture {
                                capture.finish();
                            }
                            None
                        }
                    }
                },
            );
            builder.body(Body::from_stream(stream)).unwrap()
        }
        Err(e) => {
            tracing::warn!(error = %e, "上游请求失败");
            exchange.write_response("upstream-error", 0, &[], e.to_string().as_bytes());
            logged_error_response(
                &exchange,
                StatusCode::BAD_GATEWAY,
                &format!("上游请求失败: {e}"),
            )
        }
    }
}

fn logged_error_response(
    exchange: &ExchangeLog,
    status: StatusCode,
    message: &str,
) -> Response<Body> {
    let body = serde_json::json!({"error": {"type": "pigs_error", "message": message}}).to_string();
    let headers = vec![("content-type".into(), "application/json".into())];
    exchange.write_response(
        exchange.client_response_stage(),
        status.as_u16(),
        &headers,
        body.as_bytes(),
    );
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// 编排子请求的回环传输：POST 到本服务的透传入口，带内部令牌防递归。
struct LoopbackTransport {
    /// 自动解压的客户端：请求头照原样发（含客户端的 accept-encoding），
    /// 响应体压缩由本机解开——这样上游看到的请求与父请求逐字节一致。
    client: reqwest::Client,
    self_url: String,
    token: String,
}

impl LoopbackTransport {
    /// 组一个回环请求（路径 + 查询串 + 内部令牌 + 端到端头）。
    fn request(&self, req: &orch::transport::SubRequest) -> reqwest::RequestBuilder {
        let mut url = format!("{}/{}", self.self_url, req.path.trim_start_matches('/'));
        if let Some(query) = req.query.as_deref().filter(|q| !q.is_empty()) {
            url.push('?');
            url.push_str(query);
        }
        let mut builder = self
            .client
            .post(url)
            .header(orch::LOOPBACK_TOKEN_HEADER, &self.token);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        builder
    }

    fn content_type(resp: &reqwest::Response) -> Option<String> {
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    }

    fn read_error(e: reqwest::Error) -> orch::transport::TransportError {
        orch::transport::TransportError::Send(format!("回环请求失败: {e}"))
    }
}

#[async_trait::async_trait]
impl orch::transport::Transport for LoopbackTransport {
    async fn send(
        &self,
        req: orch::transport::SubRequest,
    ) -> Result<orch::transport::SubResponse, orch::transport::TransportError> {
        let resp = self
            .request(&req)
            .body(req.body.clone())
            .send()
            .await
            .map_err(Self::read_error)?;
        let status = resp.status().as_u16();
        let content_type = Self::content_type(&resp);
        let body = resp
            .bytes()
            .await
            .map_err(|e| orch::transport::TransportError::Send(format!("回环响应读取失败: {e}")))?;
        Ok(orch::transport::SubResponse {
            status,
            content_type,
            body,
        })
    }

    /// 流式：把回环响应（上游 SSE）按块解析出文本增量，边收边交给 `sink`，
    /// 同时累积完整 body —— 编排还要在全文上判定 PIGEND/PIGFAIL 与工具调用。
    async fn send_streaming(
        &self,
        req: orch::transport::SubRequest,
        protocol: pigs_protocol::Protocol,
        sink: orch::transport::LiveSink,
    ) -> Result<orch::transport::SubResponse, orch::transport::TransportError> {
        let resp = self
            .request(&req)
            .body(req.body.clone())
            .send()
            .await
            .map_err(Self::read_error)?;
        let status = resp.status().as_u16();
        let content_type = Self::content_type(&resp);

        // 上游无视 stream 回了整体 JSON → 一次性把文本推给 sink
        if !pigs_protocol::is_sse_content_type(content_type.as_deref()) {
            let body = resp.bytes().await.map_err(|e| {
                orch::transport::TransportError::Send(format!("回环响应读取失败: {e}"))
            })?;
            if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                let output = pigs_protocol::parse_json_output(protocol, &value);
                // 上游回了整体 JSON：文本与思考一次性报给编排层（顺序按内容序列）
                for part in &output.parts {
                    match part {
                        pigs_protocol::Part::Text(text) => {
                            sink(pigs_protocol::LiveEvent::Text(text.clone()))
                        }
                        pigs_protocol::Part::Reasoning(text) => {
                            sink(pigs_protocol::LiveEvent::Thinking(text.clone()))
                        }
                        _ => {}
                    }
                }
            }
            return Ok(orch::transport::SubResponse {
                status,
                content_type,
                body,
            });
        }

        let mut accumulated: Vec<u8> = Vec::new();
        let mut extractor = pigs_protocol::SseTextStream::new(protocol);
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                orch::transport::TransportError::Send(format!("回环响应读取失败: {e}"))
            })?;
            accumulated.extend_from_slice(&chunk);
            for event in extractor.push_events(&chunk) {
                sink(event);
            }
        }
        for event in extractor.finish_events() {
            sink(event);
        }
        Ok(orch::transport::SubResponse {
            status,
            content_type,
            body: Bytes::from(accumulated),
        })
    }
}
