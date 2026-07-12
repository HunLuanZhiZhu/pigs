// =============================================================================
// server.rs — 本地 OpenAI 兼容 HTTP API 宿主
// server.rs — Local OpenAI-compatible HTTP API host
// =============================================================================
//
// 本模块是 pigs 相位化 Agent 的 HTTP API 服务层。它使用 axum 框架提供
// 三个端点，与 OpenAI 的 Chat Completions API 格式兼容，以便现有工具和
// 客户端（如 curl、OpenAI SDK、Continue.dev、Open Interpreter 等）可以
// 直接接入 pigs 的相位化运行时。
//
// This module is the HTTP API service layer for the pigs phased agent.
// Using the axum framework, it provides three endpoints compatible with
// OpenAI's Chat Completions API format, so existing tools and clients
// (curl, OpenAI SDK, Continue.dev, Open Interpreter, etc.) can connect
// directly to pigs' phased runtime.
//
// 端点 / Endpoints:
//   GET  /health                  — 健康检查 / Health check
//   GET  /v1/models               — 列出可用模型 / List available models
//   POST /v1/chat/completions     — 聊天补全（流式/非流式）/ Chat completions (streaming or not)
//
// 相位流通过 pigs 扩展元数据（pigs.phase / pigs.tool / pigs.done SSE 事件）
// 暴露给客户端，而非 OpenAI 标准响应的一部分。
// The phase flow is exposed via pigs extension metadata
// (pigs.phase / pigs.tool / pigs.done SSE events), not part of the
// standard OpenAI response.

// ---------------------------------------------------------------------------
// 标准库导入 / Standard library imports
// ---------------------------------------------------------------------------

use std::convert::Infallible; // 用于 SSE 流的不可达错误类型 / Unreachable error type for SSE streams
use std::net::SocketAddr;     // 网络地址解析 / Network address parsing
use std::sync::Arc;           // 原子引用计数 / Atomic reference counting

// ---------------------------------------------------------------------------
// 第三方依赖 / Third-party dependencies
// ---------------------------------------------------------------------------

use axum::extract::State;                       // axum 状态提取器 / Axum state extractor
use axum::http::StatusCode;                     // HTTP 状态码 / HTTP status codes
use axum::response::sse::{Event, KeepAlive, Sse}; // SSE 响应类型 / SSE response types
use axum::response::{IntoResponse, Response};   // 响应转换 trait / Response conversion traits
use axum::routing::{get, post};                 // HTTP 方法路由 / HTTP method routing
use axum::{Json, Router};                       // JSON 解析 + 路由构建 / JSON parsing + router building
use futures_util::stream;                       // 流工具 / Stream utilities
use serde::Serialize;                           // JSON 序列化 trait / JSON serialization trait
use tokio::sync::mpsc;                          // 多生产者单消费者通道 / MPSC channel (for streaming)
use tower_http::cors::CorsLayer;                // CORS 中间件（允许跨域请求）/ CORS middleware
use tracing::info;                              // 结构化日志 / Structured logging

// ---------------------------------------------------------------------------
// 项目内部导入 / Internal crate imports (from pigs-api itself)
// ---------------------------------------------------------------------------

// phased_api_convert：负责将 OpenAI 格式的请求转换为相位运行时内部格式
// Handles conversion of OpenAI-format requests to phased runtime internal format
use crate::phased_api_convert::{
    run_converted_turn_arc,  // 异步执行一轮转换后的相位对话 / Run a converted phased turn
    ChatCompletionsRequest,  // OpenAI 格式的请求体 / OpenAI-format request body
    ConvertedTurn,           // 转换后的内部轮次表示 / Converted internal turn representation
};

// phased_runtime：相位化 Agent 运行时（pre→executor→post 三阶段）
// Phased agent runtime (pre→executor→post three-phase cycle)
use crate::phased_runtime::{
    PhasedRuntime,  // 相位运行时主结构体 / Main phased runtime struct
    ProgressSink,   // 进度回调类型（用于 SSE 推送）/ Progress callback type (for SSE push)
    TurnProgress,   // 轮次进度事件枚举 / Turn progress event enum
};

// ---------------------------------------------------------------------------
// 共享状态 / Shared application state
// ---------------------------------------------------------------------------

/// 应用共享状态：通过 axum 的 State 提取器注入到每个处理器。
///
/// Shared application state: injected into every handler via axum's State extractor.
///
/// 内部持有 `Arc<PhasedRuntime>`，确保在多个并发请求之间安全共享
/// （读取时分发 Arc clone，实际数据只有一份）。
/// Holds an `Arc<PhasedRuntime>` for safe sharing across concurrent requests
/// (each handler gets an Arc clone pointing to the same data).
#[derive(Clone)]
pub struct AppState {
    /// 相位化 Agent 运行时实例（Arc 包装，支持并发访问）
    /// Phased agent runtime instance (Arc-wrapped, supports concurrent access)
    pub runtime: Arc<PhasedRuntime>,
}

// ---------------------------------------------------------------------------
// 响应类型 / Response types
// ---------------------------------------------------------------------------

/// OpenAI 兼容的 chat completions 响应体（非流式）。
///
/// OpenAI-compatible chat completions response body (non-streaming).
///
/// 结构遵循 OpenAI API 规范：id / object / created / model / choices / usage。
/// 额外包含可选的 `pigs` 扩展字段，携带相位流元数据。
/// Follows the OpenAI API spec: id / object / created / model / choices / usage.
/// Plus an optional `pigs` extension field with phase-flow metadata.
#[derive(Debug, Serialize)]
pub struct ChatCompletionsResponse {
    /// 响应唯一 ID（如 "chatcmpl-<uuid>"）/ Unique response ID
    pub id: String,
    /// 对象类型，固定为 "chat.completion" / Object type, always "chat.completion"
    pub object: &'static str,
    /// Unix 时间戳（秒）/ Unix timestamp (seconds)
    pub created: i64,
    /// 使用的模型 ID / Model ID used
    pub model: String,
    /// 候选项列表（通常只有一个）/ Choices list (usually one)
    pub choices: Vec<Choice>,
    /// token 用量统计 / Token usage statistics
    pub usage: Usage,
    /// pigs 扩展元数据：包含结束标记和经过的相位列表。
    /// Pigs extension metadata: ended_with marker + phases traversed.
    /// 当为 None 时序列化跳过（不增加响应体积）
    /// Skipped during serialization when None (no extra payload)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pigs: Option<PigsMeta>,
}

/// 单个选项（choice），包含助手回复消息。
///
/// A single choice, containing the assistant reply message.
///
/// OpenAI 格式：index + message + finish_reason
/// OpenAI format: index + message + finish_reason
#[derive(Debug, Serialize)]
pub struct Choice {
    /// 选项索引（从 0 开始）/ Choice index (starting from 0)
    pub index: u32,
    /// 助手回复消息 / Assistant reply message
    pub message: OutMessage,
    /// 结束原因（"stop" / "length" / "tool_calls" 等）/ Finish reason
    pub finish_reason: String,
}

/// 输出消息：角色 + 文本内容。
///
/// Output message: role + text content.
///
/// 注意：此处仅支持纯文本消息，不包含 tool_calls 或多模态内容。
/// Note: Only plain text messages are supported here — no tool_calls or
/// multimodal content.
#[derive(Debug, Serialize)]
pub struct OutMessage {
    /// 角色（固定为 "assistant"）/ Role (always "assistant")
    pub role: &'static str,
    /// 回复文本内容 / Response text content
    pub content: String,
}

/// token 用量统计。
///
/// Token usage statistics.
///
/// 注意：pigs 的相位化运行时暂不精确追踪 prompt_tokens，
/// 因此 prompt_tokens 固定为 0。completion_tokens 根据文本长度估算（
/// 每 4 字符约 1 token）。
/// Note: pigs' phased runtime does not yet track prompt_tokens accurately,
/// so it is hardcoded to 0. completion_tokens is estimated from text length
/// (roughly 1 token per 4 characters).
#[derive(Debug, Serialize)]
pub struct Usage {
    /// 提示 token 数量（当前固定为 0）/ Prompt token count (currently hardcoded to 0)
    pub prompt_tokens: u32,
    /// 补全 token 数量（按字符数估算）/ Completion token count (estimated from char count)
    pub completion_tokens: u32,
    /// 总 token 数量 / Total token count
    pub total_tokens: u32,
}

/// pigs 扩展元数据：整轮结束方式 + 经过的相位列表。
///
/// Pigs extension metadata: how the turn ended + phases traversed.
///
/// 这提供了标准 OpenAI 响应中不存在的相位流信息，使客户端能够了解
/// Agent 内部经历了哪些阶段（pre / executor / post）以及最终如何结束
/// （PIGEND / PIGFAILED / 默认循环）。
/// This provides phase-flow information not present in standard OpenAI
/// responses, letting clients know which stages the agent traversed
/// (pre / executor / post) and how it ended (PIGEND / PIGFAILED / default loop).
#[derive(Debug, Serialize)]
pub struct PigsMeta {
    /// 结束标记（如 "PIGEND", "PIGFAILED"）/ End marker
    pub ended_with: String,
    /// 经过的相位名称列表（如 ["pre", "executor"]）/ Phases traversed
    pub phases: Vec<String>,
}

// ---------------------------------------------------------------------------
// 服务器入口 / Server entry point
// ---------------------------------------------------------------------------

/// 启动 HTTP API 服务器，监听指定 host:port。
///
/// Start the HTTP API server, listening on the given host:port.
///
/// 配置三个路由并应用宽松 CORS 策略后，绑定 TCP listener 并启动 axum serve。
/// 此函数会阻塞当前 async 上下文直到服务器关闭。
///
/// Configures three routes, applies a permissive CORS policy, then binds the
/// TCP listener and starts axum serve. This function blocks the current async
/// context until the server shuts down.
///
/// # 参数 / Parameters
/// - `runtime`: 已构建的 PhasedRuntime 实例 / Pre-built PhasedRuntime instance
/// - `host`: 绑定地址（如 "127.0.0.1"）/ Bind address (e.g. "127.0.0.1")
/// - `port`: 绑定端口（如 3927）/ Bind port (e.g. 3927)
///
/// # 返回值 / Returns
/// - `Ok(())`: 服务器正常关闭 / Server shut down normally
/// - `Err(e)`: 绑定失败或 serve 错误 / Bind failure or serve error
pub async fn serve(runtime: PhasedRuntime, host: &str, port: u16) -> anyhow::Result<()> {
    // 将运行时包装到 Arc 中，存入共享状态 / Wrap runtime in Arc, store in shared state
    let state = AppState {
        runtime: Arc::new(runtime),
    };

    // 构建 axum 路由树 / Build the axum route tree
    let app = Router::new()
        // GET /health — 健康检查 / Health check
        .route("/health", get(health))
        // GET /v1/models — 列出可用模型 / List available models
        .route("/v1/models", get(list_models))
        // POST /v1/chat/completions — 聊天补全（流式/非流式）/ Chat completions
        .route("/v1/chat/completions", post(chat_completions))
        // 宽松 CORS：允许所有来源（方便本地开发调试）
        // Permissive CORS: allow all origins (convenient for local dev)
        .layer(CorsLayer::permissive())
        // 注入共享状态 / Inject shared state
        .with_state(state);

    // 解析 host:port 为 SocketAddr / Parse host:port into SocketAddr
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid bind address {host}:{port}: {e}"))?;

    // tracing 日志记录服务器启动 / Log server startup via tracing
    info!(%addr, "pigs local API listening");

    // 向 stderr 打印可用端点信息（不干扰 stdout 管道）
    // Print available endpoints to stderr (no stdout pipe pollution)
    eprintln!("[pigs API] listening on http://{addr}");
    eprintln!("[pigs API]   GET  /health");
    eprintln!("[pigs API]   GET  /v1/models");
    eprintln!("[pigs API]   POST /v1/chat/completions  (stream=true|false)");

    // 绑定 TCP listener 并启动 axum 服务
    // Bind the TCP listener and start the axum service
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 路由处理器 / Route handlers
// ---------------------------------------------------------------------------

/// 健康检查端点。
///
/// Health check endpoint.
///
/// 返回 JSON 格式的状态信息，包含服务名、工作模式和流式支持标志。
/// 客户端（如 Docker health check、Kubernetes liveness probe）可轮询此端点
/// 确认服务器是否正常运行。
/// Returns JSON status info: service name, work mode, and streaming support flag.
/// Clients (Docker health check, K8s liveness probe) can poll this endpoint
/// to verify the server is running.
async fn health() -> impl IntoResponse {
    // 返回固定 JSON 结构 / Return fixed JSON structure
    Json(serde_json::json!({
        "status": "ok",
        "service": "pigs",
        "mode": "phased-api",
        "stream": true
    }))
}

/// 列出可用模型端点。
///
/// List available models endpoint.
///
/// 返回符合 OpenAI /v1/models 格式的列表。当前固定返回一个模型条目，
/// 其 id 来自运行时配置的 `wrapped_model`（对外暴露的模型名），
/// root 为 `remote_model`（实际后端模型名）。
/// Returns a list matching the OpenAI /v1/models format. Currently returns a
/// single model entry whose id comes from `wrapped_model` (externally exposed
/// model name) and root from `remote_model` (actual backend model name).
async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    // 返回 OpenAI 兼容的模型列表 / Return OpenAI-compatible model list
    Json(serde_json::json!({
        "object": "list",
        "data": [{
            // 对外暴露的模型 ID（如 "gpt-4o-pig"）/ Externally exposed model ID
            "id": state.runtime.wrapped_model.clone(),
            "object": "model",
            "owned_by": "pigs",
            // 实际后端模型（如 "gpt-4o"）/ Actual backend model
            "root": state.runtime.remote_model.clone(),
            // 相位流程说明 / Phase flow description
            "phases": "pre -> executor -> post",
        }]
    }))
}

// ---------------------------------------------------------------------------
// Chat Completions 处理器 / Chat Completions handler
// ---------------------------------------------------------------------------

/// chat completions 主处理器。
///
/// Main chat completions handler.
///
/// 1. 解析 OpenAI 格式的请求体 (`ChatCompletionsRequest`)
/// 2. 转换为内部格式 (`ConvertedTurn`)
/// 3. 根据 `stream` 字段分流：
///    - `stream: true` → 进入 SSE 流式响应 (`stream_chat`)
///    - `stream: false` → 同步执行一轮相位对话，返回 JSON 响应
///
/// 1. Parse the OpenAI-format request body (`ChatCompletionsRequest`)
/// 2. Convert to internal format (`ConvertedTurn`)
/// 3. Dispatch based on the `stream` field:
///    - `stream: true` → SSE streaming response (`stream_chat`)
///    - `stream: false` → synchronous phased turn, return JSON response
async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionsRequest>,
) -> Result<Response, (StatusCode, String)> {
    // 将 OpenAI 格式请求转换为相位运行时内部格式
    // Convert OpenAI-format request to phased runtime internal format
    let converted = ConvertedTurn::from_request(&req).map_err(|e| {
        // 转换失败 → 400 Bad Request（如消息格式错误）
        // Conversion failure → 400 Bad Request (e.g. malformed messages)
        (
            StatusCode::BAD_REQUEST,
            format!("invalid messages: {e}"),
        )
    })?;

    // --- 流式路径 / Streaming path ---
    if converted.stream {
        // 直接返回 SSE 流式响应 / Return SSE streaming response directly
        return Ok(stream_chat(state, converted).into_response());
    }

    // --- 非流式路径 / Non-streaming path ---
    //
    // 同步执行一轮相位对话（不传 ProgressSink → 无实时进度回调）
    // Execute a phased turn synchronously (no ProgressSink → no real-time callbacks)
    let result = run_converted_turn_arc(&state.runtime, &converted, None)
        .await
        .map_err(|e| {
            // 执行失败 → 502 Bad Gateway（如后端 LLM 调用失败）
            // Execution failure → 502 Bad Gateway (e.g. backend LLM call failed)
            (StatusCode::BAD_GATEWAY, format!("turn failed: {e:#}"))
        })?;

    // 从事件列表中提取经过的相位名称（用于 pigs 扩展元数据）
    // Extract phase names from the event list (for pigs extension metadata)
    let phases: Vec<String> = result
        .events
        .iter()
        // 只保留 phase_start 事件 / Only keep phase_start events
        .filter(|e| e.kind == "phase_start")
        // 提取 phase 字段 / Extract the phase field
        .filter_map(|e| e.phase.clone())
        .collect();

    // 构造响应元数据 / Build response metadata
    let created = chrono::Utc::now().timestamp();  // 当前 Unix 时间戳 / Current Unix timestamp
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4()); // 唯一 ID / Unique ID
    // 粗略估算 completion token 数（每 4 字符 ≈ 1 token，英文场景）
    // Rough estimate of completion tokens (~1 token per 4 chars in English)
    let completion_tokens = (result.final_text.len() / 4) as u32;

    // 组装 OpenAI 兼容响应 / Assemble OpenAI-compatible response
    let resp = ChatCompletionsResponse {
        id,
        object: "chat.completion",
        created,
        model: state.runtime.wrapped_model.clone(),
        choices: vec![Choice {
            index: 0,
            message: OutMessage {
                role: "assistant",
                content: result.final_text,
            },
            finish_reason: "stop".into(),
        }],
        usage: Usage {
            prompt_tokens: 0,          // 暂不追踪 / Not yet tracked
            completion_tokens,
            total_tokens: completion_tokens,
        },
        // 附加 pigs 扩展元数据 / Attach pigs extension metadata
        pigs: Some(PigsMeta {
            ended_with: result.ended_with,
            phases,
        }),
    };
    // 返回 JSON 响应 / Return JSON response
    Ok(Json(resp).into_response())
}

// ---------------------------------------------------------------------------
// SSE 流式实现 / SSE streaming implementation
// ---------------------------------------------------------------------------

/// 流式 chat completions：通过 SSE 实时推送相位进度、文本增量和工具事件。
///
/// Streaming chat completions: pushes phase progress, text deltas, and tool
/// events in real time via SSE.
///
/// 流式协议 / Streaming protocol:
/// 1. 首帧：role chunk（`{"delta": {"role": "assistant"}}`）
///    First frame: role chunk (signals assistant role start)
/// 2. 每帧 SSE 事件 / Each SSE event:
///    - `data: {...}` — 标准 OpenAI content chunk（文本增量）
///      Standard OpenAI content chunk (text delta)
///    - `event: pigs.phase, data: "<phase_name>"` — 相位开始事件
///      Phase start event
///    - `event: pigs.tool, data: "start|end <phase> <name> error=..."` — 工具事件
///      Tool event
/// 3. 末帧 / Last frames:
///    - `data: {"choices": [{"delta": {}, "finish_reason": "stop"}]}` — stop chunk
///    - `event: pigs.done, data: "<ended_with>"` — 结束标记 / End marker
///    - `data: [DONE]` — 流结束哨兵 / Stream end sentinel
///
/// # 参数 / Parameters
/// - `state`: 应用状态（含 Arc<PhasedRuntime>）/ App state (with Arc<PhasedRuntime>)
/// - `converted`: 已转换的相位轮次 / Converted phased turn
///
/// # 返回值 / Returns
/// - `Sse<impl Stream<Item = Result<Event, Infallible>>>` — SSE 响应流
fn stream_chat(
    state: AppState,
    converted: ConvertedTurn,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    // 生成唯一 chat completion ID 和当前时间戳
    // Generate unique chat completion ID and current timestamp
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let model = state.runtime.wrapped_model.clone();

    // 创建无界 MPSC 通道：tx（发送端）→ rx（接收端，作为 SSE 流的源）
    // Create unbounded MPSC channel: tx (sender) → rx (receiver, SSE stream source)
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();

    // ---- 首帧：发送 role chunk ----
    // ---- First frame: send role chunk ----
    // 根据 OpenAI SSE 协议，首帧 delta 应包含 role: "assistant"
    // Per OpenAI SSE protocol, the first frame delta should contain role: "assistant"
    let _ = tx.send(Ok(Event::default().data(role_json(&id, created, &model))));

    // ---- 构建 ProgressSink（实时进度回调） ----
    // ---- Build ProgressSink (real-time progress callback) ----
    //
    // ProgressSink 是一个闭包，在相位运行时执行过程中被回调，将内部进度事件
    // 转换为 SSE 事件推送到客户端。
    // ProgressSink is a closure called during phased runtime execution,
    // converting internal progress events into SSE events pushed to the client.
    let tx_p = tx.clone();
    let id_p = id.clone();
    let model_p = model.clone();
    let sink: ProgressSink = Arc::new(move |p: TurnProgress| match p {
        // 相位开始事件 → 发送 pigs.phase SSE 事件
        // Phase start event → send pigs.phase SSE event
        TurnProgress::PhaseStart { phase } => {
            let _ = tx_p.send(Ok(Event::default().event("pigs.phase").data(phase)));
        }
        // 文本增量事件 → 发送 OpenAI content chunk（标准流式帧）
        // Text delta event → send OpenAI content chunk (standard streaming frame)
        TurnProgress::TextDelta { text, .. } => {
            if !text.is_empty() {
                let _ = tx_p.send(Ok(Event::default().data(content_json(
                    &id_p, created, &model_p, &text,
                ))));
            }
        }
        // 工具开始事件 → 发送 pigs.tool SSE 事件（含相位和工具名）
        // Tool start event → send pigs.tool SSE event (with phase and tool name)
        TurnProgress::ToolStart { phase, name } => {
            let _ = tx_p.send(Ok(Event::default()
                .event("pigs.tool")
                .data(format!("start {phase} {name}"))));
        }
        // 工具结束事件 → 发送 pigs.tool SSE 事件（含错误标志）
        // Tool end event → send pigs.tool SSE event (with error flag)
        TurnProgress::ToolEnd {
            phase,
            name,
            is_error,
        } => {
            let _ = tx_p.send(Ok(Event::default()
                .event("pigs.tool")
                .data(format!("end {phase} {name} error={is_error}"))));
        }
        // 忽略其他事件类型（如 PhaseEnd, ToolDelta 等，暂不处理）
        // Ignore other event types (PhaseEnd, ToolDelta, etc. — not yet handled)
        _ => {}
    });

    // ---- 后台任务：执行相位对话并发送结束帧 ----
    // ---- Background task: execute phased turn and send end frames ----
    let runtime = Arc::clone(&state.runtime);
    let tx_end = tx;
    let id_e = id;
    let model_e = model;
    tokio::spawn(async move {
        // 执行一轮相位对话，传入 ProgressSink 以实时推送进度
        // Execute a phased turn, passing ProgressSink for real-time progress push
        let result = run_converted_turn_arc(&runtime, &converted, Some(sink)).await;

        match result {
            Ok(r) => {
                // 成功：所有相位文本已通过 TextDelta 回调流式推送完毕
                // Success: all phase text already streamed via TextDelta callbacks
                // 发送 stop chunk（finish_reason: "stop"）
                // Send stop chunk (finish_reason: "stop")
                let _ = tx_end.send(Ok(Event::default().data(stop_json(&id_e, created, &model_e))));
                // 发送 pigs.done 事件（含结束标记，如 "PIGEND"）
                // Send pigs.done event (with end marker, e.g. "PIGEND")
                let _ = tx_end.send(Ok(Event::default().event("pigs.done").data(r.ended_with)));
                // 发送 [DONE] 哨兵标记流结束
                // Send [DONE] sentinel to mark stream end
                let _ = tx_end.send(Ok(Event::default().data("[DONE]")));
            }
            Err(e) => {
                // 失败：发送错误信息作为 content chunk
                // Failure: send error message as content chunk
                let msg = format!("turn failed: {e:#}");
                let _ = tx_end.send(Ok(Event::default().data(content_json(
                    &id_e, created, &model_e, &msg,
                ))));
                // 同样发送 stop chunk 和 [DONE] 以优雅结束流
                // Also send stop chunk and [DONE] to gracefully end the stream
                let _ = tx_end.send(Ok(Event::default().data(stop_json(&id_e, created, &model_e))));
                let _ = tx_end.send(Ok(Event::default().data("[DONE]")));
            }
        }
    });

    // ---- 将 MPSC 接收端转换为 futures_util Stream ----
    // ---- Convert MPSC receiver into a futures_util Stream ----
    //
    // stream::unfold 将 rx.recv() 的异步迭代转换为 Stream trait，
    // 供 Sse 包装器使用。
    // stream::unfold converts the rx.recv() async iteration into a Stream trait
    // for the Sse wrapper.
    let s = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });

    // 包装为 SSE 响应并启用 keep-alive 保活机制
    // Wrap as SSE response with keep-alive enabled
    Sse::new(s).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// SSE 帧构造辅助函数 / SSE frame construction helpers
// ---------------------------------------------------------------------------

/// 构造 role chunk：首帧的 delta 中包含 role: "assistant"。
///
/// Build a role chunk: the first frame's delta contains role: "assistant".
///
/// 根据 OpenAI SSE 流式协议，响应应以一个含 role 的 delta 帧开始。
/// 后续帧的 delta 只含 content 字段。
/// Per OpenAI SSE streaming protocol, the response should start with a delta
/// frame containing role. Subsequent frames have delta with only content.
fn role_json(id: &str, created: i64, model: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant"},
            "finish_reason": null
        }]
    })
    .to_string()
}

/// 构造 content chunk：delta 中包含文本增量。
///
/// Build a content chunk: delta contains a text increment.
///
/// 这是主要的 SSE 数据帧，承载 LLM 生成的逐段文本输出。
/// 客户端通常将这些增量拼接成完整的回复文本。
/// This is the main SSE data frame, carrying incremental text output from the
/// LLM. Clients typically concatenate these increments into the full reply text.
fn content_json(id: &str, created: i64, model: &str, content: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"content": content},
            "finish_reason": null
        }]
    })
    .to_string()
}

/// 构造 stop chunk：finish_reason=stop，标记流结束。
///
/// Build a stop chunk: finish_reason=stop, marking the end of the stream.
///
/// 此帧的 delta 为空对象，表示不再有新的文本内容。
/// finish_reason 告知客户端生成结束的原因（此处总是 "stop"）。
/// This frame has an empty delta object, indicating no more text content.
/// finish_reason tells the client why generation stopped (always "stop" here).
fn stop_json(id: &str, created: i64, model: &str) -> String {
    serde_json::json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    })
    .to_string()
}
