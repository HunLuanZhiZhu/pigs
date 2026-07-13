// =============================================================================
// pigs crate — 唯一产品二进制入口
// Unique product binary entry point for the Pigs phased agent runtime.
// =============================================================================
//
// 本 crate 是整个 pigs 项目的顶层二进制 crate。它统一托管两种运行时形态：
// This crate is the top-level binary crate for the whole pigs project,
// hosting two runtime modes in one process:
//
//   1. **相位化 Agent API（HTTP 服务器）** / Phased agent API (HTTP server)
//      - 作为后台 tokio 任务运行 / Runs as a background tokio task
//      - 默认绑定 127.0.0.1:3927 / Binds 127.0.0.1:3927 by default
//      - 提供 OpenAI 兼容的 /v1/chat/completions 端点（含流式 SSE）
//        Provides OpenAI-compatible /v1/chat/completions endpoint (with SSE streaming)
//      - 相位流：pre（规划）→ executor（执行）→ post（评审+目标）
//        Phase flow: pre (plan) → executor (do) → post (review + goal)
//
//   2. **交互式 CLI REPL** / Interactive CLI REPL
//      - 完整工具链：bash、文件读写、搜索、MCP、斜杠命令
//        Full tool chain: bash, file I/O, search, MCP, slash commands
//      - 通过 pigs-cli crate 的 run_cli_from 委托实现
//        Delegated via pigs-cli crate's run_cli_from
//      - 前置参数转发：--model, --mode, --resume 等
//        Forwarded args: --model, --mode, --resume, etc.
//
// 三种运行模式 / Three run modes:
//   - 默认（无参数）：**同时启动** API（后台）+ CLI（前台）
//     Default (no args): launches **both** API (background) + CLI (foreground)
//   - --api：仅 API，纯后台守护进程 / --api: API-only, pure background daemon
//   - "prompt"：一次性 CLI 对话，无 API、无 REPL
//     "prompt": one-shot CLI turn, no API, no REPL
//
// 设计文档：crates/pigs/docs/理解与规划.md
// Design doc: crates/pigs/docs/理解与规划.md

// ---------------------------------------------------------------------------
// 模块声明 / Module declarations
// ---------------------------------------------------------------------------

/// HTTP 代理 + 相位路由服务器（从 pigs-proxy crate 导入）。
/// HTTP proxy + phased router server (imported from pigs-proxy crate).
use pigs_proxy::config::Config as ProxyConfig;

// ---------------------------------------------------------------------------
// 外部依赖 / External dependencies
// ---------------------------------------------------------------------------

use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;              // 命令行参数解析 / Command-line argument parsing
use tracing_subscriber::EnvFilter; // tracing 日志级别过滤 / Tracing log level filtering

// ---------------------------------------------------------------------------
// CLI 参数结构体 / CLI argument struct
// ---------------------------------------------------------------------------

/// pigs 命令行参数。通过 clap derive macro 解析。
/// Pigs CLI arguments, parsed via the clap derive macro.
///
/// flags: --cli, --api, --describe
/// options: --host (默认 127.0.0.1), --port (默认 3927)
/// positional: [PROMPT] (可选 / optional)
/// trailing: -- (转发到 pigs-cli 的所有剩余参数 / all remaining args forwarded to pigs-cli)
#[derive(Parser, Debug)]
#[command(
    name = "pigs",
    version,
    about = "Pigs: CLI + phased agent API in one process",
    long_about = "Default (no args): starts a background phased HTTP API on 127.0.0.1:3927 \
and opens the interactive pigs-cli REPL (tools, MCP, slash commands).\n\
`pigs --api` runs API-only (no REPL, pure background).\n\
`pigs \"prompt\"` runs a one-shot CLI turn (no API, no REPL).\n\
Design: crates/pigs/docs/理解与规划.md"
)]
struct Args {
    /// --cli 标志：仅 CLI 模式。
    /// 不启动 API 服务器，只在前台运行 pigs-cli REPL。
    /// --cli flag: CLI-only mode.
    /// No API server is started; only the pigs-cli REPL runs in the foreground.
    #[arg(long)]
    cli: bool,

    /// --api 标志：仅 API 模式。
    /// 启动相位 HTTP API 服务器作为后台守护进程，不启动 REPL。
    /// --api flag: API-only mode.
    /// Starts the phased HTTP API as a background daemon, no REPL.
    #[arg(long)]
    api: bool,

    /// --describe 标志：打印运行时身份摘要并退出。
    /// 用于快速查看版本、模式、相位流程和设计文档路径。
    /// --describe flag: print runtime identity summary and exit.
    /// Useful for quickly checking version, modes, phase flow, and doc path.
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
    /// 提供时进入一次性模式：跑一轮 CLI 对话后退出（无 API、无 REPL）。
    /// Optional one-shot prompt string.
    /// When provided, enters one-shot mode: runs a single CLI turn then exits
    /// (no API server, no REPL loop).
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// -- 之后的剩余参数，转发给 pigs-cli。
    /// 例如 --model, --mode, --resume 等。
    /// Trailing arguments after `--`, forwarded to pigs-cli.
    /// E.g. --model, --mode, --resume, etc.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cli_args: Vec<String>,
}

// ---------------------------------------------------------------------------
// 主入口 / Main entry point
// ---------------------------------------------------------------------------

/// 异步主函数。使用 tokio 运行时。
/// 根据参数组合选择运行模式（describe / CLI-only / one-shot / API-only / 默认）。
/// Async main function, running on the tokio runtime.
/// Selects the run mode based on argument combinations
/// (describe / CLI-only / one-shot / API-only / default).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 解析命令行参数 / Parse command-line arguments
    let args = Args::parse();

    // --- --describe 模式：打印摘要后立即退出 ---
    // --- --describe mode: print summary and exit immediately ---
    if args.describe {
        print_describe();
        return Ok(());
    }

    // --- --cli 模式：仅 CLI REPL，无 API 服务器 ---
    // --- --cli mode: CLI-only REPL, no API server ---
    //
    // 注意：此处不初始化 tracing——pigs-cli 有自己的 init_logging 来设置日志。
    // 如果在此处初始化，pigs-cli 再次初始化时会 panic（"global default subscriber already set"）。
    // Note: tracing is NOT initialized here — pigs-cli has its own init_logging.
    // If we initialized here, pigs-cli would panic on its second init attempt.
    if args.cli {
        // 构建转发参数：以 "pigs" 为程序名，拼接 cli_args 和可选的 prompt
        // Build forwarded args: program name "pigs", then cli_args, then optional prompt
        let mut full_args = vec!["pigs".to_string()];
        full_args.extend(args.cli_args.iter().cloned());
        // 若给出了 prompt，作为第一个位置参数传入。
        // If a prompt was given, pass it as the first positional arg.
        if let Some(prompt) = &args.prompt {
            full_args.push(prompt.clone());
        }
        // 委托给 pigs-cli 的 REPL 入口 / Delegate to pigs-cli's REPL entry point
        let code = pigs_cli::run_cli_from(full_args).await;
        // 非零退出码时传播退出状态 / Propagate non-zero exit code
        if code != std::process::ExitCode::SUCCESS {
            std::process::exit(1);
        }
        return Ok(());
    }

    // --- 一次性 prompt 模式：委托给 pigs-cli（带完整工具的 agent） ---
    // --- One-shot prompt mode: delegate to pigs-cli (full agent with tools) ---
    //
    // 当用户在命令行直接提供提示词（如 `pigs "你好"`），则只运行一轮 CLI 对话。
    // pigs-cli 自行处理 tracing 初始化。
    // When a prompt is given directly (e.g. `pigs "hello"`), run one CLI turn only.
    // pigs-cli handles its own tracing initialization.
    if let Some(prompt) = args.prompt {
        // 构建转发参数：程序名 + prompt + 额外 cli_args
        // Build forwarded args: program name + prompt + extra cli_args
        let mut full_args = vec!["pigs".to_string(), prompt];
        full_args.extend(args.cli_args.iter().cloned());
        // 委托给 pigs-cli / Delegate to pigs-cli
        let code = pigs_cli::run_cli_from(full_args).await;
        // 非零退出码时传播退出状态 / Propagate non-zero exit code
        if code != std::process::ExitCode::SUCCESS {
            std::process::exit(1);
        }
        return Ok(());
    }

    // --- --api 模式：仅 API 服务器（后台守护进程） ---
    // --- --api mode: API-only server (background daemon) ---
    //
    // 此模式下 pigs-cli 不参与，因此在此处初始化 tracing。
    // 默认（API+CLI）和 --cli 模式则让 pigs-cli 的 init_logging 处理，
    // 以避免 "global default subscriber already set" panic。
    // In this mode pigs-cli is not involved, so we init tracing here.
    // Default (API+CLI) and --cli modes let pigs-cli's init_logging handle it
    // to avoid the "global default subscriber already set" panic.
    if args.api {
        init_tracing();
        return run_api_only(&args).await;
    }

    // --- 默认模式：API 服务器（后台任务）+ CLI REPL（前台） ---
    // --- Default mode: API server (background task) + CLI REPL (foreground) ---
    run_api_and_cli(&args).await
}

// ---------------------------------------------------------------------------
// 运行模式实现 / Run mode implementations
// ---------------------------------------------------------------------------

/// 默认模式：同时运行相位 HTTP API（后台）和 pigs-cli REPL（前台）。
///
/// 流程 / Flow:
///   1. 加载分层配置（加载自工作区）/ Load layered config (from workspace)
///   2. 构建 PhasedRuntime / Build the PhasedRuntime
///   3. 以后台 tokio 任务启动 HTTP API 服务器 / Spawn HTTP API as background tokio task
///   4. 在前台运行 pigs-cli REPL / Run pigs-cli REPL in the foreground
///   5. REPL 退出时中止 API 服务器 / Abort API server when REPL exits
///
/// REPL 退出时，API 随之停止。
/// When the REPL exits, the API stops as well.
async fn run_api_and_cli(args: &Args) -> anyhow::Result<()> {
    // 加载 pigs-proxy 配置（统一配置格式 / mini-proxy 格式 + pigs 顶层字段）
    // Load pigs-proxy config (unified config format: mini-proxy format + pigs top-level fields)
    let config_path = std::env::var("PIGS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.toml"));
    let proxy_config = ProxyConfig::load(config_path.as_path())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 从 pigs-proxy 配置构建 PhasedRuntime（通过 ProxyApiClient 走 pigs-proxy 重试）
    // Build PhasedRuntime from pigs-proxy config (via ProxyApiClient through pigs-proxy retry)
    let proxy_config_arc = std::sync::Arc::new(proxy_config.clone());
    let runtime = pigs_proxy::build_phased_runtime(
        proxy_config_arc,
        "pigs",
        pigs_config::Language::from_str(&proxy_config.language).unwrap_or_default(),
        pigs_api::phased_runtime::RuntimeLimits {
            max_tokens: proxy_config.max_tokens,
            temperature: proxy_config.temperature,
            ..Default::default()
        },
    )?;

    eprintln!("[pigs] CLI: full pigs-cli REPL (tools/MCP/slash commands)");
    eprintln!("[pigs] /quit to exit (proxy will stop too)");
    eprintln!();

    // 后台启动 pigs-proxy 服务器 / Spawn pigs-proxy server in background
    let proxy_config_clone = proxy_config.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = pigs_proxy::serve(proxy_config_clone, runtime).await {
            eprintln!("[pigs proxy] server error: {e:#}");
        }
    });

    // 前台运行 pigs-cli REPL / Run pigs-cli REPL in foreground
    let mut full_args = vec!["pigs".to_string()];
    full_args.extend(args.cli_args.iter().cloned());
    let code = pigs_cli::run_cli_from(full_args).await;

    // REPL 退出 → 中止代理服务器 / REPL exited → abort proxy server
    api_handle.abort();
    let _ = api_handle.await;

    if code != std::process::ExitCode::SUCCESS {
        std::process::exit(1);
    }
    Ok(())
}

/// 仅 API 模式：无 REPL，只启动相位 HTTP 服务器作为后台守护进程。
///
/// API-only mode: no REPL, only the phased HTTP server as a background daemon.
///
/// 与 run_api_and_cli 不同的唯一地方是不启动 REPL，也不 spawn 后台任务——
/// serve() 直接阻塞当前 async 上下文直到服务器停止。
/// The only difference from run_api_and_cli is that no REPL is started and no
/// background task is spawned — serve() blocks the current async context until
/// the server stops.
async fn run_api_only(_args: &Args) -> anyhow::Result<()> {
    // 加载 pigs-proxy 配置 / Load pigs-proxy config
    let config_path = std::env::var("PIGS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.toml"));
    let proxy_config = ProxyConfig::load(config_path.as_path())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 构建 PhasedRuntime（-pig 路由用，走 pigs-proxy 重试）/ Build PhasedRuntime
    let proxy_config_arc = std::sync::Arc::new(proxy_config.clone());
    let runtime = pigs_proxy::build_phased_runtime(
        proxy_config_arc,
        "pigs",
        pigs_config::Language::from_str(&proxy_config.language).unwrap_or_default(),
        pigs_api::phased_runtime::RuntimeLimits {
            max_tokens: proxy_config.max_tokens,
            temperature: proxy_config.temperature,
            ..Default::default()
        },
    )?;

    // 直接启动代理服务器（阻塞式）/ Start proxy server (blocking)
    pigs_proxy::serve(proxy_config, runtime).await
}

// ---------------------------------------------------------------------------
// 辅助函数 / Helper functions
// ---------------------------------------------------------------------------

/// 打印运行时身份摘要到 stdout。
///
/// Print runtime identity summary to stdout.
///
/// 输出内容包含：版本、角色、运行模式选项、核心架构信息、
/// 相位流程、结束标记和设计文档路径。
/// Output includes: version, role, run mode options, core architecture info,
/// phase flow, end markers, and design doc path.
fn print_describe() {
    // env!("CARGO_PKG_VERSION") 在编译时嵌入 Cargo.toml 版本号
    // env!("CARGO_PKG_VERSION") embeds the Cargo.toml version at compile time
    println!("pigs {}", env!("CARGO_PKG_VERSION"));
    println!("role: unique product binary");
    println!("default (no args): CLI REPL (foreground) + phased API (background :3927)");
    println!("  --api            API-only (no REPL, background daemon)");
    println!("  \"prompt\"         one-shot CLI turn (no API, no REPL)");
    println!("core: api_convert -> PhasedRuntime (shared)");
    println!("phases: pre (plan) -> executor -> post (review+goal)");
    println!("markers: PIGEND | PIGFAILED | (default loop)");
    println!("phase end: no tool_calls (tool loop idle)");
    println!("docs: crates/pigs/docs/理解与规划.md");
}

/// 初始化 tracing 日志订阅者。
///
/// Initialize the tracing log subscriber.
///
/// 仅在 --api 模式下调用，因为 pigs-cli 有自己的日志初始化逻辑。
/// 使用环境变量 PIGS_LOG 或 RUST_LOG 控制级别，默认 info。
/// Only called in --api mode, since pigs-cli has its own logging init.
/// Uses env var PIGS_LOG or RUST_LOG for level control, defaults to info.
fn init_tracing() {
    // 尝试从环境变量读取日志级别过滤，失败则默认 "info"
    // Try to read log level filter from env var, default to "info" on failure
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // 初始化 fmt 订阅者（禁用 target 显示以减小冗余）
    // Initialize the fmt subscriber (disable target display to reduce noise)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false) // 不显示模块路径 / Don't show module paths
        .try_init();        // try_init 而非 init，避免重复初始化 panic
                            // try_init instead of init, avoids double-init panic
}
