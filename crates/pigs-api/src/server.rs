// =============================================================================
// server.rs — 多格式 HTTP API 宿主（OpenAI Chat / Anthropic / OpenAI Responses）
// server.rs — Multi-format HTTP API host (OpenAI Chat / Anthropic / OpenAI Responses)
// =============================================================================
//
// 本模块是 pigs 相位化 Agent 的 HTTP API 服务层。它使用 axum 框架提供
// 三个 API 端点，分别支持三种上游 API 格式：
// This module is the HTTP API service layer for the pigs phased agent.
// Using the axum framework, it provides three API endpoints, each supporting
// a different upstream API format:
//
// 端点 / Endpoints:
//   GET  /health                  — 健康检查 / Health check
//   GET  /v1/models               — 列出可用模型 / List available models
//   POST /chat/completions        — OpenAI Chat 格式（流式/非流式）
//                                    OpenAI Chat format (streaming or not)
//   POST /v1/messages             — Anthropic Messages 格式（流式/非流式）
//                                    Anthropic Messages format (streaming or not)
//   POST /responses               — OpenAI Responses 格式（流式/非流式）
//                                    OpenAI Responses format (streaming or not)
//
// 核心原则：输入什么格式，输出什么格式。
// Core principle: what comes in is what goes out.
//
// 相位流通过 pigs 扩展元数据（pigs.phase / pigs.tool / pigs.done SSE 事件）
// 暴露给客户端，而非标准 API 响应的一部分。
// The phase flow is exposed via pigs extension metadata
// (pigs.phase / pigs.tool / pigs.done SSE events), not part of the
// standard API response.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::format::ApiFormat;
use crate::phased_api_convert::{run_converted_turn_arc, ConvertedTurn};
use crate::phased_runtime::{PhasedRuntime, ProgressSink, TurnProgress};

// ---------------------------------------------------------------------------
// 共享状态 / Shared application state
// ---------------------------------------------------------------------------

/// 应用共享状态：持有相位运行时的 Arc。
/// Shared application state: holds an Arc to the phased runtime.
#[derive(Clone)]
pub struct AppState {
    /// 相位化 Agent 运行时实例（Arc 包装，支持并发访问）
    /// Phased agent runtime instance (Arc-wrapped, supports concurrent access)
    pub runtime: Arc<PhasedRuntime>,
}

// ---------------------------------------------------------------------------
// 服务器入口 / Server entry point
// ---------------------------------------------------------------------------

/// 启动 HTTP API 服务器，监听指定 host:port。
/// Start the HTTP API server, listening on the given host:port.
///
/// 注册五个路由并应用宽松 CORS 策略后，绑定 TCP listener 并启动 axum serve。
/// 此函数会阻塞当前 async 上下文直到服务器关闭。
///
/// Configures five routes, applies a permissive CORS policy, then binds the
/// TCP listener and starts axum serve. Blocks until the server shuts down.
pub async fn serve(runtime: PhasedRuntime, host: &str, port: u16) -> anyhow::Result<()> {
    let state = AppState {
        runtime: Arc::new(runtime),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        // OpenAI Chat Completions 格式 / OpenAI Chat Completions format
        .route("/chat/completions", post(chat_completions_handler))
        // Anthropic Messages 格式 / Anthropic Messages format
        .route("/v1/messages", post(messages_handler))
        // OpenAI Responses 格式 / OpenAI Responses format
        .route("/responses", post(responses_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {host}:{port}: {e}"))?;
    info!(%addr, "pigs local API listening");
    eprintln!("[pigs API] listening on http://{addr}");
    eprintln!("[pigs API]   GET  /health");
    eprintln!("[pigs API]   GET  /v1/models");
    eprintln!("[pigs API]   POST /chat/completions   (OpenAI Chat)");
    eprintln!("[pigs API]   POST /v1/messages        (Anthropic)");
    eprintln!("[pigs API]   POST /responses         (OpenAI Responses)");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 基础端点 / Basic endpoints
// ---------------------------------------------------------------------------

/// 健康检查端点。
/// Health check endpoint.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "pigs",
        "mode": "phased-api",
        "stream": true,
        "formats": ["openai-chat", "anthropic", "openai-responses"]
    }))
}

/// 列出可用模型端点。
/// List available models endpoint.
async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "object": "list",
        "data": [{
            "id": state.runtime.wrapped_model.clone(),
            "object": "model",
            "owned_by": "pigs",
            "root": state.runtime.remote_model.clone(),
            "phases": "pre -> executor -> post",
        }]
    }))
}

// ---------------------------------------------------------------------------
// 三格式处理器 / Three-format handlers
// ---------------------------------------------------------------------------

/// OpenAI Chat Completions 处理器。
/// OpenAI Chat Completions handler.
///
/// 接收原始 JSON body，用 `ApiFormat::OpenAIChat` 解析，
/// 然后进入共享处理逻辑。
/// Receives raw JSON body, parses with `ApiFormat::OpenAIChat`,
/// then enters the shared handler logic.
async fn chat_completions_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, (StatusCode, String)> {
    handle_request(state, body, ApiFormat::OpenAIChat).await
}

/// Anthropic Messages 处理器。
/// Anthropic Messages handler.
///
/// 接收原始 JSON body，用 `ApiFormat::Anthropic` 解析，
/// 然后进入共享处理逻辑。
/// Receives raw JSON body, parses with `ApiFormat::Anthropic`,
/// then enters the shared handler logic.
async fn messages_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, (StatusCode, String)> {
    handle_request(state, body, ApiFormat::Anthropic).await
}

/// OpenAI Responses 处理器。
/// OpenAI Responses handler.
///
/// 接收原始 JSON body，用 `ApiFormat::OpenAIResponses` 解析，
/// 然后进入共享处理逻辑。
/// Receives raw JSON body, parses with `ApiFormat::OpenAIResponses`,
/// then enters the shared handler logic.
async fn responses_handler(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Response, (StatusCode, String)> {
    handle_request(state, body, ApiFormat::OpenAIResponses).await
}

// ---------------------------------------------------------------------------
// 共享处理逻辑 / Shared handler logic
// ---------------------------------------------------------------------------

/// 三种格式共用的处理函数。
///
/// Shared handler function for all three formats.
///
/// 1. 解析请求体为 `ConvertedTurn`
/// 2. 根据 `stream` 字段分流：
///    - `stream: true` → SSE 流式响应
///    - `stream: false` → 同步执行，返回非流式 JSON
/// 3. 响应格式与请求格式一致
///
/// 1. Parse request body into `ConvertedTurn`
/// 2. Dispatch based on `stream`:
///    - `stream: true` → SSE streaming response
///    - `stream: false` → synchronous, return non-streaming JSON
/// 3. Response format matches request format
async fn handle_request(
    state: AppState,
    body: axum::body::Bytes,
    format: ApiFormat,
) -> Result<Response, (StatusCode, String)> {
    // 将 bytes 解析为 JSON / Parse bytes into JSON
    let json_body: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid JSON: {e}"),
            )
        })?;

    // 按格式解析请求体 / Parse request body according to format
    let converted: ConvertedTurn = format
        .parse_request(&json_body)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid request: {e}"),
            )
        })?;

    // --- 流式路径 / Streaming path ---
    if converted.stream {
        return Ok(stream_turn(state, converted).into_response());
    }

    // --- 非流式路径 / Non-streaming path ---
    let result = run_converted_turn_arc(&state.runtime, &converted, None)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("turn failed: {e:#}")))?;

    // 按请求格式构造响应 / Build response in the same format as the request
    let resp_json = format.build_response(&result, &state.runtime.wrapped_model);
    Ok(Json(resp_json).into_response())
}

// ---------------------------------------------------------------------------
// SSE 流式实现 / SSE streaming implementation
// ---------------------------------------------------------------------------

/// 流式对话：通过 SSE 实时推送相位进度、文本增量和工具事件。
///
/// Streaming turn: pushes phase progress, text deltas, and tool events via SSE.
///
/// 帧格式根据 `converted.format` 选择：
/// Frame format depends on `converted.format`:
/// - OpenAI Chat → `data: {delta:{content}}\n\n`
/// - Anthropic → `event: content_block_delta\ndata: {...}\n\n`
/// - OpenAI Responses → `event: response.output_text.delta\ndata: {...}\n\n`
fn stream_turn(
    state: AppState,
    converted: ConvertedTurn,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let id = format!("pigs-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let model = state.runtime.wrapped_model.clone();
    let format = converted.format;
    let (tx, rx) = mpsc::unbounded_channel::<Result<String, Infallible>>();

    // 首帧：role chunk / message_start / response.created
    // First frame: role chunk / message_start / response.created
    let _ = tx.send(Ok(format.role_chunk(&id, created, &model)));

    // ProgressSink：将相位运行时进度转为 SSE 帧
    // ProgressSink: converts phased-runtime progress into SSE frames
    let tx_p = tx.clone();
    let id_p = id.clone();
    let model_p = model.clone();
    let fmt_p = format;
    let sink: ProgressSink = Arc::new(move |p: TurnProgress| match p {
        // 相位开始 → pigs.phase 自定义事件（三种格式通用）
        // Phase start → pigs.phase custom event (common to all formats)
        TurnProgress::PhaseStart { phase } => {
            let _ = tx_p.send(Ok(format!(
                "event: pigs.phase\ndata: {}\n\n",
                serde_json::json!({"phase": phase})
            )));
        }
        // 文本增量 → 按格式构造 content chunk
        // Text delta → format-specific content chunk
        TurnProgress::TextDelta { text, .. } => {
            if !text.is_empty() {
                let _ = tx_p.send(Ok(fmt_p.content_chunk(&id_p, created, &model_p, &text)));
            }
        }
        // 工具开始 → pigs.tool 自定义事件
        // Tool start → pigs.tool custom event
        TurnProgress::ToolStart { phase, name } => {
            let _ = tx_p.send(Ok(format!(
                "event: pigs.tool\ndata: {}\n\n",
                serde_json::json!({"action": "start", "phase": phase, "tool": name})
            )));
        }
        // 工具结束 → pigs.tool 自定义事件
        // Tool end → pigs.tool custom event
        TurnProgress::ToolEnd { phase, name, is_error } => {
            let _ = tx_p.send(Ok(format!(
                "event: pigs.tool\ndata: {}\n\n",
                serde_json::json!({"action": "end", "phase": phase, "tool": name, "error": is_error})
            )));
        }
        _ => {}
    });

    // 后台任务：执行相位对话并发送结束帧
    // Background task: execute phased turn and send end frames
    let runtime = Arc::clone(&state.runtime);
    let tx_end = tx;
    let id_e = id;
    let model_e = model;
    let fmt_e = format;
    tokio::spawn(async move {
        let result = run_converted_turn_arc(&runtime, &converted, Some(sink)).await;
        match result {
            Ok(r) => {
                // stop chunk / message_stop / response.completed
                let _ = tx_end.send(Ok(fmt_e.stop_chunk(&id_e, created, &model_e)));
                // pigs.done 自定义事件（三种格式通用）
                // pigs.done custom event (common to all formats)
                let _ = tx_end.send(Ok(format!(
                    "event: pigs.done\ndata: {}\n\n",
                    serde_json::json!({"ended_with": r.ended_with})
                )));
                // OpenAI Chat 需要 [DONE] 哨兵
                // OpenAI Chat needs [DONE] sentinel
                if let Some(done) = fmt_e.done_sentinel() {
                    let _ = tx_end.send(Ok(done));
                }
            }
            Err(e) => {
                // 错误 → 发送错误信息作为 content chunk
                // Error → send error as content chunk
                let msg = format!("turn failed: {e:#}");
                let _ = tx_end.send(Ok(fmt_e.content_chunk(&id_e, created, &model_e, &msg)));
                let _ = tx_end.send(Ok(fmt_e.stop_chunk(&id_e, created, &model_e)));
                if let Some(done) = fmt_e.done_sentinel() {
                    let _ = tx_end.send(Ok(done));
                }
            }
        }
    });

    // 将 String 流转为 axum Event 流
    // Convert String stream into axum Event stream
    let s = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(Ok(data)) => {
                // 将原始 SSE 帧字符串包装为 axum Event::data
                // Wrap raw SSE frame string as axum Event::data
                let event = Event::default().data(data);
                Some((Ok(event), rx))
            }
            Some(Err(_)) => None,
            None => None,
        }
    });

    Sse::new(s).keep_alive(KeepAlive::default())
}
