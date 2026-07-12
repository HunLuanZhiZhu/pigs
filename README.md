# Pigs

一个通用 AI Agent，使用 Rust 多 crate workspace 构建。

## 功能

- **交互式 REPL** — 支持 rustyline 行编辑的终端交互，SSE 流式输出
- **多供应商多模型** — 配置 `[[providers]]` / `[[models]]`；线格式：`openai`=Responses(`/v1/responses`，Codex 同款)、`openai-chat`=Chat Completions、`anthropic`=Messages；每模型可设 `context_window`
- **工具调用循环** — 自动解析 LLM 响应中的工具调用并执行（内置工具 + MCP + 子代理）
- **子代理** — `agent` 工具可委派子任务到独立上下文的只读子代理
- **权限系统** — 5 级权限模式（ReadOnly / WorkspaceWrite / DangerFullAccess / Ask / Allow）
- **会话持久化** — JSONL 格式存储，支持 `--resume` 恢复历史会话
- **上下文压缩** — 自动检测 token 超限并摘要旧消息
- **配置管理** — TOML 配置文件 + 环境变量覆盖 + 项目记忆（`CLAUDE.md` 优先，否则 `AGENTS.md`）+ `/reload` 热重载
- **`.pigsignore`** — 与 .gitignore 相同格式，grep/glob/list_files 自动排除
- **日志文件** — 默认写入 `~/.pigs/logs/pigs.log.YYYY-MM-DD`
- **MCP 客户端** — stdio JSON-RPC，支持 `tools/list` + `tools/call`，`/mcp` 命令管理
- **Skills** — 从 `~/.pigs/skills`、`~/.agents/skills`、`.pigs/skills`、`.agents/skills`、`skills/` 加载技能目录；system 仅注入索引，全文由工具 `skill` 按需加载
- **项目规则** — 从 `.pigs/rules/**/*.md` 注入项目级约束
- **状态仪表盘** — `/status` 汇总 session/model/tools/mcp/cost
- **Hooks** — PreToolUse / PostToolUse shell hooks（可按工具名匹配）
- **并行工具执行** — 同一 turn 内多个工具可并发执行
- **项目级配置** — `{workspace}/.pigs/config.toml` 覆盖全局配置
- **JSON 输出** — 一次性模式支持 `--output json`
- **写操作撤销** — 对 write/edit/apply_patch 自动快照并持久化到 `.pigs/undo/`，可用 `/undo` 回滚
- **可配置压缩** — `compact_token_threshold` / `compact_keep_recent`
- **会话导出** — `/export` 导出 markdown 会话记录
- **会话自动命名** — 取首条用户消息生成标题，可用 `/title` 修改
- **斜杠命令** — `/help`, `/model`, `/models`, `/mode`, `/tools`, `/todo`, `/status`, `/info`, `/cost`, `/title`, `/history`, `/mcp`, `/skills`, `/rules`, `/memory`, `/export`, `/undo`, `/hooks`, `/doctor`, `/init`, `/reload`, `/compact`, `/clear`, `/save`, `/sessions`, `/quit`

## 快速开始

### 构建

```bash
cargo build --release
```

### 配置

创建 `~/.pigs/config.toml`（也可在 REPL 中执行 `/init`）：

```toml
model = "claude-sonnet-4-20250514"
permission_mode = "workspace_write"
max_turns = 50
max_tokens = 4096
temperature = 1.0
log_to_file = true
log_level = "info"

[openai]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"

[anthropic]
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com"




# Multi-provider / multi-model catalog (optional)
# [[providers]]
# name = "openai"
# api = "openai"          # Responses API (default for OpenAI)
# api_key = "sk-..."
# base_url = "https://api.openai.com/v1"
#
# [[providers]]
# name = "deepseek"
# api = "openai-chat"   # third-party often only supports Chat Completions
# api_key = "sk-..."
# base_url = "https://api.deepseek.com"
#
# [[providers]]
# name = "ollama"
# api = "openai"
# api_key = "ollama"
# base_url = "http://localhost:11434/v1"
#
# [[providers]]
# name = "anthropic"
# api = "anthropic"
# api_key = "sk-ant-..."
#
# [[models]]
# name = "sonnet"
# provider = "anthropic"
# model = "claude-sonnet-4-20250514"
# context_window = 200000
#
# [[models]]
# name = "ds-chat"
# provider = "deepseek"
# model = "deepseek-chat"
# context_window = 65536
# max_tokens = 4096
#
# Optional MCP servers (stdio)
# [[mcp_servers]]
# name = "filesystem"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
# enabled = true

# Optional tool hooks
# [[hooks.pre_tool_use]]
# matcher = "bash"
# command = "echo pre-check $PIGS_TOOL_NAME"
# timeout = 10
# enabled = true
#
# [[hooks.post_tool_use]]
# matcher = "*"
# command = "echo done $PIGS_TOOL_NAME is_error=$PIGS_TOOL_IS_ERROR"
# enabled = true
```

或使用环境变量：

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export PIGS_MODEL="sonnet"  # 别名: opus, sonnet, haiku, gpt-4, gpt-4o, gpt-4o-mini
export PIGS_LOG_LEVEL="info"
export PIGS_LOG_TO_FILE="true"
```

### 使用

```bash
# 交互式 REPL
pigs

# 一次性执行
pigs "解释这个项目的架构"

# 指定模型和权限
pigs --model gpt-4o --mode readonly "读取 README.md"

# 第三方 OpenAI 兼容端点：配置 [openai].base_url + api_key，再指定对方 model id
# pigs --model my-local-model "..."

# 一次性 JSON 输出（脚本集成）
pigs --output json "用三句话总结本仓库"

# 恢复之前的会话
pigs --resume <session-id>

# 列出所有保存的会话
pigs --list-sessions

# 禁用工具（纯聊天模式）
pigs --no-tools "写一首关于编程的诗"

# 自定义系统提示词
pigs --system-prompt "你是一个 Python 专家" "优化这段代码"
```

## 内置工具

| 工具 | 权限 | 说明 |
|---|---|---|
| `bash` | DangerFullAccess | 执行 shell 命令（带超时） |
| `read_file` | ReadOnly | 读取文件（带行号、范围、大小限制） |
| `write_file` | WorkspaceWrite | 写入文件（创建或覆盖） |
| `edit_file` | WorkspaceWrite | 精确字符串替换 |
| `apply_patch` | WorkspaceWrite | 应用 unified diff 补丁（支持 dry-run） |
| `grep_search` | ReadOnly | 正则搜索文件内容（尊重 `.pigsignore`） |
| `glob_search` | ReadOnly | 文件名模式匹配（尊重 `.pigsignore`） |
| `list_files` | ReadOnly | 列出目录内容（尊重 `.pigsignore`） |
| `git_diff` | ReadOnly | 查看 git unstaged/staged 变更 |
| `web_fetch` | ReadOnly | HTTP GET 抓取网页内容 |
| `web_search` | ReadOnly | DuckDuckGo 即时搜索/摘要 |
| `http_request` | ReadOnly | 通用 HTTP 请求（GET/POST/PUT/PATCH/DELETE/HEAD，支持 headers/json/body） |
| `ask_user` | ReadOnly | 结构化用户提问 |
| `todo_write` | ReadOnly | 任务跟踪 |
| `sleep` | ReadOnly | 暂停执行 |
| `agent` | ReadOnly | 子代理委派（独立上下文 + 只读工具） |

## Crate 一览

本仓库是 **一个产品运行时 + 一个教学 crate** 的 monorepo。  
日常使用入口是 **`pigs-cli`（二进制 `pigs`）**；相位运行时是 **`pigs`（二进制 pigs）**；想理解 Agent 最小循环时读 **`pigs-mini-agent`**。

### 依赖分层

```
Layer 0:  pigs-core
Layer 1:  pigs-permissions, pigs-config, pigs-session
Layer 2:  pigs-llm, pigs-tools, pigs-mcp
Layer 3:  pigs              ← 相位运行时（二进制 pigs）
          pigs-cli          ← 本地 REPL（library（非产品 bin））
          pigs              ← 相位运行时（二进制 pigs）

旁路（自包含，不依赖 pigs-*，也不被 pigs-* 依赖）:
          pigs-mini-agent   ← 教学用最简 Agent
```

### 各 Crate 作用

| Crate | 类型 | 作用 |
|---|---|---|
| **`pigs-core`** | 库 | 零内部依赖的基础层。定义消息模型（`Message` / `ContentBlock`）、工具抽象（`ToolHandler` / `ToolRegistry` / `ToolSpec`）、LLM 抽象（`ApiClient` / `ApiRequest` / `StreamEvent` / `StreamCallback`）、以及 `TokenUsage`。其他正式 crate 都依赖它。 |
| **`pigs-permissions`** | 库 | 权限系统。`PermissionMode`（有序：ReadOnly &lt; WorkspaceWrite &lt; DangerFullAccess，外加 Ask / Allow）、`PermissionPolicy`、交互式 `PermissionPrompter`。决定工具在何种模式下可执行。 |
| **`pigs-config`** | 库 | 配置与提示词素材。加载全局/项目 TOML、环境变量覆盖；解析项目记忆（`CLAUDE.md` 优先 / `AGENTS.md`）；加载 Skills、项目 Rules、跨会话 Memory；组装系统提示词相关片段。 |
| **`pigs-session`** | 库 | 会话持久化。JSONL 读写、会话元数据与前缀匹配、自动标题、上下文压缩（`compact`）。会话文件默认在 `~/.pigs/sessions/`。 |
| **`pigs-llm`** | 库 | API 格式：Anthropic Messages、OpenAI Responses（默认 `/v1/responses`）、OpenAI Chat Completions（`openai-chat`）。 |
| **`pigs-tools`** | 库 | 内置工具实现（每个工具一个模块）+ 默认 `ToolRegistry` 装配 + `.pigsignore`。不包含需要 `ApiClient` 的子代理工具（子代理在 CLI 层）。 |
| **`pigs-mcp`** | 库 | 最小 MCP 客户端：stdio + Content-Length framing + `initialize` / `tools/list` / `tools/call`。通过 `McpToolHandler` 把远端工具桥成 `ToolHandler`。 |
| **`pigs-cli`** | 二进制 | 本地 REPL 宿主（二进制名 `pigs-cli`）。 |
| **`pigs`** | 二进制 | 相位化 Agent 运行时（Plan→执行者→Review+Goal）。设计：`crates/pigs/docs/理解与规划.md`。 |
| **`pigs-mini-agent`** | 教学库 | **教学用最简 Agent**（“Agent 的 nanoGPT”）。自包含：消息 / 工具 trait / 非流式 OpenAI 兼容 LLM / 4 个工具 / Agent 循环；附带章节文档与 `examples/chat`。**不依赖** `pigs-core` 等正式 crate，正式 crate 也**不得依赖**它。注释以中文为主（教学例外）。 |

### 边界约定

- 正式产品能力只加在 Layer 0–3；不要把“完整 Agent 功能”堆进 `pigs-mini-agent`。
- `pigs-mini-agent` 保持可读、自包含；与产品栈对照阅读，而不是替换 `pigs`。
- 参考项目目录（`CoreCoder/`、`claw-code/`、`codex/`、`fugu/`、`oh-my-openagent/`、`oh-my-pi/` 等）是独立嵌套仓库，**不是** workspace members。详见 `docs/参考项目分析.md`。

## Workspace 结构

```
pigs/
├── Cargo.toml                 # workspace 根配置
├── crates/                    # pigs 产品代码（见上文 Crate 一览）
├── skills/                    # 可选技能目录
├── .pigsignore
├── docs/
│   ├── agent-design.md        # 架构设计文档
│   └── 参考项目分析.md          # 参考项目综合分析（13 个）
└── 参考项目/（独立 git，不参与 cargo workspace）
    ├── CoreCoder/ claw-code/ cline/ codex/
    ├── deepseek-reasonix/ hermes-agent/ kilocode/
    ├── openclaw/ opencode/ pi/
    ├── fugu/                  # Sakana 多模型编排 · Codex 接入
    ├── oh-my-openagent/       # 多 harness 插件式 Agent OS
    └── oh-my-pi/              # Pi fork · TS+Rust · IDE 级工具
```

### 教学 crate 用法

```bash
# 运行教学示例（需 OPENAI_API_KEY 或兼容端点）
cargo run -p pigs-mini-agent --example chat

# 阅读教学文档
# crates/pigs-mini-agent/README.md
# crates/pigs-mini-agent/docs/html/index.html
```


## 参考项目

仓库中还克隆了多个独立 Agent 实现，供架构借鉴（**不参与** `cargo` 构建）。完整分析见 [`docs/参考项目分析.md`](docs/参考项目分析.md)。

| 目录 | 语言 | 一句话 |
|---|---|---|
| `CoreCoder/` | Python | 极简教学 Agent（“coding agent 的 nanoGPT”） |
| `claw-code/` | **Rust** | Claude Code 的 Rust 端口；crate 分层与安全设计 |
| `cline/` | TypeScript | 多面 Agent + 分层 SDK |
| `codex/` | **Rust** | OpenAI Codex CLI；Responses API 与沙箱 |
| `deepseek-reasonix/` | Go | DeepSeek 向编程 Agent |
| `hermes-agent/` | Python | 自我改进 / 技能闭环 |
| `kilocode/` | TypeScript | 多 IDE；CLI fork 自 opencode |
| `openclaw/` | TypeScript | 自托管助手；渠道与扩展 |
| `opencode/` | TypeScript | 大型开源编程 Agent monorepo |
| `pi/` | TypeScript | Agent harness 框架 |
| `fugu/` | 配置/接入 | **Sakana Fugu**：多模型编排以单 model API 交付；Codex 安装包 |
| `oh-my-openagent/` | TypeScript | **OmO**：多 harness 插件、Team Mode、ultrawork |
| `oh-my-pi/` | TS + **Rust** | **omp**：Pi fork，LSP/DAP/工具质量与 Rust 热路径 |

对 pigs 当前方向特别相关：

- **fugu** — 专家团/协调器 +「一个 model 对外」的产品形态
- **oh-my-openagent** — Team Mode / hooks / 多 harness 适配
- **oh-my-pi** — 工具与 harness 质量、Rust 本地核心
- **codex / claw-code** — Rust 实现与 OpenAI Responses 线格式

## 开发

```bash
# 构建全部 members（含教学 crate）
cargo build

# 只构建产品入口
cargo build -p pigs
cargo build -p pigs

# 运行测试
cargo test

# 仅测试教学 crate
cargo test -p pigs-mini-agent

# Lint 检查
cargo clippy

# 运行产品 CLI
cargo run -p pigs -- --cli
# 或
cargo run -- --help
```

## 许可证

MIT


## pigs 入口

```bash
cargo run -p pigs                 # 本地 API :3927
cargo run -p pigs -- --cli --     # REPL（转发 pigs-cli）
cargo run -p pigs -- --once "你好"
```
