//! 日志初始化：控制台（彩色）+ 文件（按天滚动，无 ANSI），级别由 RUST_LOG 控制。

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

pub const LOG_DIR: &str = "logs";
pub const LOG_PREFIX: &str = "pigs.log";

/// 初始化双输出日志；返回的 guard 必须活到进程结束（否则文件日志会被截断）。
pub fn init() -> anyhow::Result<WorkerGuard> {
    std::fs::create_dir_all(LOG_DIR)?;
    let file_appender = tracing_appender::rolling::daily(LOG_DIR, LOG_PREFIX);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = fmt::layer().with_target(false).with_filter(filter());
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(non_blocking)
        .with_filter(filter());

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();

    Ok(guard)
}
