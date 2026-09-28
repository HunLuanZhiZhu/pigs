# AGENTS.md

## 项目目的

**pigs** = **pig**（相位编排）+ **s**（自由子智能体）。

使用 Rust 多 crate workspace（14 个 crate）实现的多智能体系统。

- **pig**：相位编排——对单个 LLM API 请求施加 Pre→Executor→Post 三阶段结构。每个智能体（主或子）都走 pig 相位编排。
- **pigs**：pig + 自由子智能体——主智能体可在运行时创建多个子智能体协同工作。多个智能体同时存在 = pigs。

核心架构：pigs-proxy 作为前置路由层，监听单一端口（默认 3927），按 model ID 分流——无 `-pig` 后缀的请求透传上游 LLM（带重试 + body 清洗），有 `-pig` 后缀的请求走 pigs-api 相位运行时（Pre→Executor→Post）。三种 API 协议（OpenAI Chat / Anthropic Messages / OpenAI Responses）输入什么格式输出什么格式。每个配置模型自动 ×2（`{model}` + `{model}-pig`）。Agent 还具备全屏 TUI（ratatui + crossterm）、vim 模式、主题系统、模型选择器 overlay、权限系统、会话持久化（含 parent_id 会话树）、`.pigignore`、文件日志和配置热重载。

仓库中还包含多个参考 Agent 实现（作为子目录克隆），供架构借鉴。

### 子智能体设计（pigs = pig + s）

- **两种模式**：
  - **前台**：主智能体通过工具调用创建子智能体后等待结果，拿到结果继续推理
  - **后台**：主智能体创建子智能体后不等待，继续做其他事，子智能体完成后通知
- **一次创建多个**：工具调用层面支持一次性定义多个子智能体
- **完全对等**：子智能体与主智能体使用相同的工具、模型和 pig 相位编排
- **递归创建**：子智能体可以继续创建子智能体
- **上下文可选共享**：创建时由主智能体决定是否共享会话上下文
- **TUI 查看**：创建时获得 ID，显示在主对话流中；`/sub <id>` 切换查看子智能体会话，`/sub back` 返回主会话
- **后期规划**：支持预设自定义子智能体角色（如 scout/planner/reviewer），当前只内置通用型

## 语言约定

- **本项目代码语言为 Rust**，包括所有代码注释均使用英语。
- **文档语言为中文**（如 AGENTS.md、README、docs/ 下的分析文档等）。
- **例外**：`crates/pigs-mini-agent/` 为教学 crate，源码注释允许中文。
- 参考项目保留各自原始语言，不做翻译。

## 本项目代码结构

```
pigs/
├── Cargo.toml                 # workspace 根配置（14 个 crate）
├── config.toml                # API 代理配置文件（[server]/[log]/[[provider]] + language）
├── config-cli.toml           # CLI 专属配置文件（Agent 行为 + 供应商 + MCP + hooks）
├── crates/
│   ├── pigs-core/              # 核心类型 + trait（Message, ContentBlock, ToolHandler, ApiClient, StreamEvent）
│   ├── pigs-permissions/       # 权限系统（5 级权限模式 + 交互式提示器）
│   ├── pigs-config/            # 配置管理（TOML + CLAUDE.md/AGENTS.md + 环境变量 + 日志）
│   ├── pigs-session/           # 会话持久化（JSONL + 自动压缩）
│   ├── pigs-prompts/           # 相位 user payload（6 个活跃 txt + include_str! + 中英双语）
│   ├── pigs-llm/               # LLM 客户端（Anthropic + OpenAI Responses + OpenAI Chat + SSE）
│   ├── pigs-tools/             # 内置工具 + ToolRegistry + .pigignore
│   ├── pigs-mcp/               # MCP 客户端（stdio + tools/list + tools/call）
│   ├── pigs-api/               # 相位运行时（Pre→Executor→Post + 三格式 API 转换 + 标记路由）
│   ├── pigs-proxy/             # 多协议 HTTP 代理 + 重试 + 路由分流 + ProxyApiClient
│   ├── pigs-cli/               # Agent 逻辑 + TUI REPL（library，非产品 bin）
│   ├── pigs-tui/               # 终端 UI（ratatui + crossterm，全屏差异渲染）
│   ├── pigs/                   # 唯一产品二进制（pigs --api | 默认 API+TUI）
│   └── pigs-mini-agent/        # 教学用最简 Agent（自包含，不依赖 pigs-*）
├── .pigignore                 # 工具搜索忽略模式
└── docs/
    ├── agent-design.md         # 架构设计文档
    └── 参考项目分析.md           # 参考项目综合分析
```

## Crate 职责

| Crate | 作用 |
|---|---|
| `pigs-core` | 消息 / 工具 / LLM trait 与共享类型；零内部依赖 |
| `pigs-permissions` | 权限模式、策略、CLI 提示器 |
| `pigs-config` | TOML 配置、项目记忆（CLAUDE.md 优先/AGENTS.md）、Skills、Rules、Memory |
| `pigs-session` | JSONL 会话与压缩 |
| `pigs-prompts` | 相位 user payload（Pre/Executor/Post × 中英文，6 个活跃 .txt）；旧 identity prompt 文件保留但不注入 |
| `pigs-llm` | Anthropic / OpenAI Responses / OpenAI Chat 客户端与 SSE 流式 |
| `pigs-tools` | 内置 `ToolHandler` 实现与默认注册表 |
| `pigs-mcp` | MCP stdio 客户端与 tool bridge |
| `pigs-api` | 协议原生 HTTP 相位运行时（Pre→Executor→Post + PIGEND/PIGFAIL）+ 三协议 codec + 连续 JSON/SSE + 有界内存 continuation；保留 CLI 本地运行时 |
| `pigs-proxy` | 多协议 HTTP 代理 + 同渠道重试（10001 次）+ body 清洗 + 思考强度注入 + `-pig` 路由 + HTTP loopback；CLI 使用 ProxyApiClient + dispatch_in_process |
| `pigs-cli` | Agent 循环 + 斜杠命令 + MCP + Hooks + 流式回调（library，非产品 bin） |
| `pigs-tui` | 终端 UI：ratatui + crossterm，全屏差异渲染、多行编辑器 + vim 模式、Markdown 渲染、模型选择器 overlay、主题系统、图片内联显示、扩展系统 |
| `pigs` | 唯一产品二进制；默认模式（pigs-proxy 后台 + pigs-tui 前台）；`--api`（仅代理） |
| `pigs-mini-agent` | 教学用最简自包含 Agent；**禁止**被正式 crate 依赖，也**不要**依赖正式 crate |

## 构建和测试命令

```bash
cargo build              # 构建
cargo clippy             # Lint 检查
cargo test --workspace   # 运行测试
cargo run -p pigs -- --help  # 显示帮助
cargo run -p pigs        # 默认：API 代理 + TUI
cargo run -p pigs -- --api   # 仅 API 代理
```

## 架构边界

- **pigs-core** 是零内部依赖的基础 crate，定义所有核心 trait（`ApiClient`, `ToolHandler`, `StreamCallback`）。不要在 core 中添加外部依赖。
- **pigs-llm** 实现 `ApiClient` trait：Anthropic Messages、OpenAI Responses、OpenAI Chat Completions + SSE 流式。
- **pigs-prompts** 提示词模板外置为纯文本 `.txt` 文件，编译时 `include_str!` 嵌入，运行时 `.replace()` 填充变量。中英双语，默认 `zh`。
- **pigs-api** HTTP 相位运行时：完整保留方法、path/query、headers 和三协议原生 JSON，只定点修改 model、当前 user 与相位轨迹。状态机为 Pre→Executor→Post；Post `PIGEND` 完成、`PIGFAIL` 回 Pre、无标记继续 Post。工具调用由上游 Agent 执行，并通过有界内存 continuation 恢复。
- **pigs-proxy** 多协议 HTTP 代理：三协议端点（`/chat/completions` / `/v1/messages` / `/responses`）+ 同渠道重试 + body 清洗 + 思考强度注入。HTTP `-pig` 相位子请求去掉一层后通过带随机内部令牌的本机 HTTP loopback 重入代理；内部 header 在发往供应商前移除。CLI 的 `ProxyApiClient / dispatch_in_process / build_phased_runtime` 继续保留，不用于 HTTP 相位路径。
- **pigs-tools** 实现 `ToolHandler` trait，每个工具一个文件，通过 `ToolRegistry` 统一管理；`grep/glob/ls` 尊重 `.pigignore`。
- **pigs-mcp** 提供最小 MCP 客户端（stdio + Content-Length framing + initialize/tools.list/tools.call）。
- **pigs-cli** 是 Agent 逻辑库（非产品 bin）。MCP、斜杠命令、流式回调（`run_turn_with_callback`）等在此。日志 `~/.pig/logs/`。默认语言 zh。通过 `run_cli_from` 供 pigs 二进制调用。系统提示词对齐 PI（"expert coding assistant operating inside pig"），中英双语。子智能体系统（前台/后台、完全对等、递归创建、`/sub` 命令）待实现。
- **pigs-tui** 是终端 UI 库（ratatui + crossterm）。全屏差异渲染、多行编辑器 + vim 模式、Markdown 渲染（pulldown-cmark）、模型选择器 overlay（Ctrl+L）、主题系统（Ctrl+T）、图片内联显示（Kitty/iTerm2）、扩展系统（11 生命周期事件）、流式输出。
- **pigs** 是唯一产品二进制。默认模式：pigs-proxy（后台）+ pigs-tui（前台）。`--api`：仅代理。端口默认 3927。
- **pigs-mini-agent** 是教学 crate：自包含、可读优先。不要把产品级功能堆进它；不要让正式 crate 依赖它，也不要让它依赖 `pigs-core` 等正式 crate。

## 内置工具

| 工具 | 权限 | 说明 |
|---|---|---|
| `bash` | DangerFullAccess | 执行 shell 命令（带超时） |
| `read` | ReadOnly | 读取文件（带行号、范围） |
| `write` | WorkspaceWrite | 写入文件（自动创建父目录） |
| `edit` | WorkspaceWrite | 精确字符串替换 |
| `patch` | WorkspaceWrite | 应用 unified diff 补丁 |
| `grep` | ReadOnly | 正则搜索文件内容（尊重 `.pigignore`） |
| `find` | ReadOnly | 文件名模式匹配（尊重 `.pigignore`） |
| `ls` | ReadOnly | 列出目录内容（尊重 `.pigignore`） |
| `git_diff` | ReadOnly | 查看 git 变更 |
| `web_fetch` | ReadOnly | HTTP GET 抓取网页 |
| `web_search` | ReadOnly | DuckDuckGo 即时搜索/摘要 |
| `http_request` | ReadOnly | 通用 HTTP 请求（方法/headers/json/body） |
| `ask_user` | ReadOnly | 结构化用户提问 |
| `todo_write` | ReadOnly | 任务跟踪 |
| `sleep` | ReadOnly | 暂停执行 |
| `skill` | ReadOnly | 按需加载技能全文（catalog 在 system prompt 中，正文用此工具加载） |

## 重要路径与约定

- 配置：
  - API 代理：`config.toml`（工作区根，`PIG_CONFIG` 环境变量可覆盖路径）
  - CLI：`~/.pig/config-cli.toml`（全局）+ `{workspace}/.pig/config-cli.toml`（项目覆盖）+ `{workspace}/.pig/config-cli.local.toml`（本地，gitignored）
- 语言：`language = "zh"|"en"（默认 zh）`（配置/环境变量 `PIG_LANGUAGE`/`--language`）；斜杠命令中文与拼音别名始终可用（`/帮助` `/状态` `/中文` 等）
- 会话：`~/.pig/sessions/*.jsonl`
- 日志：`~/.pig/logs/pigs.log.YYYY-MM-DD`
- 并行工具：同一 assistant turn 内多个 tool_use 会并发执行
- 压缩策略：`compact_token_threshold` / `compact_keep_recent`（可用环境变量覆盖）
- 一次性 JSON：`--output json`
- 忽略文件：工作区根目录 `.pigignore`（gitignore 格式）
- 斜杠命令：`/reload` 热重载配置，`/status` 仪表盘，`/history` 查看会话摘要，`/mcp` 管理 MCP 服务器，`/skills` 查看技能，`/rules` 查看项目规则，`/export` 导出会话，`/undo` 撤销最近写操作（ 持久化快照），`/hooks` 查看 hooks，`/title` 设置会话标题，`/cost` 查看费用，`/doctor` 健康检查，`/models` 模型别名
- MCP 配置：`[[mcp_servers]]` 段，启动时自动连接 `enabled = true` 的服务器
- Skills：扫描目录（优先级从高到低，同名先到先得）`~/.pig/skills/`、`~/.agents/skills/`、`.pig/skills/`、`.agents/skills/`、`skills/`；**system prompt 只放 catalog（name+description）**，完整正文用工具 `skill` 按需加载（非全量注入）
- Rules 目录：`.pig/rules/*.md`（注入系统提示的项目规则）
- Memory：`~/.pig/memory.md` + `.pig/memory.md`（跨会话笔记，`/memory` 管理）
- Hooks 配置：`[[hooks.pre_tool_use]]` / `[[hooks.post_tool_use]]`，matcher 支持 `*` / 前缀* / 精确名

## 参考项目

| 目录 | 语言 | 说明 | 状态 |
|---|---|---|---|
| `CoreCoder/` | Python | 极简（约1k行）教学用编程 Agent，"coding agent 的 nanoGPT"，适合阅读和 fork | 已检出 |
| `claw-code/` | **Rust** | Claude Code 的 Rust 端口，9 个 crate 的工作空间；安全优先、可观测性优先。**与本项目语言相同，最重要参考** | 已检出 |
| `cline/` | TypeScript | 多面 Agent（CLI + VS Code + JetBrains + Kanban + SDK）；严格分层 SDK 架构 | 已检出 |
| `codex/` | **Rust** | OpenAI Codex CLI，约126个内部 crate；Responses API、双重构建、跨平台沙箱。**与本项目语言相同，重要参考** | 已检出 |
| `deepseek-reasonix/` | Go | Go 重写的编程 Agent（1.0，前身为 TS），面向 DeepSeek 模型；多前端架构 | 已检出 |
| `hermes-agent/` | Python | Nous Research 的自我改进 Agent；闭环学习（技能创建/改进、记忆、跨会话召回） | 已检出 |
| `kilocode/` | TypeScript | 多 IDE 编程 Agent；CLI 是 opencode 的 fork；27 个工作空间包 | 已检出 |
| `openclaw/` | TypeScript | 自托管个人 AI 助手；渠道优先（22+ 消息平台）；150+ 可插拔扩展 | 已检出 |
| `opencode/` | TypeScript | 大型开源编程 Agent monorepo（Bun + Effect + SolidJS）；约34个工作空间包 | 已检出 |
| `pi/` | TypeScript | "Pi Agent Harness" monorepo；自扩展编程 Agent CLI + Agent 运行时 + 统一多供应商 LLM API | 已检出 |
| `fugu/` | 配置/接入 | **Sakana Fugu**：多模型编排以单 model API 交付；Codex 安装/配置包 + 技术报告 | 已检出 |
| `oh-my-openagent/` | TypeScript | **OmO**：多 harness 插件式 Agent OS；Team Mode、ultrawork、OpenCode/Codex 双发行版 | 已检出 |
| `oh-my-pi/` | TS + **Rust** | **omp**：Pi fork；LSP/DAP/工具质量；Rust `crates/pi-*` 热路径 | 已检出 |
| `grok-build/` | **Rust** | **Grok Build**（xAI）：终端 AI 编程 Agent；~84 个 crate；Leader/Follower IPC + ACP 协议 + 子智能体 + `Tool` trait + `ToolStream<T>` + `nono` 沙箱。**与本项目语言相同，重要参考** | 已检出 |

所有子目录都是**独立的 git 仓库**（各有自己的 `.git/`）。它们是参考项目，不属于 pigs 的构建。

## 本项目工作方式

- 本仓库是父仓库（`D:\AIWorkSpace\pigs`）。参考项目是嵌套的独立仓库，不是 submodule。
- **不要**将参考项目目录提交到父仓库，除非明确要求。它们以 untracked 形式存在。
- `.gitignore` 目前是 Rust 导向的模板，与本项目的 Rust 语言选择一致。
- 探索参考项目时，`cd` 进入对应目录并视为独立仓库——它有自己的分支、标签和构建命令。

## Rust 参考项目重点

由于本项目语言为 Rust，以下三个参考项目最值得关注：

### `claw-code/` — Claude Code 的 Rust 端口
- Rust 工作空间位于 `rust/crates/`，9 个 crate：`rusty-claude-cli`（主二进制）、`api`（供应商客户端）、`runtime`（状态/权限/MCP）、`tools`（工具执行）、`commands`（斜杠命令）、`plugins`、`telemetry`、`claw-analog`（精简 Agent）、`claw-rag-service`（语义搜索）。
- 安全设计：工作区边界强制执行、显式限制、NDJSON 机器可读输出、`claw doctor` 健康检查、类型化权限模式。
- `PHILOSOPHY.md` 和 `concept.md` 阐述了设计理念。
- `PARITY.md` 跟踪与上游 TypeScript Claude Code 的对等差距。

### `codex/` — OpenAI Codex CLI
- Rust 工作空间位于 `codex-rs/`，约126个内部 crate（全部以 `codex-` 为前缀）。
- 核心架构：`core/`（中央 crate，130+源文件）、`tui/`（终端 UI）、`cli/`（CLI 二进制）、`app-server/`（程序化集成）、`sandboxing/`（跨平台沙箱）、`execpolicy/`（Starlark 策略引擎）。
- 双重构建系统：Cargo（开发）+ Bazel（CI/发布，带密封 LLVM 工具链）。
- `AGENTS.md`（22KB）包含详细的贡献者指南和编码规范。
- 严格的 Clippy lint：禁止 `unwrap_used`、`expect_used` 等。

### `grok-build/` — SpaceXAI Grok Build
- Rust 工作空间位于 `crates/codegen/` + `crates/common/` + `crates/build/`，约84个 workspace member（根 `Cargo.toml` 自动生成/只读）。
- 核心架构：`xai-grok-shell`（运行时 + SessionActor + Leader/Follower IPC + ACP）、`xai-grok-agent`（Agent 定义 + 构建器）、`xai-grok-pager`（TUI，ratatui + crossterm）、`xai-grok-tools`（工具实现 + registry/bridge）、`xai-tool-runtime`（统一 `Tool` trait + `ToolStream<T>` 流式输出）、`xai-grok-sampler`（三层 LLM 客户端：原始流→事件转换→actor 管理并发+重试+取消+doom-loop 检测）、`xai-grok-workspace`（文件系统 + VCS + 权限）、`xai-grok-sandbox`（`nono` Landlock/Seatbelt + per-subprocess seccomp 网络阻断）、`xai-grok-mcp`（隔离 `rmcp` 2.1 + `reqwest` 0.13）、`xai-grok-subagent-resolution`（子智能体模型/上下文解析）。
- **子智能体系统极其完善**：`SubagentCoordinator` 管理 pending/active/completed 三态；`SubagentSpawnContext` 继承 370+ 字段；上下文 fork 策略（live fork / resume-from-peer / new）；worktree 隔离。
- **Leader/Follower IPC**：单进程 leader 拥有状态并持久化，多 client（TUI/IDE/headless）通过 ACP JSON-RPC 附加/分离。
- **`/goal` 自主编排层**：goal orchestrator + planner + strategist + classifier + stop detector + summarizer + laziness classifier。
- 与 codex 互补：codex 偏沙箱/安全/模块化，grok-build 偏子智能体/工具系统/ACP 协议/TUI。

## 参考项目中的架构主题（供新 Agent 设计参考）

- **多供应商 LLM 层**：opencode、pi、hermes-agent、openclaw 均将模型访问抽象到统一接口后；grok-build 的 `xai-grok-sampler` 三层采样（原始流→事件转换→actor 管理并发+重试+doom-loop 检测）。
- **工具调用 Agent 循环**：每个参考项目都实现了某种工具调用/状态机 Agent 核心。
- **项目记忆文件**：deepseek-reasonix 用 `REASONIX.md`，hermes-agent/opencode/pi/claw-code/codex 自带 `AGENTS.md`——一种加载到系统提示词中的项目记忆文件。
- **插件/扩展架构**：opencode（34包）、pi（3包）、hermes-agent（技能系统）、openclaw（150+扩展）、codex（ext/ 子 crate 树）、grok-build（插件市场 + 进程级 `register_tool_pack`）。
- **多前端**：TUI、HTTP/SSE、桌面应用、CLI、消息平台集成；grok-build 的 Leader/Follower IPC + ACP 协议支持 TUI/IDE/headless 同时附加。
- **Fork 关系**：kilocode CLI fork 自 opencode；deepseek-reasonix 是 TS→Go 重写；claw-code 是 TS→Rust 端口。
- **沙箱与安全**：codex 的跨平台沙箱（landlock + seccomp + Windows + bubblewrap + Starlark 策略）；claw-code 的工作区边界与权限模式；grok-build 的 `nono` (Landlock/Seatbelt) + per-subprocess seccomp 网络阻断。
- **Agent 自治/协调**：claw-code 的 OmX + clawhip + OmO 多 Agent 协调系统；hermes-agent 的自我改进闭环；grok-build 的 `SubagentCoordinator` + `SubagentSpawnContext`（370+ 字段继承包）+ `/goal` 自主编排层。
- **多模型编排 / 单 model 交付**：fugu（Coordinator/Conductor + 模型池，对外 Chat/Responses）；oh-my-openagent Team Mode。
- **IDE 级工具面**：oh-my-pi 的 LSP/DAP 与 harness 调优；对照上游 pi harness。
- **工具系统设计**：grok-build 的 `Tool` trait + `ToolStream<T>` 流式输出 + `SharedResources` 类型化 map + `ToolFamily` 类型擦除。

## 设计新 Agent 前应阅读的关键文件

- `claw-code/PHILOSOPHY.md` — Rust Agent 的设计理念和安全原则。
- `claw-code/rust/crates/` — Rust Agent 工作空间的 crate 分层（与本项目语言相同）。
- `codex/AGENTS.md` — Rust 超大型项目的贡献者指南和编码规范。
- `codex/codex-rs/Cargo.toml` — 约126个 crate 的依赖组织和构建配置。
- `codex/codex-rs/core/src/` — 核心 Agent 循环的实现（130+文件）。
- `deepseek-reasonix/REASONIX.md` — Go Agent 的多前端架构笔记。
- `hermes-agent/AGENTS.md` — 自我改进 Agent 的约定。
- `opencode/AGENTS.md` 和 `opencode/CONTEXT.md` — 大型 TS Agent monorepo 的约定。
- `pi/AGENTS.md` — Agent harness 项目的约定（最接近"Agent 运行时"框架）。
- `CoreCoder/README.md` — 极简可读的 Agent 实现（约1k行），理解核心循环。
- `docs/参考项目分析.md` — 本仓库中对所有参考项目的综合分析文档。
- `fugu/README.md` + `Fugu_technical_report.pdf` — 多模型编排与单 model API 形态。
- `oh-my-openagent/AGENTS.md` / Team Mode 文档 — 多 harness 与多 Agent 团队。
- `oh-my-pi/AGENTS.md` + `crates/pi-*` — 工具质量与 Rust 热路径。
- `grok-build/README.md` — 工业级 Rust TUI Agent 的概述和仓库布局。
- `grok-build/crates/codegen/xai-grok-shell/src/session/acp_session_impl/` — SessionActor 核心状态机和 Agent 回合循环。
- `grok-build/crates/common/xai-tool-runtime/src/tool.rs` — 统一 `Tool` trait + `ToolStream<T>` 流式输出设计。
- `grok-build/crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs` — `SubagentCoordinator` 子智能体系统。
- `grok-build/crates/codegen/xai-grok-sampler/src/lib.rs` — 三层 LLM 采样架构（原始流→事件转换→actor 管理并发+重试+doom-loop 检测）。

## 参考项目的构建命令（已检出时）

每个参考项目有自己的工具链。在对应目录内运行命令：

- **CoreCoder**（Python/hatch）：`pip install -e .` 然后 `corecoder`
- **claw-code**（Rust）：`cd rust && cargo build` 或 `cargo run --bin claw`
- **cline**（Bun/TS）：`bun install` 然后 `bun run dev`
- **codex**（Rust）：`cd codex-rs && cargo build` 或用 `just codex`（需安装 just）
- **deepseek-reasonix**（Go）：`make build` 或 `go build ./cmd/...`
- **hermes-agent**（Python）：`pip install -e .` 然后 `cli.py`
- **kilocode**（Bun/TS）：`bun install` 然后 `bun run dev`
- **openclaw**（pnpm/TS）：`pnpm install` 然后查看 `package.json` 脚本
- **opencode**（Bun/TS）：`bun install` 然后 `bun run dev`
- **pi**（npm/TS）：`npm install` 然后查看 `packages/*/package.json` 脚本
- **fugu**（配置/接入）：主要是安装脚本与 Codex profile；`codex-fugu` / 见 `docs/commands_details.md`（需 Sakana API）
- **oh-my-openagent**（Bun/TS）：`bun install`；OpenCode Ultimate 与 Codex Light（lazycodex）见 README
- **oh-my-pi**（Bun + Rust）：`bun install`；Rust 侧 `cargo build`（`crates/pi-*`）；CLI 见 `omp` / README
- **grok-build**（Rust）：`cargo run -p xai-grok-pager-bin`（构建+启动 TUI）；`cargo check -p <crate>`（快速验证，总是指定单个 crate）；需 DotSlash + protoc

父仓库 `pigs` 使用 `cargo build` / `cargo test` / `cargo clippy` 构建与验证产品代码（`crates/pigs-*`）。参考项目各自独立构建。

## 当前状态

- 架构已基本确定：pigs-proxy（前置路由）+ pigs-api（相位运行时）+ pigs-cli（Agent 逻辑）+ pigs-tui（终端 UI）四层。
- CLI 已对齐 PI：工具名（read/write/edit/bash/grep/find/ls）、系统提示词（pi→pig 双语）、配置路径（~/.pig/）、22 个 PI 斜杠命令。
- TUI 已实现：ratatui + crossterm 全屏差异渲染、vim 模式、Markdown 渲染、模型选择器 overlay、主题系统、图片内联显示、扩展系统。
- 会话树已实现：parent_id + fork_from() + fork/clone/tree 命令。
- 子智能体系统尚未实现（已明确设计：前台/后台两种模式、完全对等、递归创建、上下文可选共享、/sub 命令切换查看）。
- 日期：2026-07-17。
<!-- ARIS-CODEX:BEGIN -->
## ARIS Codex Skill Scope
ARIS skills installed in this project: 80 entries.
Manifest: `.aris/installed-skills-codex.txt`
ARIS repo root: `D:\ProgramGitHub\Awesome-Auto-Research-Tools\Auto-claude-code-research-in-sleep`
Project skill path: `.agents/skills/<skill-name>`
For ARIS workflows, prefer the project-local skills under `.agents/skills/`.
Do not edit or delete junctioned skills in place; update upstream or rerun:
`powershell -NoProfile -ExecutionPolicy Bypass -File "D:\ProgramGitHub\Awesome-Auto-Research-Tools\Auto-claude-code-research-in-sleep\tools\install_aris.ps1" "D:\AIWorkSpace\pigs" -Platform codex -Reconcile`
<!-- ARIS-CODEX:END -->


## pigs 入口

```bash
cargo run -p pigs                     # 默认：API 代理 + TUI
cargo run -p pigs -- --api            # 仅 API 代理
cargo run -p pigs -- "你好"           # 一次性对话
```
