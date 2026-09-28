# Pigs

pigs = **pig**（相位编排）+ **s**（自由子智能体）。

使用 Rust 多 crate workspace 构建的多智能体系统。

## 名字含义

| 名称 | 含义 |
|---|---|
| **pig** | 相位编排——对单个 LLM API 请求施加 Pre→Executor→Post 三阶段结构 |
| **pigs** | pig + 自由子智能体——主智能体可在运行时创建多个子智能体协同工作 |

- 单个智能体（无论主或子）都走 **pig** 相位编排
- 多个智能体同时存在并协同 = **pigs**
- 子智能体与主智能体完全对等（相同工具、相同模型、也走 pig 相位编排），且可递归创建子智能体

## 架构概览

```
客户端请求 (端口 3927)
    │
    ▼
pigs-proxy (前置路由层)
    │
    ├── model 无 -pig 后缀 → 透传到上游 LLM（重试 + body 清洗 + 思考强度注入）
    │
    └── model 有 -pig 后缀 → 转给 pigs-api 相位运行时
                                │
                                ├── PRE（规划/分流）
                                ├── Executor（信息收集/起草）
                                └── POST（审阅/验收/路由）
                                    │
                                    │ 每个相位的协议原生 HTTP 请求
                                    │ 去掉一层 -pig 后回到 pigs-proxy
                                    │ 复用渠道映射、清洗、思考强度与重试
                                    ▼
                                上游 LLM

主智能体 (pig)
    │
    ├── 创建前台子智能体（等待结果）→ /sub <id> 查看输出
    ├── 创建后台子智能体（不等待）→ 完成后通知
    └── 子智能体可递归创建子智能体
```

**核心设计**：

- **单一端口**（默认 3927）同时服务三种 API 协议
- **输入什么格式，输出什么格式**：OpenAI Chat / Anthropic Messages / OpenAI Responses
- **模型 ID ×2**：每个配置的模型对外暴露两个 ID — `{model}`（透传）+ `{model}-pig`（相位化）
- **相位请求走本机 HTTP loopback**：完整保留入口协议、路径/查询、headers 和 JSON 扩展字段，去掉一层 `-pig` 后重新进入 pigs-proxy，复用渠道映射、body 清洗、思考强度注入与重试
- **子智能体**：主智能体通过工具调用创建子智能体，支持前台（等待）和后台（不等待）两种模式，TUI 中通过 `/sub <id>` 切换查看

## 功能

### 代理层（pigs-proxy）

- **三协议支持** — `/chat/completions`（OpenAI）、`/v1/messages`（Anthropic）、`/responses`（OpenAI Responses），按路径自动区分
- **同渠道重试** — HTTP 状态码范围 + 业务错误码双重判断，最多 10001 次，含 SSE 流中途 error 检测
- **body 清洗** — 移除空 content 消息项，补全 Responses 协议缺失的 `type:"message"`
- **思考强度注入** — 按协议强制覆盖到最高档（可配置 `passthrough` 透传）
- **模型名映射** — `model_map` 支持客户端模型名 → 上游模型名转换
- **API Key 模式** — `passthrough`（透传客户端 Key）或 `override`（用配置 Key 覆盖）

### 相位运行时（pigs-api）

- **三相位流程** — Pre（规划/分流）→ Executor（执行/起草）→ Post（审阅/验收）
- **控制标记** — `PIGEND`（成功完成）/ `PIGFAIL`（执行错误，回到 Pre 重规划）/ 无标记（继续 Post）；仅最后一个有效非空行控制路由
- **协议原生请求** — clone 完整 JSON，只定点修改 model、当前用户输入和相位轨迹；system/history/tools/媒体/metadata/未知字段保持原生结构
- **外部工具 continuation** — 上游 Agent 执行原请求工具；运行时按 tool-call ID 在内存中暂停/恢复，存储受 TTL 和容量限制，不落盘
- **连续响应** — Pre、Executor、历次 Post 文本按顺序连续输出；流式末行缓冲隐藏控制标记，错误流不发送成功结束帧
- **用户 system 透传** — 调用方的 system/instructions 原样保留，相位指令只进入 user payload
- **中英双语提示词** — `language = "zh"`（默认）或 `"en"`，user payload 模板外置为纯文本文件（`pigs-prompts`）

### 子智能体（pigs = pig + s）

- **两种模式**：
  - **前台**：主智能体通过工具调用创建子智能体后等待结果，拿到结果继续推理
  - **后台**：主智能体创建子智能体后不等待，继续做其他事，子智能体完成后通知
- **一次创建多个**：工具调用层面支持一次性定义多个子智能体
- **完全对等**：子智能体与主智能体使用相同的工具、模型和 pig 相位编排
- **递归创建**：子智能体可以继续创建子智能体
- **上下文可选共享**：创建时由主智能体决定是否共享会话上下文
- **TUI 查看**：创建时获得 ID，显示在主对话流中；`/sub <id>` 切换查看子智能体会话，`/sub back` 返回主会话
- **后期规划**：支持预设自定义子智能体角色（如 scout/planner/reviewer），当前只内置通用型

### CLI / TUI（pigs-cli + pigs-tui）

- **全屏 TUI** — ratatui + crossterm，全屏差异渲染
- **多行编辑器** — Emacs 风格快捷键 + vim 模式（Ctrl+V 开关）
- **Markdown 渲染** — pulldown-cmark 解析，标题/代码块/列表/引用/行内代码
- **模型选择器** — Ctrl+L 弹出 overlay
- **主题系统** — Ctrl+T 切换 dark/light/high-contrast
- **图片内联显示** — Kitty/iTerm2 图形协议
- **扩展系统** — 11 个生命周期事件 + ExtensionRegistry
- **流式输出** — 逐 token 实时渲染
- **工具调用展示** — 实时显示工具名、参数、结果
- **状态栏** — pwd、git 分支、token 统计、模型名
- **权限系统** — 5 级权限模式（ReadOnly / WorkspaceWrite / DangerFullAccess / Ask / Allow）
- **会话持久化** — JSONL 格式存储，支持 fork/clone/tree，含 parent_id 会话树
- **上下文压缩** — 自动检测 token 超限并摘要旧消息
- **`.pigignore`** — 与 .gitignore 相同格式，grep/glob/ls 自动排除
- **MCP 客户端** — stdio JSON-RPC，支持 `tools/list` + `tools/call`
- **Skills** — 从多个目录加载技能，system 仅注入索引，全文按需加载
- **斜杠命令** — 对齐 PI 的 22 个命令：`/help`, `/model`, `/theme`, `/new`, `/fork`, `/clone`, `/tree`, `/copy`, `/compact`, `/export`, `/share`, `/settings`, `/hotkeys`, `/login`, `/logout`, `/resume`, `/reload`, `/quit` 等

## 快速开始

### 构建

```bash
cargo build --release
```

### 配置

pigs 使用**两个独立配置文件**：

- **`config.toml`** — API 代理配置（`[server]` / `[log]` / `[[provider]]` + `language`）
- **`config-cli.toml`** — CLI 专属配置（Agent 行为 + 供应商目录 + MCP + hooks）

项目根目录创建 `config.toml`（API 代理配置，首次运行自动生成默认配置）：

```toml
# ═══ pigs 顶层字段 ═══
language = "zh"                    # UI / 回复语言：zh 或 en（相位运行时用）

# ═══ 服务器 ═══
[server]
listen = "127.0.0.1:3927"          # 本地监听地址
clean_empty_content = true

# ═══ 日志 ═══
[log]
level = "info"
format = "pretty"
to_stdout = true
to_file = "logs/pigs.log"
rotate_size_mb = 50
rotate_keep = 7

# ═══ 供应商 ═══
[[provider]]
name = "AstronCodingPlan"
api_key = ""                       # passthrough 模式可留空
models = ["xopglm52", "auto"]      # 自动 ×2（每个 + -pig 版本）
max_retries = 10000
retry_on_code = [10007, 10008, 10009, 10010, 10012, 10110]
key_mode = "passthrough"           # passthrough | override

[provider.openai]                  # → /chat/completions
thinking_effort = "xhigh"
base_url = "https://maas-coding-api.cn-huabei-1.xf-yun.com/v2"

[provider.anthropic]               # → /v1/messages
thinking_effort = "max"
base_url = "https://maas-coding-api.cn-huabei-1.xf-yun.com/anthropic"

[provider.responses]               # → /responses
thinking_effort = "xhigh"
path_mode = "full"
base_url = "https://maas-coding-api.cn-huabei-1.xf-yun.com/v1/responses"
```

CLI 配置 `config-cli.toml` 三层分层加载：
1. `~/.pig/config-cli.toml`（全局用户级）
2. `{workspace}/.pig/config-cli.toml`（项目级覆盖）
3. `{workspace}/.pig/config-cli.local.toml`（机器本地，gitignored）

### 运行

```bash
# 默认：API 代理（后台）+ TUI（前台）
pig

# 仅 API 代理（无 TUI）
pig --api

# 一次性对话
pig "分析这个项目"

# 指定模型
pig --model auto-pig "你好"
```

### 对外服务端点

```
POST http://127.0.0.1:3927/chat/completions   → OpenAI Chat 协议
POST http://127.0.0.1:3927/v1/messages        → Anthropic 协议
POST http://127.0.0.1:3927/responses          → OpenAI Responses 协议
GET  http://127.0.0.1:3927/v1/models          → 模型列表（×2）
```

### 模型 ID ×2

每个配置的模型自动暴露两个 ID：

| 模型 ID | 路径 | 说明 |
|---|---|---|
| `xopglm52` | 透传 | 直接转发到上游 LLM（带重试） |
| `xopglm52-pig` | 相位化 | 走 Pre→Executor→Post 三相位 |

## 内置工具

| 工具 | 权限 | 说明 |
|---|---|---|
| `bash` | DangerFullAccess | 执行 shell 命令（带超时） |
| `read` | ReadOnly | 读取文件（带行号、范围、大小限制） |
| `write` | WorkspaceWrite | 写入文件（创建或覆盖） |
| `edit` | WorkspaceWrite | 精确字符串替换 |
| `patch` | WorkspaceWrite | 应用 unified diff 补丁（支持 dry-run） |
| `grep` | ReadOnly | 正则搜索文件内容（尊重 `.pigignore`） |
| `find` | ReadOnly | 文件名模式匹配（尊重 `.pigignore`） |
| `ls` | ReadOnly | 列出目录内容（尊重 `.pigignore`） |
| `git_diff` | ReadOnly | 查看 git unstaged/staged 变更 |
| `web_fetch` | ReadOnly | HTTP GET 抓取网页内容 |
| `web_search` | ReadOnly | DuckDuckGo 即时搜索/摘要 |
| `http_request` | ReadOnly | 通用 HTTP 请求 |
| `ask_user` | ReadOnly | 结构化用户提问 |
| `todo_write` | ReadOnly | 任务跟踪 |
| `sleep` | ReadOnly | 暂停执行 |
| `skill` | ReadOnly | 按需加载技能全文 |

## Crate 一览

### 依赖分层

```
Layer 0:  pigs-core              ← 核心类型 + trait（零内部依赖）
Layer 1:  pigs-permissions        ← 权限系统
          pigs-config             ← 配置 + AGENTS.md + 语言
          pigs-session            ← 会话持久化（含 parent_id 会话树）
          pigs-prompts            ← 提示词模板（纯文本文件 + include_str!）
Layer 2:  pigs-llm               ← LLM 客户端（OpenAI / Anthropic / DeepSeek / Ollama）
          pigs-tools              ← 内置工具 + ToolRegistry + .pigignore
          pigs-mcp                ← MCP 客户端（stdio）
Layer 3:  pigs-api               ← 相位运行时 + 三格式 API 转换
          pigs-proxy              ← 多协议 HTTP 代理 + 重试 + 路由分流
Layer 4:  pigs-cli               ← Agent 逻辑 + 斜杠命令 + MCP + Hooks（library）
          pigs-tui               ← 终端 UI（ratatui + crossterm + vim + 主题 + overlay）
          pigs                    ← 产品二进制（API 代理 + TUI）

旁路:     pigs-mini-agent         ← 教学用最简 Agent（自包含）
```

### 各 Crate 作用

| Crate | 类型 | 作用 |
|---|---|---|
| **`pigs-core`** | 库 | 核心类型：`Message` / `ContentBlock` / `ApiClient` trait / `ToolHandler` trait / `ToolRegistry`。零内部依赖。 |
| **`pigs-permissions`** | 库 | 权限系统：5 级 `PermissionMode` + `PermissionPolicy` + 交互式 `PermissionPrompter`。 |
| **`pigs-config`** | 库 | 配置管理：TOML 加载 + 环境变量覆盖 + AGENTS.md 解析 + Skills/Rules/Memory 加载 + `Language` 枚举。 |
| **`pigs-session`** | 库 | 会话持久化：JSONL 读写 + 自动压缩 + 会话元数据 + `parent_id` 会话树 + `fork_from()`。 |
| **`pigs-prompts`** | 库 | 相位 user payload：Pre / Executor / Post × 中英文，6 个活跃 `.txt` 通过 `include_str!` 编译。 |
| **`pigs-llm`** | 库 | LLM 客户端：OpenAI Responses / OpenAI Chat Completions / Anthropic Messages + SSE 流式。 |
| **`pigs-tools`** | 库 | 内置工具实现（每工具一个文件）+ 默认 `ToolRegistry` + `.pigignore`。 |
| **`pigs-mcp`** | 库 | MCP 客户端：stdio + Content-Length framing + `tools/list` + `tools/call`。 |
| **`pigs-api`** | 库 | 协议原生 HTTP 相位运行时：请求信封与三协议 codec、Pre→Executor→Post 纯状态机、PIGEND/PIGFAIL、连续 JSON/SSE 输出、有界内存 continuation。 |
| **`pigs-proxy`** | 库 | 多协议 HTTP 代理：三协议端点 + 同渠道重试 + body 清洗 + 思考强度注入 + `-pig` 路由 + `LoopbackPhaseTransport`。 |
| **`pigs-cli`** | 库 | Agent 逻辑 + 斜杠命令 + MCP + Hooks + 流式回调（`run_turn_with_callback`）。产品二进制通过 `run_cli_from` 调用。 |
| **`pigs-tui`** | 库 | 终端 UI：ratatui + crossterm 全屏差异渲染、多行编辑器 + vim 模式、Markdown 渲染、模型选择器 overlay、主题系统、图片内联显示、扩展系统。 |
| **`pigs`** | 二进制 | 唯一产品入口。默认模式：pigs-proxy（后台）+ pigs-tui（前台）。`--api`：仅代理。 |
| **`pigs-mini-agent`** | 教学库 | 最简 Agent（"Agent 的 nanoGPT"）。自包含，不依赖 pigs-* crate。 |

### 提示词模板

提示词外置为纯文本文件，方便人类查看和修改：

```
crates/pigs-prompts/prompts/
├── pre_user_zh.txt      # PRE user payload 模板（中文）
├── pre_user_en.txt      # PRE user payload 模板（英文）
├── executor_user_zh.txt
├── executor_user_en.txt
├── post_user_zh.txt
└── post_user_en.txt
```

编译时通过 `include_str!` 嵌入，运行时用 `.replace()` 填充变量。

## Workspace 结构

```
pigs/
├── Cargo.toml                 # workspace 根配置（14 个 crate）
├── config.toml                # API 代理配置文件
├── config-cli.toml           # CLI 专属配置文件
├── crates/
│   ├── pigs-core/             # 核心类型 + trait
│   ├── pigs-permissions/      # 权限系统
│   ├── pigs-config/           # 配置管理
│   ├── pigs-session/          # 会话持久化（含 parent_id 会话树）
│   ├── pigs-prompts/          # 提示词模板（纯文本 + include_str!）
│   ├── pigs-llm/              # LLM 客户端
│   ├── pigs-tools/            # 内置工具
│   ├── pigs-mcp/              # MCP 客户端
│   ├── pigs-api/              # 相位运行时 + 三格式 API 转换
│   ├── pigs-proxy/            # 多协议 HTTP 代理 + 重试 + 路由
│   ├── pigs-cli/              # Agent 逻辑 + 斜杠命令（library）
│   ├── pigs-tui/              # 终端 UI（ratatui + crossterm）
│   ├── pigs/                  # 产品二进制
│   └── pigs-mini-agent/       # 教学用最简 Agent
├── docs/
│   ├── agent-design.md        # 架构设计文档
│   └── 参考项目分析.md          # 参考项目综合分析
└── 参考项目/（独立 git，不参与 cargo workspace）
```

## 开发

```bash
# 构建全部
cargo build

# 只构建产品入口
cargo build -p pigs

# 运行测试
cargo test --workspace

# Lint 检查
cargo clippy

# 运行产品
cargo run -p pigs                    # 默认：API + TUI
cargo run -p pigs -- --api           # 仅 API
cargo run -p pigs -- "你好"          # 一次性对话
```

## 许可证

MIT
