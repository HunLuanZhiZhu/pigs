//! Pigs CLI library — the complete local agent / REPL.
//!
//! This crate is a **library** (not a binary). The product binary `pigs`
//! imports it for `--cli` mode. All agent, REPL, slash-command, MCP, hooks,
//! doctor, i18n, and session logic lives here.

pub mod agent;
pub mod cli;
pub mod command_aliases;
pub mod commands;
pub mod doctor;
pub mod hooks;
pub mod http_client;
pub mod i18n;
pub mod models;
pub mod output;
pub mod repl;
pub mod skill_tool;
pub mod snapshots;
pub mod sub_agent;
pub mod sub_agent_tool;
pub mod tui_repl;

// 相位模块已移至 pigs-api crate；此处 re-export 保持旧路径可用。
// Phased modules moved to pigs-api crate; re-export keeps old paths working.
pub use pigs_api::phased_api_convert;
pub use pigs_api::phased_markers;
pub use pigs_api::phased_phase;
pub use pigs_api::phased_prompts;
pub use pigs_api::phased_runtime;
pub use pigs_api::phased_tools;

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Run the full pigs-cli agent (REPL or one-shot).
///
/// Called by `pigs --cli` — the product binary delegates here instead of
/// implementing its own CLI logic. Reads args from `std::env::args()`.
pub async fn run_cli() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Like [`run_cli`] but with explicit args (for forwarding from `pigs --cli`).
///
/// `args` should include the program name as the first element (e.g.
/// `["pigs", "--list-sessions"]`).
pub async fn run_cli_from(args: Vec<String>) -> ExitCode {
    match run_from(&args, None).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Like [`run_cli_from`] but with an injected `ApiClient`.
///
/// 当 `api_client` 为 `Some` 时，Agent 使用它代替内部创建的 `pigs-llm` 直连客户端。
/// `pigs` 二进制借此将 CLI 的 LLM 请求统一经过 `pigs-proxy` 的
/// `ProxyApiClient` → `dispatch_in_process` 重试逻辑。
///
/// When `api_client` is `Some`, the agent uses it instead of building its own
/// direct `pigs-llm` client. The `pigs` binary uses this to route CLI LLM
/// requests through `pigs-proxy`'s `ProxyApiClient` → `dispatch_in_process`.
pub async fn run_cli_with_client(
    args: Vec<String>,
    api_client: Option<Arc<dyn pigs_core::ApiClient>>,
) -> ExitCode {
    match run_from(&args, api_client).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Like [`run_cli_from`] but connects to the local API server over HTTP.
///
/// The agent uses `HttpAgentClient` to send LLM requests to
/// `http://{host}:{port}/chat/completions` with real SSE streaming.
/// The model name sent is the real (non-`-pig`) model name, so the API
/// server routes it through passthrough; `PhasedRuntime` inside the agent
/// does Pre→Executor→Post orchestration locally.
pub async fn run_cli_with_http(args: Vec<String>, host: String, port: u16) -> ExitCode {
    match run_from_http(&args, &host, port, None).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Like `run_cli_with_http` but also receives proxy log lines for TUI display.
/// The log receiver is passed to the TUI which renders it as a virtual "api" session.
pub async fn run_cli_with_http_and_log(
    args: Vec<String>,
    host: String,
    port: u16,
    log_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
) -> ExitCode {
    match run_from_http(&args, &host, port, Some(log_rx)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_from_http(
    args: &[String],
    host: &str,
    port: u16,
    log_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
) -> anyhow::Result<()> {
    let parsed = clap::Parser::parse_from(args.iter().map(|s| s.as_str()));
    run_with_http(parsed, host, port, log_rx).await
}

async fn run_with_http(
    args: cli::CliArgs,
    host: &str,
    port: u16,
    log_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
) -> anyhow::Result<()> {
    if args.list_sessions {
        let sessions = agent::Agent::list_sessions()?;
        if sessions.is_empty() {
            println!("No saved sessions.");
        } else {
            println!(
                "{:<10} {:<6} {:<28} {:<18} {:<6} {:<16}",
                "Code", "Type", "Title", "Model", "Msgs", "Updated"
            );
            println!("{}", "-".repeat(90));
            for s in sessions.iter().take(20) {
                let updated = s.updated_at.format("%Y-%m-%d %H:%M");
                let title = s.title.clone().unwrap_or_else(|| "(untitled)".to_string());
                let title = if title.chars().count() > 26 {
                    let t: String = title.chars().take(23).collect();
                    format!("{t}...")
                } else {
                    title
                };
                let agent_type = if s.agent_type.is_empty() { "?" } else { &s.agent_type };
                println!(
                    "{:<10} {:<6} {title:<28} {:<18} {:<6} {updated}",
                    s.session_id, agent_type, s.model, s.message_count
                );
            }
        }
        return Ok(());
    }

    pigs_config::AppConfig::ensure_config_dir().map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let config = pigs_config::AppConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_env_overrides();

    let _log_guard = init_logging(&config)?;

    let mut agent = agent::Agent::new_with_http(config, args, host, port)?;
    agent.connect_configured_mcp().await?;

    if let Some(prompt) = agent.one_shot_prompt.clone() {
        let text = agent.run_turn(&prompt).await?;
        if agent.output_format.eq_ignore_ascii_case("json") {
            let payload = serde_json::json!({
                "session_id": agent.session.session_id,
                "title": agent.session.display_title(),
                "model": agent.session.model,
                "output": text,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        agent.session.save(&agent.sessions_dir)?;
        return Ok(());
    }

    // Use TUI by default (aligned with PI's full-screen interactive mode).
    // Falls back to rustyline REPL if TUI initialization fails.
    match crate::tui_repl::run_tui_repl(&mut agent, log_rx).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("TUI failed ({e}), falling back to line mode.");
            crate::repl::run_repl(&mut agent).await?;
        }
    }
    agent.session.save(&agent.sessions_dir)?;
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    let args = cli::CliArgs::parse();
    run_with_args(args, None).await
}

async fn run_from(
    args: &[String],
    injected_client: Option<Arc<dyn pigs_core::ApiClient>>,
) -> anyhow::Result<()> {
    let parsed = clap::Parser::parse_from(args.iter().map(|s| s.as_str()));
    run_with_args(parsed, injected_client).await
}

async fn run_with_args(
    args: cli::CliArgs,
    injected_client: Option<Arc<dyn pigs_core::ApiClient>>,
) -> anyhow::Result<()> {
    if args.list_sessions {
        let sessions = agent::Agent::list_sessions()?;
        if sessions.is_empty() {
            println!("No saved sessions.");
        } else {
            println!(
                "{:<10} {:<6} {:<28} {:<18} {:<6} {:<16}",
                "Code", "Type", "Title", "Model", "Msgs", "Updated"
            );
            println!("{}", "-".repeat(90));
            for s in sessions.iter().take(20) {
                let updated = s.updated_at.format("%Y-%m-%d %H:%M");
                let title = s.title.clone().unwrap_or_else(|| "(untitled)".to_string());
                let title = if title.chars().count() > 26 {
                    let t: String = title.chars().take(23).collect();
                    format!("{t}...")
                } else {
                    title
                };
                let agent_type = if s.agent_type.is_empty() { "?" } else { &s.agent_type };
                println!(
                    "{:<10} {:<6} {title:<28} {:<18} {:<6} {updated}",
                    s.session_id, agent_type, s.model, s.message_count
                );
            }
        }
        return Ok(());
    }

    pigs_config::AppConfig::ensure_config_dir().map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let config = pigs_config::AppConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_env_overrides();

    let _log_guard = init_logging(&config)?;

    let mut agent = agent::Agent::new_with_client(config, args, injected_client)?;
    agent.connect_configured_mcp().await?;

    if let Some(prompt) = agent.one_shot_prompt.clone() {
        let text = agent.run_turn(&prompt).await?;
        if agent.output_format.eq_ignore_ascii_case("json") {
            let payload = serde_json::json!({
                "session_id": agent.session.session_id,
                "title": agent.session.display_title(),
                "model": agent.api_client.model(),
                "output": text,
                "usage": {
                    "input_tokens": agent.session.total_usage.input_tokens,
                    "output_tokens": agent.session.total_usage.output_tokens,
                    "total_tokens": agent.session.total_usage.total_tokens(),
                    "cache_read_tokens": agent.session.total_usage.cache_read_tokens,
                    "est_cost_usd": agent.session.total_usage.estimate_cost_for_model(agent.api_client.model()),
                },
                "message_count": agent.session.message_count(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!("{text}");
        }
    } else {
        if agent.output_format.eq_ignore_ascii_case("json") {
            eprintln!("Note: --output json applies to one-shot mode; entering TUI as text.");
        }
        match crate::tui_repl::run_tui_repl(&mut agent, None).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("TUI failed ({e}), falling back to line mode.");
                repl::run_repl(&mut agent).await?;
            }
        }
    }

    Ok(())
}

fn init_logging(
    config: &pigs_config::AppConfig,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = match config.log_level.to_lowercase().as_str() {
            "error" => "error",
            "warn" => "warn",
            "debug" => "debug",
            "trace" => "trace",
            _ => "info",
        };
        EnvFilter::new(level)
    });

    let console_layer = fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact();

    if config.log_to_file {
        let logs_dir = pigs_config::AppConfig::logs_dir();
        std::fs::create_dir_all(&logs_dir)
            .map_err(|e| anyhow::anyhow!("Failed to create logs directory: {e}"))?;

        let file_appender = tracing_appender::rolling::daily(&logs_dir, "pigs.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let file_layer = fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(non_blocking);

        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .with(file_layer)
            .try_init();

        tracing::info!(
            logs_dir = %logs_dir.display(),
            "File logging enabled"
        );

        Ok(Some(guard))
    } else {
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .try_init();
        Ok(None)
    }
}
