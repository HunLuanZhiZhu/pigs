from pathlib import Path
import re

# main.rs
Path('crates/pigs/src/main.rs').write_text('''//! Pigs — unique product binary.
//!
//! Default: bind a local HTTP port and expose the phased OpenAI-compatible API.
//! `pigs --cli` uses the **same api_convert + PhasedRuntime** path in-process (no port).
//!
//! Architecture:
//!   OpenAI-shaped request
//!        -> api_convert (shared)
//!        -> PhasedRuntime
//!        -> HTTP transport  OR  CLI/terminal transport

mod api_convert;
mod cli_repl;
mod markers;
mod phase;
mod prompts;
mod runtime;
mod server;
mod tools_info;

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::api_convert::{run_converted_turn, ConvertedTurn};

#[derive(Parser, Debug)]
#[command(
    name = "pigs",
    version,
    about = "Pigs phased agent: local API by default; --cli reuses the same conversion core",
    long_about = "Default mode binds 127.0.0.1 and exposes a phased agent API \
(/v1/chat/completions).\n\
`pigs --cli` opens an interactive REPL that reuses the same api_convert + runtime \
(no local port).\n\
Design: crates/pigs/docs/理解与规划.md"
)]
struct Args {
    /// Interactive REPL using shared api_convert + phased runtime (no port)
    #[arg(long)]
    cli: bool,

    /// Print runtime identity and exit
    #[arg(long)]
    describe: bool,

    /// One-shot phased turn via shared conversion (no HTTP server)
    #[arg(long)]
    once: Option<String>,

    /// Bind host for local API (default loopback)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Bind port for local API
    #[arg(long, default_value_t = 3927)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.describe {
        print_describe();
        return Ok(());
    }

    init_tracing();
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = pigs_config::AppConfig::load_layered(&workspace)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_env_overrides();

    let runtime = runtime::PhasedRuntime::from_config(&config)?;

    if args.cli {
        return cli_repl::run_phased_repl(runtime).await;
    }

    if let Some(prompt) = args.once {
        let converted = ConvertedTurn::from_user_question(prompt, Vec::new());
        let result = run_converted_turn(&runtime, &converted, None).await?;
        println!("{}", result.final_text);
        eprintln!(
            "[pigs] ended_with={} phases={} (via api_convert)",
            result.ended_with,
            result
                .events
                .iter()
                .filter(|e| e.kind == "phase_start")
                .filter_map(|e| e.phase.as_deref())
                .collect::<Vec<_>>()
                .join(",")
        );
        return Ok(());
    }

    server::serve(runtime, &args.host, args.port).await
}

fn print_describe() {
    println!("pigs {}", env!("CARGO_PKG_VERSION"));
    println!("role: unique product binary");
    println!("default: local phased HTTP API (127.0.0.1:3927)");
    println!("cli: pigs --cli  -> same api_convert + runtime, no port");
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
''', encoding='utf-8')
print('main ok')

# cli_repl
cli_path = Path('crates/pigs/src/cli_repl.rs')
cli = cli_path.read_text(encoding='utf-8')
if 'api_convert' not in cli:
    cli = cli.replace(
        'use crate::runtime::PhasedRuntime;',
        'use crate::api_convert::{run_converted_turn, ConvertedTurn};\nuse crate::runtime::PhasedRuntime;',
    )
cli = cli.replace(
    'match runtime.run_turn(&history, trimmed).await {',
    'let converted = ConvertedTurn::from_user_question(trimmed, history.clone());\n                match run_converted_turn(&runtime, &converted, None).await {',
)
cli = cli.replace(
    '"[pigs] ended_with={} phases={}"',
    '"[pigs] ended_with={} phases={} (api_convert)"',
)
cli = cli.replace(
    'Same core as local API (/v1/chat/completions). Type /help for commands.',
    'Shared core: api_convert -> PhasedRuntime (same as HTTP API). Type /help for commands.',
)
cli_path.write_text(cli, encoding='utf-8')
print('cli_repl ok', 'run_converted_turn' in cli)

# pigs-cli library only
ct_path = Path('crates/pigs-cli/Cargo.toml')
ct = ct_path.read_text(encoding='utf-8')
ct = re.sub(r'\n\[\[bin\]\]\nname = "pigs-cli"\npath = "src/main.rs"\n', '\n', ct)
ct = ct.replace(
    'description = "Pigs local REPL host (binary `pigs-cli`); shares workspace crates with `pigs`"',
    'description = "Legacy local agent modules as a library only; product binary is `pigs`"',
)
if '[lib]' not in ct:
    ct = ct.replace(
        'publish.workspace = true\n',
        'publish.workspace = true\n\n[lib]\nname = "pigs_cli"\npath = "src/lib.rs"\n',
    )
ct_path.write_text(ct, encoding='utf-8')
print('pigs-cli Cargo.toml ok')

src = Path('crates/pigs-cli/src')
mods = []
for name in [
    'agent', 'agent_tool', 'cli', 'command_aliases', 'commands', 'doctor',
    'hooks', 'i18n', 'models', 'repl', 'skill_tool', 'snapshots',
]:
    if (src / f'{name}.rs').exists():
        mods.append(f'pub mod {name};')
(src / 'lib.rs').write_text(
    '//! Legacy local agent modules (library only; product bin is `pigs`).\n\n'
    + '\n'.join(mods)
    + '\n',
    encoding='utf-8',
)
print('lib.rs mods', mods)

# keep main.rs for reference but unused — better rename to avoid accidental bin
main_legacy = src / 'main.rs'
if main_legacy.exists():
    main_legacy.rename(src / 'bin_main_legacy.rs.bak')
    print('renamed legacy main')

smoke = Path('crates/pigs-cli/tests/cli_smoke.rs')
smoke.write_text(
    '''//! pigs-cli is library-only; product smoke is on `pigs`.

#[test]
fn legacy_crate_is_library_only() {
    assert_eq!(env!("CARGO_PKG_NAME"), "pigs-cli");
}
''',
    encoding='utf-8',
)
print('smoke ok')
