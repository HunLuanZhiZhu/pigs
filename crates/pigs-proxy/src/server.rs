//! axum 服务：fallback 接住所有路径与方法，按需分流。

use crate::config::Config;
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
    pub config: Arc<Config>,
    pub upstream: Arc<Upstream>,
    /// 回环内部令牌（进程启动时随机生成）。
    pub loopback_token: Arc<String>,
    /// 回环目标（http://127.0.0.1:port）。
    pub self_url: Arc<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new().fallback(handle).with_state(state)
}

/// 单一入口：所有路径、所有方法。
/// - 回环子请求（带内部令牌）→ 直接透传（防递归）；
/// - POST 三协议路径 + model 带 `-pig` → 编排；
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

    // 编排分支：POST 协议路径 + model 带 -pig（回环子请求不走此分支）
    if !internal && method == Method::POST {
        if let Some(protocol) = pigs_protocol::protocol_from_path(&path) {
            match serde_json::from_slice::<Value>(&body) {
                Ok(mut parsed) => {
                    let model = parsed.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
                    if let Some(real_model) = pigs_protocol::strip_pig_suffix(&model) {
                        return orchestrate(
                            &state, &headers, &mut parsed, protocol, &real_model, &path,
                        )
                        .await;
                    }
                    // 无 -pig → 落到下面的透传
                }
                Err(e) => {
                    tracing::warn!(error = %e, "请求 body 不是合法 JSON");
                    return error_response(StatusCode::BAD_REQUEST, "请求 body 不是合法 JSON");
                }
            }
        }
    }

    // 透传（含回环子请求、非 -pig 主请求、models 等其他路径）
    passthrough(&state, method, &path, query, &headers, body).await
}

/// -pig 编排：剥后缀 → 三 pig 状态机 → 合成协议正确的响应。
///
/// 客户端要流式时**边编排边推帧**：每只 pig 的上游增量实时过滤后立刻转给客户端；
/// 客户端不要流式时，等编排跑完一次性给出 JSON（内容 = 各 pig 可见文本拼接）。
async fn orchestrate(
    state: &AppState,
    headers: &HeaderMap,
    parsed: &mut Value,
    protocol: pigs_protocol::Protocol,
    real_model: &str,
    path: &str,
) -> Response<Body> {
    let client_wants_stream = pigs_protocol::has_client_stream(parsed);
    pigs_protocol::set_model(parsed, real_model);

    let base_headers = state.upstream.forward_headers(headers);
    let client_session = orch::find_client_session(&base_headers);

    let transport = Arc::new(LoopbackTransport {
        client: reqwest::Client::new(),
        self_url: state.self_url.as_str().to_string(),
        token: state.loopback_token.as_str().to_string(),
    });
    let input = orch::TurnInput {
        protocol,
        body: parsed.clone(),
        path: path.to_string(),
        base_headers,
        client_session,
    };

    tracing::info!(model = %real_model, protocol = ?protocol, streaming = client_wants_stream, "进入编排");

    if client_wants_stream {
        return orchestrate_streaming(input, transport, protocol, real_model);
    }

    match orch::Orchestrator::new().run(input, transport).await {
        Ok(turn) => {
            tracing::info!(
                ended_with = turn.ended_with.as_str(),
                pigs = turn.path.len(),
                session = %turn.session,
                chars = turn.text.chars().count(),
                "编排完成"
            );
            let json = pigs_protocol::synthesize_json(protocol, real_model, &turn.text);
            let mut resp = Response::new(Body::from(json.to_string()));
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::warn!(error = %e, "编排失败");
            let status = if matches!(e, orch::Error::Budget(_)) {
                // 预算耗尽 = 本轮没做成，不是上游故障
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::BAD_GATEWAY
            };
            error_response(status, &format!("编排失败: {e}"))
        }
    }
}

/// 流式编排：起一个后台任务跑状态机，把 SSE 帧通过 channel 推给客户端。
///
/// 起始帧立刻发出（客户端连接马上变成流），此后每只 pig 的文本边到边转；
/// 出错则发流内错误帧（此时 HTTP 状态已定，无法再改）。
fn orchestrate_streaming(
    input: orch::TurnInput,
    transport: Arc<dyn orch::transport::Transport>,
    protocol: pigs_protocol::Protocol,
    client_model: &str,
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
                orch::PigEvent::End(_) => encoder.end_pig(),
                orch::PigEvent::Start(pig) => {
                    tracing::debug!(pig = pig.as_str(), "pig started (streaming)");
                    String::new()
                }
            };
            if !frames.is_empty() {
                let _ = tx.send(Ok(Bytes::from(frames)));
            }
        })
    };

    tokio::spawn(async move {
        let outcome = orch::Orchestrator::new()
            .run_with_progress(input, transport, Some(progress))
            .await;
        let frames = match encoder.lock() {
            Ok(mut encoder) => match &outcome {
                Ok(turn) => {
                    tracing::info!(
                        ended_with = turn.ended_with.as_str(),
                        pigs = turn.path.len(),
                        session = %turn.session,
                        chars = turn.text.chars().count(),
                        "编排完成（流式）"
                    );
                    encoder.finish()
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

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// 原样透传：转发请求，响应流式回传（content-encoding 跟 body 一起走）。
async fn passthrough(
    state: &AppState,
    method: Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let fwd_headers = state.upstream.forward_headers(headers);
    let has_body = method == Method::POST || method == Method::PUT || method == Method::PATCH;

    match state
        .upstream
        .send(method, path, query, fwd_headers, has_body.then(|| body.clone()))
        .await
    {
        Ok((status, resp_headers, resp)) => {
            let mut builder = Response::builder().status(status);
            for (name, value) in resp_headers.iter() {
                let lower = name.as_str().to_lowercase();
                // 只跳过逐跳头；content-encoding 必须跟着 body 走（血泪教训）
                if matches!(lower.as_str(), "connection" | "transfer-encoding" | "content-length") {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    axum::http::HeaderName::from_bytes(name.as_ref()),
                    axum::http::HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    builder = builder.header(n, v);
                }
            }
            let stream = resp.bytes_stream().map(|r| {
                r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            });
            builder.body(Body::from_stream(stream)).unwrap()
        }
        Err(e) => {
            tracing::warn!(error = %e, "上游请求失败");
            error_response(StatusCode::BAD_GATEWAY, &format!("上游请求失败: {e}"))
        }
    }
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::json!({"error": {"type": "pigs_error", "message": message}});
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 编排子请求的回环传输：POST 到本服务的透传入口，带内部令牌防递归。
struct LoopbackTransport {
    client: reqwest::Client,
    self_url: String,
    token: String,
}

impl LoopbackTransport {
    /// 组一个回环请求（路径 + 内部令牌 + 端到端头）。
    fn request(&self, req: &orch::transport::SubRequest) -> reqwest::RequestBuilder {
        let url = format!("{}/{}", self.self_url, req.path.trim_start_matches('/'));
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
    /// 同时累积完整 body —— 编排还要在全文上判定 PIGEND/PIGFAIL。
    async fn send_streaming(
        &self,
        req: orch::transport::SubRequest,
        protocol: pigs_protocol::Protocol,
        sink: orch::transport::TextSink,
    ) -> Result<orch::transport::SubResponse, orch::transport::TransportError> {
        let resp = self
            .request(&req)
            .body(req.body.clone())
            .send()
            .await
            .map_err(Self::read_error)?;
        let status = resp.status().as_u16();
        let content_type = Self::content_type(&resp);

        // 上游无视 stream:true 回了整体 JSON → 一次性把文本推给 sink
        if !pigs_protocol::is_sse_content_type(content_type.as_deref()) {
            let body = resp.bytes().await.map_err(|e| {
                orch::transport::TransportError::Send(format!("回环响应读取失败: {e}"))
            })?;
            if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                if let Some(text) = pigs_protocol::extract_response_text(protocol, &value) {
                    if !text.is_empty() {
                        sink(&text);
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
            let delta = extractor.push(&chunk);
            if !delta.is_empty() {
                sink(&delta);
            }
        }
        let tail = extractor.finish();
        if !tail.is_empty() {
            sink(&tail);
        }
        Ok(orch::transport::SubResponse {
            status,
            content_type,
            body: Bytes::from(accumulated),
        })
    }
}
