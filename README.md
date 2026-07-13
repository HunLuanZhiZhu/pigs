# Pigs

一个通用 AI Agent，使用 Rust 多 crate workspace 构建。

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
                                    │ 每个相位的 LLM 请求
                                    │ 通过进程内调用回到 pigs-proxy
                                    │ 自动享受重试
                                    ▼
                                上游 LLM
```

**核心设计**：

- **单一端口**（默认 3927）同时服务三种 API 协议
- **输入什么格式，输出什么格式**：OpenAI Chat / Anthropic Messages / OpenAI Responses
- **模型 ID ×2**：每个配置的模型对外暴露两个 ID — `{model}`（透传）+ `{model}-pig`（相位化）
- **相位运行时的 LLM 请求走 pigs-proxy 进程内调度**：三相位的每个 LLM 调用都自动享受重试（10001 次 + 业务错误码 + SSE error 检测）

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
- **控制标记** — `PIGEND`（整轮结束）/ `PIGFAILED`（路径失败，回到 Pre 重规划）/ 无标记（默认回环）
- **用户 system 透传** — 调用方的 system 消息原样传给 LLM，三相位不覆盖
- **中英双语提示词** — `language = "zh"`（默认）或 `"en"`，提示词模板外置为纯文本文件（`pigs-prompts`）
- **进程内调度** — LLM 请求通过 `dispatch_in_process` 走 pigs-proxy 重试，不走 HTTP loopback

### CLI（pigs-cli）

- **交互式 REPL** — rustyline 行编辑，SSE 流式输出
- **工具调用循环** — 自动解析 LLM 响应中的工具调用并执行（内置工具 + MCP + 子代理）
- **子代理** — `agent` 工具可委派子任务到独立上下文的只读子代理
- **权限系统** — 5 级权限模式（ReadOnly / WorkspaceWrite / DangerFullAccess / Ask / Allow）
- **会话持久化** — JSONL 格式存储，支持 `--resume` 恢复历史会话
- **上下文压缩** — 自动检测 token 超限并摘要旧消息
- **`.pigsignore`** — 与 .gitignore 相同格式，grep/glob/list_files 自动排除
- **MCP 客户端** — stdio JSON-RPC，支持 `tools/list` + `tools/call`
- **Skills** — 从多个目录加载技能，system 仅注入索引，全文按需加载
- **项目规则** — 从 `.pigs/rules/**/*.md` 注入项目级约束
- **斜杠命令** — `/help`, `/model`, `/models`, `/mode`, `/tools`, `/status`, `/cost`, `/history`, `/mcp`, `/skills`, `/rules`, `/memory`, `/export`, `/undo`, `/doctor`, `/reload`, `/quit` 等

## 快速开始

### 构建

```bash
cargo build --release
```

### 配置

项目根目录创建 `config.toml`（首次运行自动生成默认配置）：

```toml
# ═══ pigs 顶层字段 ═══
language = "zh"                    # UI / 回复语言：zh 或 en
permission_mode = "workspace_write"
max_turns = 50
max_tokens = 4096
temperature = 0.2
compact_token_threshold = 100000
compact_keep_recent = 10

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

### 运行

```bash
# 默认：API 代理（后台）+ CLI REPL（前台）
pigs

# 仅 API 代理（无 REPL）
pigs --api

# 仅 CLI（无 API 代理）
pigs --cli

# 一次性对话
pigs "分析这个项目"

# 指定模型
pigs --model auto-pig "你好"
```

### 对外服务端点

```
POST http://127.0.0.1:3927/chat/completions   → OpenAI Chat 协议
POST http://127.0.0.1:3927/v1/messages        → Anthropic 协议
POST http://127.0.0.1:3927/responses          → OpenAI Responses 协议
GET  http://127.0.0.1:3927/v1/models          → 模型列表（×2）
GET  http://127.0.0.1:3927/health             → 健康检查
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
| `http_request` | ReadOnly | 通用 HTTP 请求 |
| `ask_user` | ReadOnly | 结构化用户提问 |
| `todo_write` | ReadOnly | 任务跟踪 |
| `sleep` | ReadOnly | 暂停执行 |
| `agent` | ReadOnly | 子代理委派（独立上下文 + 只读工具） |

## Crate 一览

### 依赖分层

```
Layer 0:  pigs-core              ← 核心类型 + trait（零内部依赖）
Layer 1:  pigs-permissions        ← 权限系统
          pigs-config             ← 配置 + AGENTS.md + 语言
          pigs-session            ← 会话持久化
          pigs-prompts            ← 提示词模板（纯文本文件 + include_str!）
Layer 2:  pigs-llm               ← LLM 客户端（OpenAI / Anthropic / DeepSeek / Ollama）
          pigs-tools              ← 内置工具 + ToolRegistry + .pigsignore
          pigs-mcp                ← MCP 客户端（stdio）
Layer 3:  pigs-api               ← 相位运行时 + 三格式 API 转换
          pigs-proxy              ← 多协议 HTTP 代理 + 重试 + 路由分流
Layer 4:  pigs-cli               ← CLI REPL + Agent 循环（library）
          pigs                    ← 产品二进制（API 代理 + CLI REPL）

旁路:     pigs-mini-agent         ← 教学用最简 Agent（自包含）
```

### 各 Crate 作用

| Crate | 类型 | 作用 |
|---|---|---|
| **`pigs-core`** | 库 | 核心类型：`Message` / `ContentBlock` / `ApiClient` trait / `ToolHandler` trait / `ToolRegistry`。零内部依赖。 |
| **`pigs-permissions`** | 库 | 权限系统：5 级 `PermissionMode` + `PermissionPolicy` + 交互式 `PermissionPrompter`。 |
| **`pigs-config`** | 库 | 配置管理：TOML 加载 + 环境变量覆盖 + AGENTS.md 解析 + Skills/Rules/Memory 加载 + `Language` 枚举。 |
| **`pigs-session`** | 库 | 会话持久化：JSONL 读写 + 自动压缩 + 会话元数据。 |
| **`pigs-prompts`** | 库 | 相位提示词模板：12 个 `.txt` 纯文本文件 + `include_str!` 编译 + `Language` 中英切换 + `.replace()` 填充变量。 |
| **`pigs-llm`** | 库 | LLM 客户端：OpenAI Responses / OpenAI Chat Completions / Anthropic Messages + SSE 流式。 |
| **`pigs-tools`** | 库 | 内置工具实现（每工具一个文件）+ 默认 `ToolRegistry` + `.pigsignore`。 |
| **`pigs-mcp`** | 库 | MCP 客户端：stdio + Content-Length framing + `tools/list` + `tools/call`。 |
| **`pigs-api`** | 库 | 相位运行时（Pre→Executor→Post）+ 三格式 API 转换（OpenAI Chat / Anthropic / Responses）+ 标记路由 + 进度回调。 |
| **`pigs-proxy`** | 库 | 多协议 HTTP 代理：三协议端点 + 同渠道重试 + body 清洗 + 思考强度注入 + `-pig` 路由分流 + `ProxyApiClient`（进程内调度）+ `dispatch_in_process`。 |
| **`pigs-cli`** | 库 | CLI REPL + Agent 循环 + 斜杠命令 + MCP + Hooks + 子代理。产品二进制通过 `run_cli_from` 调用。 |
| **`pigs`** | 二进制 | 唯一产品入口。默认模式：pigs-proxy（后台）+ pigs-cli REPL（前台）。`--api`：仅代理。`--cli`：仅 REPL。 |
| **`pigs-mini-agent`** | 教学库 | 最简 Agent（"Agent 的 nanoGPT"）。自包含，不依赖 pigs-* crate。 |

### 提示词模板

提示词外置为纯文本文件，方便人类查看和修改：

```
crates/pigs-prompts/prompts/
├── pre_zh.txt           # PRE 相位指令（中文）
├── pre_en.txt           # PRE 相位指令（英文）
├── pre_user_zh.txt      # PRE user payload 模板
├── pre_user_en.txt
├── executor_zh.txt
├── executor_en.txt
├── executor_user_zh.txt
├── executor_user_en.txt
├── post_zh.txt
├── post_en.txt
├── post_user_zh.txt
└── post_user_en.txt
```

编译时通过 `include_str!` 嵌入，运行时用 `.replace()` 填充变量。

## Workspace 结构

```
pigs/
├── Cargo.toml                 # workspace 根配置（13 个 crate）
├── config.toml                # 统一配置文件
├── crates/
│   ├── pigs-core/             # 核心类型 + trait
│   ├── pigs-permissions/      # 权限系统
│   ├── pigs-config/           # 配置管理
│   ├── pigs-session/          # 会话持久化
│   ├── pigs-prompts/          # 提示词模板（纯文本 + include_str!）
│   ├── pigs-llm/              # LLM 客户端
│   ├── pigs-tools/            # 内置工具
│   ├── pigs-mcp/              # MCP 客户端
│   ├── pigs-api/              # 相位运行时 + 三格式 API 转换
│   ├── pigs-proxy/            # 多协议 HTTP 代理 + 重试 + 路由
│   ├── pigs-cli/              # CLI REPL（library）
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
cargo test

# Lint 检查
cargo clippy

# 运行产品
cargo run -p pigs                    # 默认：API + CLI
cargo run -p pigs -- --api           # 仅 API
cargo run -p pigs -- --cli           # 仅 CLI
cargo run -p pigs -- "你好"          # 一次性对话
```

## 许可证

MIT
