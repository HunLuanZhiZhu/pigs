# pigs-cli

Pigs 完整本地 Agent / REPL 的库实现（非二进制），产品二进制 `pigs` 通过 `--cli` 模式委托调用。包含 agent、REPL、斜杠命令、MCP 连接、hooks、doctor、i18n 和会话管理等全部 CLI 逻辑。

## 核心内容

- `agent::Agent` — 核心 Agent 结构体，管理会话、工具、MCP 连接与多轮对话
- `repl` — 交互式 REPL 循环（基于 `rustyline`）
- `cli::CliArgs` — clap 命令行参数定义
- `commands` / `command_aliases` — 斜杠命令实现与别名
- `hooks` — Agent 生命周期钩子
- `doctor` — 环境诊断工具
- `i18n` — 国际化字符串
- `models` — 模型选择逻辑
- `skill_tool` — 技能工具（将 Skill 作为 ToolHandler 暴露给 Agent）
- `snapshots` — 工作区快照
- `fn run_cli()` / `fn run_cli_from(args)` — Agent 入口（REPL 或一次性对话）

## 依赖

- `pigs-core`、`pigs-llm`、`pigs-tools`、`pigs-permissions`、`pigs-session`、`pigs-config`、`pigs-mcp`、`pigs-prompts`、`pigs-api`（均 workspace）
- `clap` / `rustyline` / `tokio` / `tracing-subscriber` / `tracing-appender` / `anyhow` / `futures-util`

## 在 workspace 中的角色

交互层 — 为产品二进制 `pigs` 提供完整的 CLI Agent 能力（REPL、工具链、MCP、斜杠命令），是用户与 Agent 直接交互的前台。
