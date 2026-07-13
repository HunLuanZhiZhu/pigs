// =============================================================================
// server.rs — 多协议 HTTP 路由 + 透传/相位化分流
// server.rs — Multi-protocol HTTP routing + passthrough/phased dispatch
// =============================================================================
//
// 核心路由逻辑 / Core routing logic:
// 1. 从请求路径判定协议（OpenAI / Anthropic / Responses）
//    Determine protocol from request path
// 2. 从请求体提取 model 字段
//    Extract model field from request body
// 3. 按 model 后缀路由：
//    Route by model suffix:
//    - 无 `-pig` → 透传到上游 LLM（原始代理逻辑）
//      No `-pig` → passthrough to upstream LLM (original proxy logic)
//    - 有 `-pig` → 转给 pigs-api 相位运行时
//      Has `-pig` → route to pigs-api phased runtime
// 4. GET /v1/models → 返回 ×2 模型列表
//    GET /v1/models → return ×2 model list

use crate::config::{Config, Endpoint};
use crate::protocol::Protocol;
use crate::retry::{dispatch, DispatchOutcome};
use crate::upstream::UpstreamClient;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Response, StatusCode};
use axum::routing::{any, get, post};
use axum::Router;
use bytes::Bytes;
use serde_json::Value;
use std::sync::Arc;

/// 应用共享状态。
/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// 代理配置（provider/endpoint 等）。
    /// Proxy configuration (providers, endpoints, etc.).
    pub config: Arc<Config>,
    /// 上游 HTTP 客户端（透传用）。
    /// Upstream HTTP client (for passthrough).
    pub client: Arc<UpstreamClient>,
    /// 相位化运行时（`-pig` 模型用）。
    /// Phased runtime (for `-pig` models).
    pub runtime: Arc<pigs_api::phased_runtime::PhasedRuntime>,
}

/// 构建 axum 路由树。
/// Build the axum router.
pub fn build(state: AppState) -> Router {
    Router::new()
        // GET /v1/models — 模型列表（×2）/ model list (×2)
        .route("/v1/models", get(list_models))
        // POST / — 兜底 / catch-all
        .route("/", post(handle))
        // ANY /*path — 所有其他请求 / all other requests
        .route("/*path", any(handle))
        .with_state(state)
}

/// 主处理器：解析请求 → 按模型路由 → 透传或相位化。
/// Main handler: parse request → route by model → passthrough or phased.
async fn handle(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let protocol = match Protocol::from_path(&path) {
        Some(p) => p,
        None => {
            tracing::warn!(path = %path, "无法识别协议路径");
            return error_response(StatusCode::NOT_FOUND, "不支持的请求路径");
        }
    };

    // 解析 body 取 model 字段 / Parse body to extract model field
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "请求 body 不是合法 JSON");
            return error_response(StatusCode::BAD_REQUEST, "请求 body 不是合法 JSON");
        }
    };

    let client_model = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    tracing::info!(
        path = %path,
        protocol = ?protocol,
        model = %client_model,
        "收到请求"
    );

    // ================================================================
    // 路由分流：-pig 后缀 → 相位运行时；无后缀 → 透传
    // Route: -pig suffix → phased runtime; no suffix → passthrough
    // ================================================================
    if client_model.ends_with("-pig") {
        return handle_pig(&state, protocol, &client_model, &body, headers).await;
    }

    handle_passthrough(&state, protocol, &client_model, &headers, parsed, &body).await
}

/// 处理 `-pig` 模型：走 pigs-api 相位运行时。
///
/// Handle `-pig` models: route through pigs-api phased runtime.
async fn handle_pig(
    state: &AppState,
    protocol: Protocol,
    client_model: &str,
    body: &Bytes,
    client_headers: HeaderMap,
) -> Response<Body> {
    // 剥离 -pig 后缀得到真实模型名 / Strip -pig suffix to get real model name
    let real_model = client_model.trim_end_matches("-pig");

    tracing::info!(
        client_model = %client_model,
        real_model = %real_model,
        "→ 相位化运行时"
    );

    // 将请求体解析为 ConvertedTurn / Parse request body into ConvertedTurn
    let format = match protocol {
        Protocol::OpenAI => pigs_api::format::ApiFormat::OpenAIChat,
        Protocol::Anthropic => pigs_api::format::ApiFormat::Anthropic,
        Protocol::Responses => pigs_api::format::ApiFormat::OpenAIResponses,
    };

    let json_body: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid JSON: {e}"),
            );
        }
    };

    let converted = match format.parse_request(&json_body) {
        Ok(c) => c,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid request: {e}"),
            );
        }
    };

    // 判断是否流式 / Check if streaming
    let is_stream = converted.stream;

    // 把客户端 auth 头注入 task-local，供 dispatch_in_process 读取
    // Inject client auth headers into task-local for dispatch_in_process
    if is_stream {
        // 流式响应：基于 ProgressSink 构造 SSE 流
        // Streaming: build SSE stream based on ProgressSink
        return crate::with_client_headers(client_headers.clone(), async {
            stream_pig(state, protocol, client_model, converted, client_headers).await
        })
        .await;
    }

    // 非流式：执行相位对话 / Non-streaming: execute phased turn
    let result = match crate::with_client_headers(client_headers, async {
        pigs_api::phased_api_convert::run_converted_turn_arc(
            &state.runtime,
            &converted,
            None,
        )
        .await
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("turn failed: {e:#}"),
            );
        }
    };

    // 按请求格式构造响应 / Build response in the request's format
    let resp_json = format.build_response(&result, &state.runtime.wrapped_model);
    let resp_body = serde_json::to_vec(&resp_json).unwrap_or_default();

    let mut resp = Response::new(Body::from(resp_body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    resp
}

/// 处理 `-pig` 模型的流式请求：基于 `ProgressSink` 构造 SSE 流。
///
/// Handle streaming for `-pig` models: build SSE stream from `ProgressSink`.
///
/// `PhasedRuntime` 内部每个阶段的文本增量通过 `ProgressSink::TextDelta`
/// 回调发出。这里把每个 `TextDelta` 转成对应协议的 SSE 帧，通过
/// `tokio::sync::mpsc` channel 推送到 SSE body 流。
///
/// Each phase's text deltas are emitted via `ProgressSink::TextDelta`.
/// This converts each `TextDelta` into a protocol-specific SSE frame and
/// pushes it through a `tokio::sync::mpsc` channel into the SSE body stream.
async fn stream_pig(
    state: &AppState,
    protocol: Protocol,
    client_model: &str,
    converted: pigs_api::phased_api_convert::ConvertedTurn,
    client_headers: HeaderMap,
) -> Response<Body> {
    use pigs_api::format::ApiFormat;
    use pigs_api::phased_runtime::{ProgressSink, TurnProgress};
    use tokio::sync::mpsc;

    let format = match protocol {
        Protocol::OpenAI => ApiFormat::OpenAIChat,
        Protocol::Anthropic => ApiFormat::Anthropic,
        Protocol::Responses => ApiFormat::OpenAIResponses,
    };

    let id = uuid::Uuid::new_v4().to_string();
    let created = chrono::Utc::now().timestamp();
    let model = client_model.to_string();

    // 创建无界 channel 用于 SSE 帧 / Create unbounded channel for SSE frames
    let (tx, rx) = mpsc::unbounded_channel::<String>();

    // 发送首帧（role chunk / message_start）
    // Send the first frame (role chunk / message_start)
    let _ = tx.send(format.role_chunk(&id, created, &model));

    // 创建 ProgressSink：把 TextDelta 转成 SSE 帧推入 channel
    // Create ProgressSink: convert TextDelta into SSE frames and push to channel
    // 需要 clone id/model/format 给 sink 闭包，因为它们也被 spawn 的任务使用
    // Clone id/model/format for the sink closure since they're also used by the spawned task
    let tx_clone = tx.clone();
    let id_sink = id.clone();
    let model_sink = model.clone();
    let sink: ProgressSink = std::sync::Arc::new(move |p: TurnProgress| {
        if let TurnProgress::TextDelta { text, .. } = p {
            if !text.is_empty() {
                let chunk = format.content_chunk(&id_sink, created, &model_sink, &text);
                let _ = tx_clone.send(chunk);
            }
        }
        // 其他进度事件在此模式下忽略（可后续扩展为 pigs.phase 等 SSE 事件）
        // Other progress events are ignored in this mode (could be extended
        // to emit pigs.phase / pigs.tool SSE events in the future).
    });

    // 后台任务：执行相位对话，完成后发送结束帧
    // Background task: run the phased turn, then send the stop frame
    let runtime = std::sync::Arc::clone(&state.runtime);
    let converted = std::sync::Arc::new(converted);
    let converted_clone = std::sync::Arc::clone(&converted);
    let id_task = id.clone();
    let model_task = model.clone();
    tokio::spawn(async move {
        // 在 task-local 上下文中执行，传递客户端 auth 头给 dispatch_in_process
        // Execute within task-local context, forwarding client auth headers to dispatch_in_process
        let result = crate::with_client_headers(client_headers, async {
            pigs_api::phased_api_convert::run_converted_turn_arc(
                &runtime,
                &converted_clone,
                Some(sink),
            )
            .await
        })
        .await;

        // 无论成功或失败，都发送结束帧
        // Always send the stop frame, regardless of success or failure
        let _ = tx.send(format.stop_chunk(&id_task, created, &model_task));
        if let Some(sentinel) = format.done_sentinel() {
            let _ = tx.send(sentinel);
        }

        match result {
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "phased turn failed during streaming");
            }
        }
        // tx drop 后 rx 会结束 / rx ends when tx is dropped
        drop(tx);
    });

    // 构建 SSE body 流 / Build the SSE body stream
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(frame) => {
                let chunk = format!("data: {frame}\n\n");
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(chunk)), rx))
            }
            None => None,
        }
    });

    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    resp
}

/// 处理透传请求：原始 mini-proxy 代理逻辑。
///
/// Handle passthrough requests: original mini-proxy proxy logic.
async fn handle_passthrough(
    state: &AppState,
    protocol: Protocol,
    client_model: &str,
    headers: &HeaderMap,
    mut parsed: Value,
    body: &Bytes,
) -> Response<Body> {
    let endpoint = match pick_endpoint(&state.config, protocol, client_model) {
        Some(ep) => ep,
        None => {
            tracing::warn!(?protocol, model = %client_model, "无可用渠道");
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("未找到匹配的渠道：协议 {:?}，模型 {}", protocol, client_model),
            );
        }
    };

    // 模型名映射：客户端模型名 → 上游模型名
    // Model name mapping: client model → upstream model
    let upstream_model = endpoint.map_model(client_model);
    if upstream_model != client_model {
        if let Some(obj) = parsed.as_object_mut() {
            obj.insert("model".into(), Value::String(upstream_model.clone()));
        }
        tracing::info!(client_model = %client_model, upstream_model = %upstream_model, "模型名已映射");
    }

    // 清洗请求体中 content 为空/空白的项
    // Clean empty-content messages
    if state.config.server.clean_empty_content {
        clean_empty_messages(&mut parsed, protocol);
    }

    // 思考强度注入 / Thinking-effort injection
    let effort = endpoint
        .thinking_effort
        .clone()
        .unwrap_or_else(|| protocol.default_effort().to_string());
    inject_thinking(&mut parsed, protocol, &effort);

    let body_bytes = serde_json::to_vec(&parsed).unwrap_or_else(|_| body.to_vec());
    let body_bytes = Bytes::from(body_bytes);

    let req_id = uuid::Uuid::now_v7();
    let span = tracing::info_span!(
        "request",
        request_id = %req_id,
        protocol = ?protocol,
        model = %client_model,
        channel = %endpoint.base_url,
    );
    let _enter = span.enter();

    match dispatch(&state.client, &endpoint, protocol, &body_bytes, headers).await {
        DispatchOutcome::Ok(r) => r,
        DispatchOutcome::Failed { status, body } => {
            let mut resp = Response::new(Body::from(body));
            *resp.status_mut() = status;
            resp
        }
    }
}

/// GET /v1/models — 返回 ×2 模型列表。
///
/// GET /v1/models — return ×2 model list.
///
/// 遍历所有 provider 的 models 列表，
/// 每个模型输出两条：`{model}`（透传）+ `{model}-pig`（相位化）。
/// Iterates all provider model lists,
/// outputting two entries per model: `{model}` (passthrough) + `{model}-pig` (phased).
async fn list_models(State(state): State<AppState>) -> Response<Body> {
    let mut data: Vec<Value> = Vec::new();

    for p in &state.config.provider {
        // 收集三种协议端点的模型（去重）/ Collect models from all endpoints (deduped)
        let mut models: Vec<String> = Vec::new();
        if let Some(ep) = p.openai_endpoint() {
            for m in &ep.models {
                if !models.contains(m) {
                    models.push(m.clone());
                }
            }
        }
        if let Some(ep) = p.anthropic_endpoint() {
            for m in &ep.models {
                if !models.contains(m) {
                    models.push(m.clone());
                }
            }
        }
        if let Some(ep) = p.responses_endpoint() {
            for m in &ep.models {
                if !models.contains(m) {
                    models.push(m.clone());
                }
            }
        }

        // 每个模型 ×2：原始 + -pig / Each model ×2: original + -pig
        for m in &models {
            data.push(serde_json::json!({
                "id": m,
                "object": "model",
                "owned_by": p.name,
                "type": "passthrough"
            }));
            data.push(serde_json::json!({
                "id": format!("{}-pig", m),
                "object": "model",
                "owned_by": p.name,
                "root": m,
                "type": "phased",
                "phases": "pre -> executor -> post"
            }));
        }
    }

    let resp = serde_json::json!({
        "object": "list",
        "data": data
    });
    let body = serde_json::to_vec(&resp).unwrap_or_default();
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = StatusCode::OK;
    r.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    r
}

// ---------------------------------------------------------------------------
// 以下为 mini-proxy 原有辅助函数（透传逻辑用）
// Below are original mini-proxy helper functions (for passthrough logic)
// ---------------------------------------------------------------------------

/// 按协议 + 模型在 providers 中查找第一个匹配的 endpoint。
/// Find the first matching endpoint by protocol + model across providers.
pub fn pick_endpoint(cfg: &Config, protocol: Protocol, model: &str) -> Option<Endpoint> {
    for p in &cfg.provider {
        let ep = match protocol {
            Protocol::OpenAI => p.openai_endpoint(),
            Protocol::Anthropic => p.anthropic_endpoint(),
            Protocol::Responses => p.responses_endpoint(),
        };
        if let Some(ep) = ep {
            if ep.models.iter().any(|m| m == model) {
                return Some(ep);
            }
        }
    }
    None
}

/// 清洗请求体：补全缺失 type:"message" + 移除空 content 项。
/// Clean request body: patch missing type:"message" + remove empty-content items.
pub fn clean_empty_messages(parsed: &mut Value, protocol: Protocol) {
    let field = match protocol {
        Protocol::Responses => "input",
        _ => "messages",
    };

    let Some(arr) = parsed.get_mut(field).and_then(|v| v.as_array_mut()) else {
        return;
    };

    // Response 协议：补全缺失的 type: "message"
    if matches!(protocol, Protocol::Responses) {
        let mut patched = 0;
        for item in arr.iter_mut() {
            if let Some(obj) = item.as_object_mut() {
                if !obj.contains_key("type") {
                    obj.insert("type".into(), Value::String("message".into()));
                    patched += 1;
                }
            }
        }
        if patched > 0 {
            tracing::info!(patched, "已补全 input 项缺失的 type: \"message\"");
        }
    }

    // 移除 content 为空/空白的项
    let before = arr.len();
    arr.retain(|item| {
        let content = item.get("content");
        match content {
            Some(Value::String(s)) => !s.trim().is_empty(),
            Some(Value::Array(a)) => {
                a.iter().any(|c| {
                    if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                        !t.trim().is_empty()
                    } else if let Some(t) = c.get("content").and_then(|t| t.as_str()) {
                        !t.trim().is_empty()
                    } else {
                        true
                    }
                })
            }
            None => true,
            _ => true,
        }
    });
    let removed = before - arr.len();
    if removed > 0 {
        tracing::info!(field, removed, "已清洗空白 content 消息项");
    }
}

/// 思考强度注入：按协议强制覆盖到指定档位。
/// Thinking-effort injection: force-override to specified level per protocol.
pub fn inject_thinking(parsed: &mut Value, protocol: Protocol, effort: &str) {
    let original = read_effort(parsed, protocol);

    if effort == "passthrough" {
        tracing::info!(original_effort = %original_or_unset(&original), "思考强度透传（未修改）");
        return;
    }

    match protocol {
        Protocol::OpenAI => {
            if let Some(obj) = parsed.as_object_mut() {
                obj.insert("reasoning_effort".into(), Value::String(effort.into()));
            }
        }
        Protocol::Responses => {
            if let Some(obj) = parsed.as_object_mut() {
                let reasoning = obj
                    .entry("reasoning")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let Some(r) = reasoning.as_object_mut() {
                    r.insert("effort".into(), Value::String(effort.into()));
                }
            }
        }
        Protocol::Anthropic => {
            if let Some(obj) = parsed.as_object_mut() {
                let cfg = obj
                    .entry("output_config")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let Some(c) = cfg.as_object_mut() {
                    c.insert("effort".into(), Value::String(effort.into()));
                }
                let thinking = obj
                    .entry("thinking")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let Some(t) = thinking.as_object_mut() {
                    t.insert("type".into(), Value::String("adaptive".into()));
                }
                if let Some(mt) = obj.get("max_tokens").and_then(|v| v.as_u64()) {
                    if mt < 1024 {
                        obj.insert("max_tokens".into(), Value::Number(1024.into()));
                    }
                }
            }
        }
    }

    tracing::info!(
        original_effort = %original_or_unset(&original),
        new_effort = %effort,
        "思考强度已注入"
    );
}

/// 读取客户端请求中原有的思考强度档位。
/// Read the client's original thinking-effort level.
fn read_effort(parsed: &Value, protocol: Protocol) -> Option<String> {
    match protocol {
        Protocol::OpenAI => parsed
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        Protocol::Responses => parsed
            .get("reasoning")
            .and_then(|r| r.get("effort"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        Protocol::Anthropic => parsed
            .get("output_config")
            .and_then(|c| c.get("effort"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

/// 原值为空时显示 "未设置"。
/// Display "未设置" when original value is absent.
fn original_or_unset(original: &Option<String>) -> String {
    match original {
        Some(s) if !s.is_empty() => s.clone(),
        _ => "未设置".to_string(),
    }
}

/// 构造错误响应 JSON。
/// Build an error response JSON.
fn error_response(status: StatusCode, msg: &str) -> Response<Body> {
    let body = format!(
        r#"{{"error":{{"message":"{}","type":"proxy_error"}}}}"#,
        msg
    );
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    resp
}
