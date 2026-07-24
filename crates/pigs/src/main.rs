// =============================================================================
// pigs crate — 唯一产品二进制入口
// Unique product binary entry point for the Pigs phased agent runtime.
// =============================================================================
//
// 本 crate 是整个 pigs 项目的顶层二进制 crate。
// This crate is the top-level binary crate for the whole pigs project.
//
// 运行模式 / Run modes:
//   - 默认（无参数）：启动相位 HTTP API（后台）+ CLI REPL（前台，走 HTTP 端口）
//     Default (no args): starts phased HTTP API (background) + CLI REPL (foreground, over HTTP)
//   - --api：仅 API，纯后台守护进程 / --api: API-only, pure background daemon
//   - "prompt"：启动 API + 一次性 CLI 对话（走 HTTP 端口）
//     "prompt": starts API + one-shot CLI turn (over HTTP)
//
// CLI 始终通过 HTTP 端口与 API 服务器交互，获得真流式 SSE 输出。
// The CLI always connects to the API server over HTTP, getting real SSE streaming.
//
// 设计文档：crates/pigs/docs/理解与规划.md
// Design doc: crates/pigs/docs/理解与规划.md

use std::path::PathBuf;

use clap::Parser;

use pigs_proxy::config::Config as ProxyConfig;

/// pigs 命令行参数。
/// Pigs CLI arguments, parsed via the clap derive macro.
///
/// flags: --api, --describe
/// options: --host (默认 127.0.0.1), --port (默认 3927)
/// positional: [PROMPT] (可选 / optional)
/// trailing: -- (转发到 pigs-cli 的所有剩余参数 / all remaining args forwarded to pigs-cli)
#[derive(Parser, Debug)]
#[command(
    name = "pigs",
    version,
    about = "Pigs: CLI + phased agent API in one process",
    long_about = "Default (no args): starts a background phased HTTP API on 127.0.0.1:3927 \
and opens the interactive pigs-cli REPL connected via HTTP.\n\
`pigs --api` runs API-only (no REPL, pure background).\n\
`pigs \"prompt\"` starts API + runs a one-shot CLI turn over HTTP.\n\
Design: crates/pigs/docs/理解与规划.md"
)]
struct Args {
    /// --api 标志：仅 API 模式。
    /// 启动相位 HTTP API 服务器作为后台守护进程，不启动 CLI。
    /// --api flag: API-only mode.
    /// Starts the phased HTTP API as a background daemon, no CLI.
    #[arg(long)]
    api: bool,

    /// --describe 标志：打印运行时身份摘要并退出。
    /// --describe flag: print runtime identity summary and exit.
    #[arg(long)]
    describe: bool,

    /// --host：本地 API 绑定地址（默认 loopback 127.0.0.1）。
    /// --host: local API bind address (default loopback 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// --port：本地 API 绑定端口（默认 3927）。
    /// --port: local API bind port (default 3927).
    #[arg(long, default_value_t = 3927)]
    port: u16,

    /// 可选的一次性提示词。
    /// 提供时进入一次性模式：启动 API + 跑一轮 CLI 对话后退出。
    /// Optional one-shot prompt string.
    /// When provided, starts API + runs a single CLI turn then exits.
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// -- 之后的剩余参数，转发给 pigs-cli。
    /// Trailing arguments after `--`, forwarded to pigs-cli.
    /// E.g. --model, --mode, --resume, etc.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cli_args: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // --- --describe 模式 ---
    if args.describe {
        print_describe();
        return Ok(());
    }

    // --- --api 模式：仅 API 服务器（后台守护进程） ---
    if args.api {
        return run_api_only(&args).await;
    }

    // --- 默认 / 一次性 prompt 模式：API（后台）+ CLI（前台，走 HTTP） ---
    run_api_and_cli(&args).await
}

/// 启动 API 服务器（后台）+ CLI REPL（前台，通过 HTTP 连接）。
///
/// Starts the API server (background) + CLI REPL (foreground, over HTTP).
async fn run_api_and_cli(args: &Args) -> anyhow::Result<()> {
    // 加载 pigs-proxy 配置（分层：~/.pigs/config.toml → .pigs/config.toml → .pigs/config.local.toml）
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let proxy_config = ProxyConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 普通模式：proxy 日志不输出到终端，通过 channel 桥接到 TUI（可在 /ses api 中查看）
    let (log_tx, log_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let _ = pigs_proxy::log::init_with_bridge(&proxy_config.log, log_tx);

    let host = args.host.clone();
    let port = args.port;

    eprintln!("[pigs] API: http://{host}:{port} (background)");
    eprintln!("[pigs] CLI: pigs-cli REPL over HTTP (foreground)");
    eprintln!("[pigs] /quit to exit, /ses api to view API logs");
    eprintln!();

    // 后台启动 API 服务器
    let api_handle = tokio::spawn(async move {
        if let Err(e) = pigs_proxy::serve(proxy_config).await {
            eprintln!("[pigs proxy] server error: {e:#}");
        }
    });

    // 构建 CLI 转发参数
    let mut full_args = vec!["pigs".to_string()];
    full_args.extend(args.cli_args.iter().cloned());
    if let Some(prompt) = &args.prompt {
        full_args.push(prompt.clone());
    }

    // 前台运行 pigs-cli，通过 HTTP 连接 API 服务器（并接收 proxy 日志）
    let code = pigs_cli::run_cli_with_http_and_log(full_args, host, port, log_rx).await;

    // CLI 退出 → 中止 API 服务器
    api_handle.abort();
    let _ = api_handle.await;

    if code != std::process::ExitCode::SUCCESS {
        std::process::exit(1);
    }
    Ok(())
}

/// 仅 API 模式：无 CLI，只启动相位 HTTP 服务器。
async fn run_api_only(args: &Args) -> anyhow::Result<()> {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let proxy_config = ProxyConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    pigs_proxy::log::init(&proxy_config.log).map_err(|e| anyhow::anyhow!(e.to_string()))?;

    eprintln!("[pigs] API-only: http://{}:{}", args.host, args.port);
    eprintln!();

    pigs_proxy::serve(proxy_config).await
}

fn print_describe() {
    println!("pigs {}", env!("CARGO_PKG_VERSION"));
    println!("role: unique product binary");
    println!("default (no args): CLI REPL (foreground, over HTTP) + phased API (background :3927)");
    println!("  --api            API-only (no REPL, background daemon)");
    println!("  \"prompt\"         start API + one-shot CLI turn over HTTP");
    println!("core: protocol-native HttpPhasedRuntime + HTTP loopback transport");
    println!("phases: pre -> executor -> post; markerless post -> post");
    println!("markers: PIGEND | PIGFAIL (final non-empty line only)");
    println!("tools: upstream agent execution + bounded in-memory continuation");
    println!("cli: connects to local API via HTTP, real SSE streaming");
    println!("docs: crates/pigs/docs/理解与规划.md");
}
