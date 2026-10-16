//! 日志初始化：彩色控制台及按天滚动的纯文本文件。 / Initialize colored console logging and daily plain-text file rotation.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

pub const LOG_DIR: &str = "logs";
pub const LOG_PREFIX: &str = "pigs.log";

/// 初始化双路日志；守卫必须持续到进程退出。 / Initialize dual logging; retain the guard until process exit to avoid truncation.
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
