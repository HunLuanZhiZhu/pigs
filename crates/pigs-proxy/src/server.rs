//! axum 服务：统一接收各方法/路径并按需分流。 / Axum service: accept all methods/paths and dispatch as needed.
//!
//! PIGS 请求支持新编排与工具续接两种情况。 / PIGS requests either start a turn or resume a tool-paused turn.
//! - 首次请求开启新编排。 / The initial client request starts a new turn.
//! - 包含待回填工具结果的请求恢复暂停相位。 / A request with pending tool results resumes the paused phase.

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
    /// 启动时生成的回环内部令牌。 / Random loopback token generated at startup.
    pub loopback_token: Arc<String>,
    /// 本机回环服务地址。 / Local loopback endpoint.
    pub self_url: Arc<String>,
    /// 跨请求保存的工具暂停现场。 / Persisted state for paused tool calls across client requests.
    pub store: Arc<Mutex<orch::state::ContinuationStore>>,
}

pub fn router(state: AppState) -> Router {
    Router::new().fallback(handle).with_state(state)
}

/// 单入口处理所有路径和 HTTP 方法。 / Single handler for all paths and HTTP methods.
/// - 内部回环请求直接透传，防止递归。 / Internal loopback requests pass through to avoid recursion.
/// - 已识别协议的 POST + PIGS 后缀进入编排。 / Recognized POST requests with PIGS suffixes enter orchestration.
/// - 其它请求均透传。 / All other requests pass through.
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

    // 仅外部 POST 协议请求按模型后缀进入编排。 / Only external POST protocol requests use suffix routing.
    if !internal && method == Method::POST {
        if let Some(protocol) = pigs_protocol::protocol_from_path(&path) {
            match serde_json::from_slice::<Value>(&body) {
                Ok(mut parsed) => {
                    let model = parsed
                        .get("model")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some((real_model, mode)) = pigs_protocol::parse_pigs_model(&model) {
                        // 上游用真实模型名，客户端回显原名。 / Use the stripped model upstream and echo the client model downstream.
                        pigs_protocol::set_model(&mut parsed, &real_model);
                        return orchestrate(
                            &state,
                            &headers,
                            parsed,
                            protocol,
                            mode,
                            &model,
                            &real_model,
                            &path,
                            query,
                            exchange.clone(),
                        )
                        .await;
                    }
                    // 未识别 PIGS 后缀则透传。 / Missing PIGS suffix falls through to passthrough.
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

    // 回环及其它普通请求均走透传。 / Loopback and ordinary requests use passthrough.
    passthrough(&state, method, &path, query, &headers, body, exchange).await
}

/// 识别新请求或工具恢复，并交给状态机。 / Distinguish new turns from tool resumes and delegate to the state machine.
#[allow(clippy::too_many_arguments)]
async fn orchestrate(
    state: &AppState,
    headers: &HeaderMap,
    parsed: Value,
    protocol: pigs_protocol::Protocol,
    mode: pigs_protocol::PigsMode,
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
        mode,
        force_full: client_model.ends_with("-pig3") || client_model.ends_with("-pigs3"),
        body: parsed.clone(),
        path: path.to_string(),
        query: query.map(String::from),
        base_headers,
        client_session: orch::find_client_session(&state.upstream.forward_headers(headers)),
    };

    // 续接按全部工具结果匹配，尾部连续结果仅供诊断。 / Match continuations using all tool results; trailing results are diagnostic only.
    // 可据此观察工具结果后追加的提醒或用户消息。 / This reveals reminders or user messages appended after tool results.
    let all_result_ids = pigs_protocol::all_tool_result_ids(protocol, &parsed);
    let trailing_result_ids = pigs_protocol::trailing_tool_result_ids(protocol, &parsed);
    let client_session = input
        .client_session
        .clone()
        .unwrap_or_else(|| "<none>".into());

    let mut decision_fields = vec![
        format!("model: {real_model}"),
        format!("mode: {mode:?}"),
        format!("force_full: {}", input.force_full),
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
                    "pending: id={} session={} mode={:?} phase={} tool_ids={:?} age_ms={} in_flight={}",
                    summary.id,
                    summary.session,
                    summary.mode,
                    summary.phase.as_str(),
                    summary.pending,
                    summary.age_ms,
                    summary.in_flight
                ));
            }

            match store.claim_match_for_mode(&all_result_ids, mode) {
                orch::state::ClaimMatch::Claimed(continuation) => {
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
                }
                orch::state::ClaimMatch::InFlight(id) => {
                    decision_fields.push(format!("decision: conflict_continuation_in_flight {id}"));
                    exchange.write_event("orchestration-decision", &decision_fields);
                    tracing::warn!(
                        model = %real_model,
                        continuation = %id,
                        results = ?trailing_result_ids,
                        "匹配的编排现场正在被另一个请求恢复"
                    );
                    return logged_error_response(
                        &exchange,
                        StatusCode::CONFLICT,
                        "与这批工具结果匹配的编排现场正在恢复中，请稍后重试",
                    );
                }
                orch::state::ClaimMatch::NoMatch if trailing_result_ids.is_empty() => {
                    decision_fields.push("decision: start_new_no_matching_tool_results".into());
                    None
                }
                orch::state::ClaimMatch::NoMatch => {
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
    let continuation_claim_id = continuation.as_ref().map(|item| item.id.clone());

    tracing::info!(
        model = %real_model,
        protocol = ?protocol,
        mode = ?mode,
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
    let usage_mode = match state.config.orchestration.usage_mode {
        crate::config::UsageMode::Max => orch::UsageMode::Max,
        crate::config::UsageMode::Sum => orch::UsageMode::Sum,
    };
    let orchestrator = orch::Orchestrator::with_limits_and_usage(
        state.config.orchestration.max_exec_runs,
        state.config.orchestration.max_post_protocol_retries,
        usage_mode,
    );

    if client_wants_stream {
        return orchestrate_streaming(
            transport,
            store,
            orchestrator,
            input,
            continuation,
            continuation_claim_id,
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
    let outcome = match continuation {
        Some(continuation) => orchestrator.resume(input, rt, continuation).await,
        None => orchestrator.run(input, rt).await,
    };

    match outcome {
        Ok(outcome) => {
            if let Some(id) = continuation_claim_id.as_deref() {
                if let Ok(mut store) = state.store.lock() {
                    if !store.commit_claim(id) {
                        tracing::warn!(continuation = %id, "resume 成功，但 continuation claim 提交失败");
                    }
                } else {
                    tracing::warn!(continuation = %id, "resume 成功，但 continuation 存储不可用，无法提交 claim");
                }
            }
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
            if let Some(id) = continuation_claim_id.as_deref() {
                if let Ok(mut store) = state.store.lock() {
                    if !store.rollback_claim(id) {
                        tracing::warn!(continuation = %id, "resume 失败，但 continuation claim 回滚失败");
                    }
                } else {
                    tracing::warn!(continuation = %id, "resume 失败，且 continuation 存储不可用，无法回滚 claim");
                }
            }
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

/// 流式编排在异步任务中运行，并通过通道发送帧。 / Run streaming orchestration asynchronously and send frames through a channel.
///
/// 文本实时转发；工具调用导致暂停并交付原生调用。 / Forward text live; tool calls pause the phase and are delivered natively.
#[allow(clippy::too_many_arguments)]
fn orchestrate_streaming(
    transport: Arc<dyn orch::transport::Transport>,
    store: Arc<Mutex<orch::state::ContinuationStore>>,
    orchestrator: orch::Orchestrator,
    input: orch::TurnInput,
    continuation: Option<orch::state::Continuation>,
    continuation_claim_id: Option<String>,
    protocol: pigs_protocol::Protocol,
    client_model: String,
    exchange: ExchangeLog,
) -> Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    let encoder = Arc::new(Mutex::new(pigs_protocol::StreamEncoder::new(
        protocol,
        client_model.to_string(),
    )));
    let output_mode = input.mode;
    // 先发送协议起始帧。 / Emit a valid protocol start frame immediately.
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
                // 思考增量实时发送，编码器处理 Anthropic 块生命周期。 / Stream reasoning deltas; the encoder manages Anthropic block lifecycles.
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
        let claim_store = Arc::clone(&store);
        let rt = orch::Runtime {
            transport,
            store,
            progress: Some(progress),
        };
        let outcome = match continuation {
            Some(continuation) => orchestrator.resume(input, rt, continuation).await,
            None => orchestrator.run(input, rt).await,
        };
        if let Some(id) = continuation_claim_id.as_deref() {
            if let Ok(mut claim_store) = claim_store.lock() {
                let finalized = if outcome.is_ok() {
                    claim_store.commit_claim(id)
                } else {
                    claim_store.rollback_claim(id)
                };
                if !finalized {
                    tracing::warn!(continuation = %id, success = outcome.is_ok(), "continuation claim 收尾失败");
                }
            } else {
                tracing::warn!(continuation = %id, success = outcome.is_ok(), "continuation 存储不可用，claim 无法收尾");
            }
        }
        let frames = match encoder.lock() {
            Ok(mut encoder) => match &outcome {
                Ok(outcome) => {
                    log_outcome(outcome, &outcome_exchange);
                    let content = final_content(&client_model, outcome);
                    encoder.set_finish(
                        content.stop_reason.map(String::from),
                        content.usage.cloned(),
                    );
                    // 避免重复发送已流出的文本与思考。 / Avoid replaying already-streamed text or reasoning.
                    // 这里只补发工具调用及其它完整原生块。 / Emit only native blocks unavailable as live deltas.
                    let tail: Vec<pigs_protocol::Part> = content
                        .parts
                        .iter()
                        .filter(|part| !already_streamed(part, output_mode))
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
        // 发送器释放后通道关闭，客户端流结束。 / Dropping the sender closes the client stream.
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

/// 判断内容是否已实时发送，避免收尾重复。 / Check whether a part was already streamed to prevent duplication.
///
/// - 文本：模式 A 实时发，模式 B 在验收后补发。 / Text: Mode A streams live; Mode B emits accepted text later.
/// - Chat 思考字段在增量阶段发送。 / Chat reasoning is sent in deltas.
/// - 原生思考块按增量发送，签名单发。 / Native thinking streams by delta; signatures use separate events.
/// - 其它完整块及工具调用在收尾发送。 / Other native blocks and tool calls are emitted at finalization.
fn already_streamed(part: &pigs_protocol::Part, mode: pigs_protocol::PigsMode) -> bool {
    match part {
        pigs_protocol::Part::Text(_) => mode == pigs_protocol::PigsMode::A,
        pigs_protocol::Part::Reasoning(_) => true,
        pigs_protocol::Part::Native(block) => {
            block.get("type").and_then(|t| t.as_str()) == Some("thinking")
        }
        pigs_protocol::Part::ToolCall(_) => false,
    }
}

/// 组装客户端响应，回显模型名并保留 usage/停止原因。 / Build client output with the original model and native usage/stop reason.
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
            // 原样保留上游停止原因。 / Preserve the upstream stop reason.
            stop_reason: paused.stop_reason.as_deref(),
            usage: paused.usage.as_ref(),
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

/// 透传请求并流式返回响应，保留编码头。 / Proxy the request and stream the response with content-encoding intact.
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
                // 仅过滤逐跳头，保留内容编码。 / Strip only hop-by-hop headers; keep content-encoding.
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

/// 子请求经带令牌的本机回环透传。 / Subrequests use token-protected loopback passthrough.
struct LoopbackTransport {
    /// 回环客户端保留请求头并自动解压响应。 / Loopback client preserves request headers and decompresses responses.
    /// 保留父请求传输语义并在本机处理响应压缩。 / Preserve parent request semantics while handling response compression locally.
    client: reqwest::Client,
    self_url: String,
    token: String,
}

impl LoopbackTransport {
    /// 构造包含原路径、查询串、令牌和端到端头的回环请求。 / Build a loopback request with path, query, token, and end-to-end headers.
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

    /// 流式解析上游 SSE 并即时回调 `sink`。 / Incrementally parse upstream SSE and invoke `sink`.
    /// 同时保留完整响应以检查控制标记及工具调用。 / Also retain the full response for marker/tool-call detection.
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

        // 若上游返回 JSON 而非流，直接一次性回调。 / If upstream returns JSON despite streaming, emit one callback.
        if !pigs_protocol::is_sse_content_type(content_type.as_deref()) {
            let body = resp.bytes().await.map_err(|e| {
                orch::transport::TransportError::Send(format!("回环响应读取失败: {e}"))
            })?;
            if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                let output = pigs_protocol::parse_json_output(protocol, &value);
                // JSON 整体响应按内容顺序回传文本和思考。 / Deliver text and reasoning from whole JSON in content order.
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
