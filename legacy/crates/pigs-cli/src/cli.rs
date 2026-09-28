//! CLI argument parsing using clap.

use clap::Parser;

/// Pigs — a general-purpose AI agent.
#[derive(Parser, Debug)]
#[command(
    name = "pigs",
    version,
    about = "A general-purpose AI agent with tool-use capabilities",
    long_about = "Pigs is a general-purpose AI agent that can interact with LLMs, execute tools, and manage conversations. Run without arguments to enter the interactive REPL."
)]
pub struct CliArgs {
    /// One-shot prompt (if provided, the agent runs this and exits instead of entering REPL)
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Model to use (overrides config and env)
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// Permission mode: readonly, workspace_write, danger, ask, allow
    #[arg(long, value_name = "MODE")]
    pub mode: Option<String>,

    /// UI / default reply language: en or zh (中文)
    #[arg(long, value_name = "LANG")]
    pub language: Option<String>,

    /// Custom system prompt (overrides the built-in language-aware default)
    #[arg(long, value_name = "TEXT")]
    pub system_prompt: Option<String>,

    /// Resume a previous session by agent code (6-char ID)
    #[arg(long, value_name = "CODE")]
    pub resume: Option<String>,

    /// Disable all tools (LLM-only mode)
    #[arg(long)]
    pub no_tools: bool,

    /// Maximum agent loop iterations per turn
    #[arg(long, value_name = "N", default_value_t = 50)]
    pub max_turns: u32,

    /// List saved sessions
    #[arg(long)]
    pub list_sessions: bool,

    /// Output format for one-shot mode: text (default) or json
    #[arg(long = "output", value_name = "FORMAT", default_value = "text")]
    pub output: String,
}
