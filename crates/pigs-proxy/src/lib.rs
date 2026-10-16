//! pigs-proxy：HTTP 入口与传输层。 / HTTP entry point and transport layer.
//!
//! 单端口承载三协议；识别 PIGS 后缀后编排，否则透传。 / One listener serves three protocols; PIGS-suffixed models are orchestrated, others pass through.
//! 上游可为 mini-proxy 或直连接口。 / Upstream may be mini-proxy or any direct API.

pub mod config;
pub mod diagnostics;

/// 初始配置模板与示例导出。 / Initial configuration template and example export.
pub fn config_template() -> &'static str {
    config::Config::example()
}
pub mod server;
pub mod upstream;

use axum::serve as axum_serve;
use std::sync::Arc;
use tokio::net::TcpListener;

pub use config::{Config, LogDetail, LoggingConfig, OrchestrationConfig, Upstreams, UsageMode};

/// 启动服务并阻塞至退出。 / Start serving and block until exit.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let listener = bind_listener(&config.listen).await?;
    let addr = listener.local_addr()?;
    let app = build(config, addr);
    axum_serve(listener, app).await?;
    Ok(())
}

/// 绑定监听地址；零端口可分配随机端口。 / Bind a listener; port zero allocates an ephemeral port.
pub async fn bind_listener(listen: &str) -> anyhow::Result<TcpListener> {
    Ok(TcpListener::bind(listen).await?)
}

/// 使用预绑定监听器启动服务。 / Start the server with a pre-bound listener.
pub async fn serve_on(listener: TcpListener, config: Config) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    let app = build(config, addr);
    axum_serve(listener, app).await?;
    Ok(())
}

/// 构建请求路由器。 / Build the request router.
fn build(config: Config, addr: std::net::SocketAddr) -> axum::Router {
    tracing::info!(
        listen = %addr,
        openai = %config.upstream.openai,
        responses = %config.upstream.responses,
        anthropic = %config.upstream.anthropic,
        max_exec_runs = config.orchestration.max_exec_runs,
        max_post_protocol_retries = config.orchestration.max_post_protocol_retries,
        usage_mode = ?config.orchestration.usage_mode,
        "pigs 已启动"
    );
    println!("═══════════════════════════════════════════");
    println!("  pigs 已启动，监听 http://{addr}");
    println!(
        "  上游：chat={} responses={} anthropic={}",
        config.upstream.openai, config.upstream.responses, config.upstream.anthropic
    );
    println!("  -pigs(A) / -pig(B) / -pigs3(A full) / -pig3(B full) 编排：Pre → Exec → Post");
    println!(
        "  编排次数：Exec 最多 {} 次；Post 无标记协议重试最多 {} 次",
        config.orchestration.max_exec_runs, config.orchestration.max_post_protocol_retries
    );
    println!("  usage 汇报模式：{:?}", config.orchestration.usage_mode);
    println!(
        "  HTTP 诊断日志：{:?} → {}",
        config.logging.detail, config.logging.directory
    );
    println!("═══════════════════════════════════════════");
    server::router(build_state(config, addr))
}

/// 从配置构建共享应用状态。 / Construct shared application state from configuration.
pub fn build_state(config: Config, addr: std::net::SocketAddr) -> server::AppState {
    let self_host = match addr.ip() {
        ip if ip.is_unspecified() => std::net::IpAddr::from([127, 0, 0, 1]),
        ip => ip,
    };
    let self_url = format!("http://{}:{}", self_host, addr.port());
    server::AppState {
        diagnostics: Arc::new(diagnostics::HttpDiagnostics::new(&config.logging)),
        config: Arc::new(config.clone()),
        upstream: Arc::new(upstream::Upstream::new(&config.upstream, &config.key)),
        loopback_token: Arc::new(uuid::Uuid::now_v7().to_string()),
        self_url: Arc::new(self_url),
        store: Arc::new(std::sync::Mutex::new(
            pigs_orchestrator::state::ContinuationStore::default(),
        )),
    }
}
