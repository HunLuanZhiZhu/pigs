//! pigs-proxy — 多协议 HTTP 代理 + 相位化 Agent 路由器。
//! pigs-proxy — Multi-protocol HTTP proxy + phased agent router.
//!
//! 从 mini-proxy 演化而来，作为 pigs 项目的**前置路由层**。
//! Evolved from mini-proxy, serving as the **front router** for the pigs project.
//!
//! 核心职责 / Core responsibilities:
//! 1. 监听单一端口（默认 3927），接收三种协议的请求：
//!    Listens on a single port (default 3927), accepting three protocols:
//!    - `/chat/completions` — OpenAI Chat
//!    - `/v1/messages` — Anthropic Messages
//!    - `/responses` — OpenAI Responses
//! 2. 按 model ID 路由：
//!    Routes by model ID:
//!    - 无 `-pig` 后缀 → 透传到上游 LLM（原始代理逻辑）
//!      No `-pig` suffix → passthrough to upstream LLM (original proxy logic)
//!    - 有 `-pig` 后缀 → 转给 pigs-api 相位运行时（Pre→Executor→Post）
//!      Has `-pig` suffix → route to pigs-api phased runtime (Pre→Executor→Post)
//! 3. `/v1/models` 返回 ×2 模型列表（每个模型 + 其 `-pig` 版本）
//!    `/v1/models` returns ×2 model list (each model + its `-pig` variant)

pub mod config;
pub mod log;
pub mod protocol;
pub mod proxy_client;
pub mod retry;
pub mod server;
pub mod upstream;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::Value;

/// 从 pigs-proxy 配置构建 PhasedRuntime（通过 ProxyApiClient 走 pigs-proxy 重试）。
///
/// Build a PhasedRuntime from pigs-proxy config (via ProxyApiClient through pigs-proxy retry).
///
/// LLM 请求不直接连上游，而是通过 `dispatch_in_process` 走代理重试逻辑。
/// LLM requests don't connect to upstream directly; instead they go through
/// `dispatch_in_process` for proxy retry logic.
pub fn build_phased_runtime(
    proxy_config: Arc<config::Config>,
    model: &str,
    language: pigs_config::Language,
    limits: pigs_api::phased_runtime::RuntimeLimits,
) -> anyhow::Result<pigs_api::phased_runtime::PhasedRuntime> {
    use pigs_api::phased_tools::info_tool_registry;
    use pigs_core::ApiClient;

    // 默认用 OpenAI Chat 协议 / Default to OpenAI Chat protocol
    let protocol = protocol::Protocol::OpenAI;
    let api: Arc<dyn ApiClient> = Arc::new(proxy_client::ProxyApiClient::new(
        proxy_config,
        protocol,
        model.to_string(),
    ));
    let wrapped_model = format!("{}-pig", model);
    Ok(pigs_api::phased_runtime::PhasedRuntime {
        api,
        remote_model: model.to_string(),
        wrapped_model,
        tools: info_tool_registry(),
        limits,
        language,
    })
}

/// 进程内调度请求（不经过 HTTP，直接走重试逻辑）。
///
/// In-process request dispatch (bypasses HTTP, goes straight to retry logic).
///
/// 供 pigs-api 的 `ProxyApiClient` 调用：
/// 三相位的 LLM 请求不走 HTTP loopback，而是直接调这个函数，
/// 自动享受 `retry::dispatch` 的重试 + body 清洗 + 思考强度注入。
///
/// Called by pigs-api's `ProxyApiClient`:
/// phased LLM requests bypass HTTP loopback and call this function directly,
/// automatically benefiting from `retry::dispatch`'s retry + body cleaning
/// + thinking-effort injection.
///
/// # 参数 / Parameters
/// - `config`: 代理配置
/// - `client`: 上游 HTTP 客户端
/// - `protocol`: 请求协议（决定上游 URL 后缀）
/// - `model`: 模型 ID（不含 `-pig` 后缀，用于端点匹配）
/// - `body`: 请求体 JSON（包含 messages/tools/temperature 等）
///
/// # 返回 / Returns
/// - `Ok(Value)`: 上游响应体 JSON
/// - `Err(String)`: 错误描述
pub async fn dispatch_in_process(
    config: &config::Config,
    client: &upstream::UpstreamClient,
    protocol: protocol::Protocol,
    model: &str,
    body: &Value,
) -> Result<Value, String> {
    // 查找匹配的端点 / Find matching endpoint
    let endpoint = match server::pick_endpoint(config, protocol, model) {
        Some(ep) => ep,
        None => {
            return Err(format!(
                "no matching endpoint for protocol {:?} model {}",
                protocol, model
            ));
        }
    };

    // 克隆 body 做可变处理 / Clone body for mutation
    let mut parsed = body.clone();

    // 模型名映射 / Model name mapping
    let upstream_model = endpoint.map_model(model);
    if upstream_model != model {
        if let Some(obj) = parsed.as_object_mut() {
            obj.insert("model".into(), Value::String(upstream_model.clone()));
        }
    }

    // 清洗空 content 项 / Clean empty-content messages
    if config.server.clean_empty_content {
        server::clean_empty_messages(&mut parsed, protocol);
    }

    // 思考强度注入 / Thinking-effort injection
    let effort = endpoint
        .thinking_effort
        .clone()
        .unwrap_or_else(|| protocol.default_effort().to_string());
    server::inject_thinking(&mut parsed, protocol, &effort);

    // 序列化处理后的 body / Serialize processed body
    let body_bytes = bytes::Bytes::from(
        serde_json::to_vec(&parsed).unwrap_or_else(|_| serde_json::to_vec(body).unwrap_or_default()),
    );

    // 空请求头（进程内调用无客户端头）/ Empty headers (no client headers in-process)
    let headers = HeaderMap::new();

    // 走重试调度 / Go through retry dispatch
    let outcome = retry::dispatch(client, &endpoint, protocol, &body_bytes, &headers).await;

    match outcome {
        retry::DispatchOutcome::Ok(resp) => {
            // 读取响应 body / Read response body
            use axum::body::to_bytes;
            let bytes = to_bytes(resp.into_body(), 1024 * 1024 * 10)
                .await
                .map_err(|e| format!("failed to read response body: {e}"))?;
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| format!("failed to parse response JSON: {e}"))
        }
        retry::DispatchOutcome::Failed { status, body } => {
            Err(format!(
                "upstream failed with status {}: {}",
                status,
                String::from_utf8_lossy(&body)
            ))
        }
    }
}

use anyhow::Result;
use tracing::info;

/// 启动 HTTP 代理服务器。
///
/// Start the HTTP proxy server.
///
/// # 参数 / Parameters
/// - `config`: 代理配置（含 provider/endpoint/日志等）
///   Proxy configuration (providers, endpoints, logging, etc.)
/// - `runtime`: 相位化运行时（用于 `-pig` 模型路由）
///   Phased runtime (used for `-pig` model routing)
pub async fn serve(
    config: config::Config,
    runtime: pigs_api::phased_runtime::PhasedRuntime,
) -> Result<()> {
    log::init(&config.log)?;
    tracing::info!("配置加载完成");

    // 启动时打印渠道信息 / Print provider info at startup
    for p in &config.provider {
        if let Some(ep) = p.openai_endpoint() {
            info!(
                provider = %p.name,
                protocol = "openai",
                base_url = %ep.base_url,
                models = ?ep.models,
                max_retries = ep.max_retries,
                key_mode = ?ep.key_mode,
                "已加载渠道"
            );
        }
        if let Some(ep) = p.anthropic_endpoint() {
            info!(
                provider = %p.name,
                protocol = "anthropic",
                base_url = %ep.base_url,
                models = ?ep.models,
                max_retries = ep.max_retries,
                key_mode = ?ep.key_mode,
                "已加载渠道"
            );
        }
        if let Some(ep) = p.responses_endpoint() {
            info!(
                provider = %p.name,
                protocol = "responses",
                base_url = %ep.base_url,
                models = ?ep.models,
                max_retries = ep.max_retries,
                key_mode = ?ep.key_mode,
                "已加载渠道"
            );
        }
    }

    let client = Arc::new(upstream::UpstreamClient::new());
    let state = server::AppState {
        config: Arc::new(config.clone()),
        client,
        runtime: Arc::new(runtime),
    };
    let app = server::build(state);

    let listen = config.server.listen.clone();
    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid listen address {listen}: {e}"))?;

    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  pigs-proxy 已启动，监听 {}", listen);
    eprintln!();
    eprintln!("  对外服务端点：");
    eprintln!("      POST http://{}/chat/completions  → OpenAI 协议", listen);
    eprintln!("      POST http://{}/v1/messages       → Anthropic 协议", listen);
    eprintln!("      POST http://{}/responses         → Response 协议", listen);
    eprintln!("      GET  http://{}/v1/models         → 模型列表（×2）", listen);
    eprintln!();
    eprintln!("  model ID 无 -pig 后缀 → 透传上游");
    eprintln!("  model ID 有 -pig 后缀 → 相位化运行时（Pre→Executor→Post）");
    eprintln!("═══════════════════════════════════════════════════════════");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(listen = %listen, "服务启动完成");
    axum::serve(listener, app).await?;
    Ok(())
}
