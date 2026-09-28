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
use std::sync::Arc;

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

    let mut base_headers = state.upstream.forward_headers(headers);
    let client_session = orch::find_client_session(&base_headers);

    let transport = Arc::new(LoopbackTransport {
        client: reqwest::Client::new(),
        self_url: state.self_url.as_str().to_string(),
        token: state.loopback_token.as_str().to_string(),
    });

    tracing::info!(model = %real_model, protocol = ?protocol, "进入编排");
    let result = orch::Orchestrator::new()
        .run(
            orch::TurnInput {
                protocol,
                body: parsed.clone(),
                path: path.to_string(),
                base_headers,
                client_session,
            },
            transport,
        )
        .await;

    match result {
        Ok(turn) => {
            tracing::info!(
                ended_with = turn.ended_with.as_str(),
                pigs = turn.path.len(),
                session = %turn.session,
                "编排完成"
            );
            if client_wants_stream {
                let sse = pigs_protocol::synthesize_sse(protocol, real_model, &turn.text);
                let (mut resp, ct) = (Response::new(Body::from(sse)), "text/event-stream");
                resp.headers_mut().insert("content-type", ct.parse().unwrap());
                resp
            } else {
                let json = pigs_protocol::synthesize_json(protocol, real_model, &turn.text);
                let (mut resp, ct) = (
                    Response::new(Body::from(json.to_string())),
                    "application/json",
                );
                resp.headers_mut().insert("content-type", ct.parse().unwrap());
                resp
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "编排失败");
            error_response(StatusCode::BAD_GATEWAY, &format!("编排失败: {e}"))
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
) -> Response<Body> {
    let fwd_headers = state.upstream.forward_headers(headers);
    let has_body = method == Method::POST || method == Method::PUT || method == Method::PATCH;

    match state
        .upstream
        .send(method, path, query, fwd_headers, has_body.then(|| body.clone()))
        .await
    {
        Ok((status, resp_headers, resp)) => {
            let is_stream = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.to_ascii_lowercase().contains("text/event-stream"))
                .unwrap_or(false);
            let _ = is_stream; // 统一流式转发，SSE 只是其中一种
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

#[async_trait::async_trait]
impl orch::transport::Transport for LoopbackTransport {
    async fn send(
        &self,
        req: orch::transport::SubRequest,
    ) -> Result<orch::transport::SubResponse, orch::transport::TransportError> {
        let url = format!("{}{}", self.self_url, req.path.trim_start_matches('/'));
        let mut request = self.client.post(format!("{}/{}", self.self_url, req.path.trim_start_matches('/')));
        let _ = url;
        request = request.header(orch::LOOPBACK_TOKEN_HEADER, &self.token);
        for (k, v) in &req.headers {
            request = request.header(k, v);
        }
        let resp = request.body(req.body.clone()).send().await.map_err(|e| {
            orch::transport::TransportError::Send(format!("回环请求失败: {e}"))
        })?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = resp.bytes().await.map_err(|e| {
            orch::transport::TransportError::Send(format!("回环响应读取失败: {e}"))
        })?;
        Ok(orch::transport::SubResponse {
            status,
            content_type,
            body,
        })
    }
}
