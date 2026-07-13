# pigs

Pigs 项目的唯一产品二进制 crate。统一托管两种运行时形态：相位化 Agent HTTP API（后台）与交互式 CLI REPL（前台），通过命令行参数选择运行模式。

## 核心内容

- `main()` — 异步入口，根据参数组合选择运行模式
- `Args` — clap 参数结构体（`--cli` / `--api` / `--describe` / `--host` / `--port` / `[PROMPT]` / `--` 转发）
- `run_api_and_cli()` — 默认模式：后台启动 `pigs-proxy` + 前台运行 `pigs-cli` REPL
- `run_api_only()` — `--api` 模式：仅启动相位 HTTP 服务器（后台守护进程）
- 一次性 prompt 模式：委托 `pigs-cli::run_cli_from` 执行单轮对话
- `print_describe()` — 打印运行时身份摘要

三种运行模式：
- 默认（无参数）：API（后台）+ CLI（前台）同时启动
- `--api`：仅 API，纯后台守护进程
- `"prompt"`：一次性 CLI 对话，无 API、无 REPL

## 依赖

- `pigs-cli`、`pigs-api`、`pigs-proxy`、`pigs-core`、`pigs-llm`、`pigs-config`、`pigs-session`、`pigs-tools`（均 workspace）
- `clap` / `tokio` / `anyhow` / `tracing` / `tracing-subscriber` / `reqwest` / `serde_json`

## 在 workspace 中的角色

顶层入口 — 整个 pigs 项目的唯一可执行二进制，编排 `pigs-proxy`（前置路由）与 `pigs-cli`（交互层）的启动与生命周期。
