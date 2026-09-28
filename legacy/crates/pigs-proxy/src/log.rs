// 日志系统：stdout + 文件双输出，按天滚动
// 也可通过 channel 桥接到 TUI（不输出到 stdout）

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, prelude::*, util::SubscriberInitExt, EnvFilter};

use crate::config::LogConfig;

pub fn init(cfg: &LogConfig) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let mut layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static>,
    > = Vec::new();

    if cfg.to_stdout {
        let l: Box<dyn tracing_subscriber::Layer<_> + Send + Sync + 'static> =
            match cfg.format.as_str() {
                "json" => tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(filter.clone())
                    .boxed(),
                _ => tracing_subscriber::fmt::layer()
                    .pretty()
                    .with_filter(filter.clone())
                    .boxed(),
            };
        layers.push(l);
    }

    if !cfg.to_file.is_empty() {
        let parent = std::path::Path::new(&cfg.to_file)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).ok();
        let prefix = std::path::Path::new(&cfg.to_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("proxy");
        let suffix = std::path::Path::new(&cfg.to_file)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("log");
        let appender = tracing_appender::rolling::daily(parent, format!("{}.{}", prefix, suffix));
        layers.push(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(appender)
                .with_filter(filter.clone())
                .boxed(),
        );
    }

    let subscriber = tracing_subscriber::registry().with(layers);
    // 使用 try_init 避免与已设置的全场 subscriber 冲突（如 pigs-cli 的 init_logging）。
    // Use try_init to avoid panicking when a global subscriber is already set
    // (e.g. by pigs-cli's init_logging).
    subscriber
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to set global trace subscriber: {e}"))?;
    Ok(())
}

/// Initialize logging with a TUI bridge layer instead of stdout.
/// File logging is still active. Tracing events are forwarded to the TUI as formatted strings.
///
/// Used in normal mode (API + TUI) so proxy logs don't pollute the terminal.
pub fn init_with_bridge(
    cfg: &LogConfig,
    log_sender: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let mut layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static>,
    > = Vec::new();

    // TUI bridge layer (replaces stdout) — sends formatted strings
    layers.push(Box::new(TuiLogBridge {
        sender: log_sender,
    }) as Box<dyn tracing_subscriber::Layer<_> + Send + Sync + 'static>);

    // File layer (same as init)
    if !cfg.to_file.is_empty() {
        let parent = std::path::Path::new(&cfg.to_file)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).ok();
        let prefix = std::path::Path::new(&cfg.to_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("proxy");
        let suffix = std::path::Path::new(&cfg.to_file)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("log");
        let appender = tracing_appender::rolling::daily(parent, format!("{}.{}", prefix, suffix));
        layers.push(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(appender)
                .with_filter(filter.clone())
                .boxed(),
        );
    }

    let subscriber = tracing_subscriber::registry().with(layers);
    subscriber
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to set global trace subscriber: {e}"))?;
    Ok(())
}

/// Tracing layer that forwards events to a channel for TUI display.
struct TuiLogBridge {
    sender: tokio::sync::mpsc::UnboundedSender<String>,
}

impl<S> tracing_subscriber::Layer<S> for TuiLogBridge
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = event.metadata().level();
        let target = event.metadata().target();

        // Format the message
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = if visitor.message.is_empty() {
            event.metadata().name().to_string()
        } else {
            visitor.message
        };

        // Format: [LEVEL target] message
        let formatted = format!("[{level} {target}] {message}");

        // Send to channel (ignore if receiver dropped)
        let _ = self.sender.send(formatted);
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(&format!("{}={}", field.name(), value));
        }
    }

    fn record_error(&mut self, field: &tracing::field::Field, value: &(dyn std::error::Error + 'static)) {
        if !self.message.is_empty() {
            self.message.push(' ');
        }
        self.message.push_str(&format!("{}={}", field.name(), value));
    }
}
