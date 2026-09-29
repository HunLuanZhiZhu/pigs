//! pigs-proxy —— HTTP 入口与传输壳。
//!
//! 单端口接住三种协议；model 带 `-pig` 走编排，否则原样透传到上游。
//! 上游可以是 mini-proxy（多拿会话头/映射/重试）或任意直连 API，本 crate 不关心。

pub mod config;

/// 配置模板（首次生成 / --example 导出用）。
pub fn config_template() -> &'static str {
    config::Config::example()
}
pub mod server;
pub mod upstream;

use axum::serve as axum_serve;
use std::sync::Arc;
use tokio::net::TcpListener;

pub use config::{Config, Upstreams};

/// 启动服务（阻塞直到进程退出）。
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let listener = bind_listener(&config.listen).await?;
    let addr = listener.local_addr()?;
    let app = build(config, addr);
    axum_serve(listener, app).await?;
    Ok(())
}

/// 绑定监听（测试用 `127.0.0.1:0` 拿随机端口）。
pub async fn bind_listener(listen: &str) -> anyhow::Result<TcpListener> {
    Ok(TcpListener::bind(listen).await?)
}

/// 由已绑定的 listener 构建并启动服务（测试注入随机端口）。
pub async fn serve_on(listener: TcpListener, config: Config) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    let app = build(config, addr);
    axum_serve(listener, app).await?;
    Ok(())
}

/// 组装 Router。
fn build(config: Config, addr: std::net::SocketAddr) -> axum::Router {
    tracing::info!(
        listen = %addr,
        openai = %config.upstream.openai,
        responses = %config.upstream.responses,
        anthropic = %config.upstream.anthropic,
        "pigs 已启动"
    );
    println!("═══════════════════════════════════════════");
    println!("  pigs 已启动，监听 http://{addr}");
    println!(
        "  上游：chat={} responses={} anthropic={}",
        config.upstream.openai, config.upstream.responses, config.upstream.anthropic
    );
    println!("  -pig 编排：Pre → Executor → Post");
    println!("═══════════════════════════════════════════");
    server::router(build_state(config, addr))
}

/// 便捷构造：按配置构建 AppState（测试与 bin 共用）。
pub fn build_state(config: Config, addr: std::net::SocketAddr) -> server::AppState {
    let self_host = match addr.ip() {
        ip if ip.is_unspecified() => std::net::IpAddr::from([127, 0, 0, 1]),
        ip => ip,
    };
    let self_url = format!("http://{}:{}", self_host, addr.port());
    server::AppState {
        config: Arc::new(config.clone()),
        upstream: Arc::new(upstream::Upstream::new(&config.upstream, &config.key)),
        loopback_token: Arc::new(uuid::Uuid::now_v7().to_string()),
        self_url: Arc::new(self_url),
        store: Arc::new(std::sync::Mutex::new(
            pigs_orchestrator::state::ContinuationStore::default(),
        )),
    }
}
