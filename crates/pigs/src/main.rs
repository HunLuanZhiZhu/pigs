//! Pigs — unique product binary.
//!
//! Default (`pigs` with no args): launches **both**:
//!   - A background phased HTTP API server (127.0.0.1:3927)
//!   - The interactive pigs-cli REPL (foreground)
//!
//! The CLI is the primary user interface; the API runs as a background task
//! within the same process. Future `/api` slash command will inspect API state.
//!
//! `pigs --api` runs API-only (no REPL, pure background daemon).
//! `pigs "prompt"` runs a one-shot CLI turn (no API, no REPL).

mod server;

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

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
    /// CLI-only mode: no API server, just the REPL (foreground)
    #[arg(long)]
    cli: bool,

    /// API-only mode: start HTTP server without the REPL (background daemon)
    #[arg(long)]
    api: bool,

    /// Print runtime identity and exit
    #[arg(long)]
    describe: bool,

    /// Bind host for local API (default loopback)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Bind port for local API
    #[arg(long, default_value_t = 3927)]
    port: u16,

    /// One-shot prompt: run a single CLI turn then exit (no API, no REPL)
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// Additional args forwarded to pigs-cli (e.g. --model, --mode, --resume)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cli_args: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.describe {
        print_describe();
        return Ok(());
    }

    // --cli: CLI-only mode, no API server.
    // Don't init tracing here — pigs-cli has its own init_logging.
    if args.cli {
        let mut full_args = vec!["pigs".to_string()];
        full_args.extend(args.cli_args.iter().cloned());
        // If a prompt was given, pass it as the first positional arg.
        if let Some(prompt) = &args.prompt {
            full_args.push(prompt.clone());
        }
        let code = pigs_cli::run_cli_from(full_args).await;
        if code != std::process::ExitCode::SUCCESS {
            std::process::exit(1);
        }
        return Ok(());
    }

    // One-shot prompt: delegate to pigs-cli (full agent with tools)
    // pigs-cli handles its own tracing.
    if let Some(prompt) = args.prompt {
        let mut full_args = vec!["pigs".to_string(), prompt];
        full_args.extend(args.cli_args.iter().cloned());
        let code = pigs_cli::run_cli_from(full_args).await;
        if code != std::process::ExitCode::SUCCESS {
            std::process::exit(1);
        }
        return Ok(());
    }

    // API-only or default (API+CLI): init tracing for the API server.
    // --cli and one-shot delegate to pigs-cli which has its own logging.
    init_tracing();

    // API-only mode: just serve, no REPL
    if args.api {
        return run_api_only(&args).await;
    }

    // Default: API server (background task) + CLI REPL (foreground)
    run_api_and_cli(&args).await
}

/// Start the phased HTTP API as a background tokio task, then run the
/// pigs-cli REPL in the foreground. When the REPL exits, the API stops.
async fn run_api_and_cli(args: &Args) -> anyhow::Result<()> {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = pigs_config::AppConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_env_overrides();

    // Build the phased runtime for the API server.
    let runtime = pigs_cli::phased_runtime::PhasedRuntime::from_config(&config)?;
    let wrapped = runtime.wrapped_model.clone();
    let root = runtime.remote_model.clone();
    let host_display = args.host.clone();
    let port = args.port;
    let host = host_display.clone();

    // Spawn API server as a background task.
    let api_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(runtime, &host, port).await {
            eprintln!("[pigs API] server error: {e:#}");
        }
    });

    eprintln!("[pigs] API: http://{host_display}:{port}  (wrapped: {wrapped}, root: {root})");
    eprintln!("[pigs] CLI: full pigs-cli REPL (tools/MCP/slash commands)");
    eprintln!("[pigs] /quit to exit (API will stop too)");
    eprintln!();

    // Run pigs-cli REPL (foreground).
    let mut full_args = vec!["pigs".to_string()];
    full_args.extend(args.cli_args.iter().cloned());
    let code = pigs_cli::run_cli_from(full_args).await;

    // REPL exited — abort API server.
    api_handle.abort();
    let _ = api_handle.await;

    if code != std::process::ExitCode::SUCCESS {
        std::process::exit(1);
    }
    Ok(())
}

/// API-only mode: no REPL, just the HTTP server.
async fn run_api_only(args: &Args) -> anyhow::Result<()> {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = pigs_config::AppConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_env_overrides();

    let runtime = pigs_cli::phased_runtime::PhasedRuntime::from_config(&config)?;
    eprintln!(
        "[pigs] API-only: http://{}:{}  (wrapped: {}, root: {})",
        args.host, args.port, runtime.wrapped_model, runtime.remote_model
    );
    server::serve(runtime, &args.host, args.port).await
}

fn print_describe() {
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
