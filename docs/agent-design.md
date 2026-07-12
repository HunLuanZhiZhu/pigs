# Pigs Agent 架构设计

> 本文档描述 Pigs Agent 的完整架构设计，包括 crate 分层、核心数据类型、Agent 循环算法、权限模型、LLM 供应商抽象和工具系统。
>
> 日期：2026-07-11

---

## 1. 总览

Pigs Agent 是一个通用 AI Agent，使用 Rust 多 crate workspace 结构实现。Agent 具备：
- 交互式 REPL（支持 SSE 流式输出）
- 双 API 格式（Anthropic Messages + OpenAI Chat Completions），SSE 流式解析；兼容端点经 openai.base_url 接入
- 工具调用循环（16 个内置工具，含子代理 / git_diff / apply_patch / http_request）
- 权限系统（5 级权限模式 + 交互式提示器）
- 会话持久化（JSONL + 自动压缩）
- 配置管理（TOML + 环境变量 + AGENTS.md 项目记忆 + `/reload` 热重载）
- 任务跟踪（TodoWrite 工具 + `/todo` 命令）
- 结构化用户提问（AskUser 工具）
- `.pigsignore` 搜索忽略（grep/glob/list_files）
- 文件日志输出（`~/.pigs/logs/`）
- MCP 客户端（stdio JSON-RPC，`tools/list` + `tools/call`，`/mcp` 管理）

### 设计原则

- **安全优先**：`unsafe_code = "forbid"`，禁止 `unwrap()`/`expect()`，强制错误处理
- **模块化**：每个 crate 单一职责，依赖单向流动
- **依赖反转**：core 定义 trait，llm/tools 实现 trait，cli 编排一切
- **工作区边界**：文件操作限制在 workspace root 内；搜索工具尊重 `.pigsignore`
- **可观测性**：tracing 控制台日志 + 按日滚动文件日志、用量追踪

## 2. Crate 依赖图

产品运行时（正式 Agent）分层：

```
Layer 0:  pigs-core（核心类型 + trait，零内部依赖）
Layer 1:  pigs-permissions, pigs-config, pigs-session（依赖 core）
Layer 2:  pigs-llm, pigs-tools, pigs-mcp
          - pigs-llm / pigs-mcp 依赖 core
          - pigs-tools 依赖 core + permissions
Layer 3:  pigs（相位运行时，二进制 pigs）+ pigs-cli（本地 REPL，二进制 pigs-cli）
```

旁路教学 crate（**不**进入产品依赖链）：

```
pigs-mini-agent  — 自包含最简 Agent（消息 / 工具 / 非流式 LLM / 循环）
                   不依赖 pigs-*，正式 crate 也不得依赖它
```

依赖关系（产品栈）：

```
pigs-core ←─── pigs-permissions
           ←─── pigs-config
           ←─── pigs-session
           ←─── pigs-llm  ←──┐
           ←─── pigs-mcp  ←──┤
           ←─── pigs-tools ←┤
                             ├── pigs-cli
                             ├── pigs (runtime lib)
pigs-permissions ←─── pigs-tools
pigs-permissions ←─── pigs-cli
pigs-config      ←─── pigs-cli
pigs-session     ←─── pigs-cli
```

| Crate | 职责摘要 |
|---|---|
| `pigs-core` | `Message` / `ContentBlock` / `ToolHandler` / `ApiClient` / `StreamEvent` / `TokenUsage` |
| `pigs-permissions` | `PermissionMode`、策略、CLI 提示器 |
| `pigs-config` | TOML 配置、AGENTS.md、Skills、Rules、Memory |
| `pigs-session` | JSONL 会话、标题、压缩 |
| `pigs-llm` | Anthropic Messages + OpenAI Chat Completions SSE；无内置第三方供应商 |
| `pigs-tools` | 内置工具实现 + 默认注册表 + `.pigsignore` |
| `pigs-mcp` | MCP stdio 客户端 + `McpToolHandler` 桥接 |
| `pigs-cli` | 本地 REPL（二进制 `pigs-cli`） |
| `pigs` | 相位运行时（二进制 `pigs`）；见 crates/pigs/docs/理解与规划.md |
| `pigs-mini-agent` | 教学用最简实现（对照阅读，非产品入口） |

## 3. Workspace 结构

```
pigs/
├── Cargo.toml                 # workspace 根（9 个 members）
├── crates/
│   ├── pigs-core/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── message.rs      # Message, MessageRole, ContentBlock
│   │       ├── tool.rs         # ToolSpec, ToolHandler, ToolRegistry
│   │       ├── api.rs          # ApiClient, ApiRequest/Response, StreamEvent
│   │       └── usage.rs        # TokenUsage
│   ├── pigs-permissions/
│   │   └── src/
│   │       ├── mode.rs / policy.rs / prompter.rs
│   ├── pigs-config/
│   │   └── src/
│   │       ├── config.rs       # TOML + 分层加载 + hooks/mcp 段
│   │       ├── agents_md.rs
│   │       ├── skills.rs / rules.rs / memory.rs
│   ├── pigs-session/
│   │   └── src/
│   │       ├── session.rs / compact.rs
│   ├── pigs-llm/
│   │   └── src/
│   │       ├── openai.rs / anthropic.rs / provider.rs
│   ├── pigs-tools/
│   │   └── src/
│   │       ├── lib.rs          # 默认注册表 + tool_permission_modes
│   │       ├── bash.rs, read_file.rs, write_file.rs, edit_file.rs
│   │       ├── apply_patch.rs, grep.rs, glob.rs, list_files.rs
│   │       ├── git_diff.rs, web_fetch.rs, web_search.rs, http_request.rs
│   │       ├── ask_user.rs, todo_write.rs, sleep.rs, ignore.rs
│   ├── pigs-mcp/
│   │   └── src/
│   │       ├── client.rs / protocol.rs / tool_bridge.rs / error.rs
│   ├── pigs-cli/               # 本地 REPL + 二进制 pigs-cli
│   ├── pigs/                   # 相位运行时 + 二进制 pigs
│   │   └── src/
│   │       ├── main.rs / cli.rs / repl.rs / agent.rs / commands.rs
│   │       ├── agent_tool.rs / doctor.rs / hooks.rs / snapshots.rs / models.rs
│   └── pigs-mini-agent/        # 教学 crate（自包含）
│       ├── src/                # agent / llm / message / tool / tools/*
│       ├── examples/chat.rs
│       └── docs/               # 章节 MD + HTML 导读
├── skills/                     # 可选技能目录
├── .pigsignore
└── docs/
    ├── agent-design.md
    └── 参考项目分析.md
```

## 4. 核心数据类型（pigs-core）

### 4.1 消息模型

```rust
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, output: String, is_error: bool },
}

pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
}
```

### 4.2 工具模型

```rust
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub required_permission: PermissionMode,
}

pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

// Object-safe trait for dynamic dispatch
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>>;
}
```

### 4.3 API 抽象

```rust
pub struct ApiRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

pub struct ApiResponse {
    pub content: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
    pub model: String,
}

pub trait ApiClient: Send + Sync {
    fn send_message<'a>(
        &'a self,
        request: ApiRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResponse, ApiError>> + Send + 'a>>;
    fn model(&self) -> &str;
}
```

### 4.4 用量追踪

```rust
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost: Option<f64>,
}
```

## 5. Agent 循环算法

```
fn run_turn(user_input):
    1. session.messages.push(Message::user(user_input))

    2. loop (max_iterations):
        a. build ApiRequest from session.messages + system_prompt + tool definitions
        b. response = api_client.send_message(request)
        c. session.messages.push(Message::assistant(response.content))
        d. update usage tracker

        e. tool_uses = extract_tool_uses(response.content)

        f. if tool_uses.is_empty():
              break  // terminal turn, display assistant text

        g. for each tool_use in tool_uses:
              - permission_result = policy.check(tool_use.name, tool_use.input)
              - match permission_result:
                  Allow => execute tool
                  Deny => tool_result = "Permission denied"
                  Ask => prompt user, then Allow/Deny
              - session.messages.push(Message::tool_result(tool_use_id, result))

    3. save session
    4. return TurnSummary
```

## 6. 权限模型

### 6.1 权限模式（有序）

```rust
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionMode {
    ReadOnly,         // 只读工具
    WorkspaceWrite,   // workspace 内写入
    DangerFullAccess, // 完全访问
}
```

外加特殊模式：
- `Ask` — 每次工具执行前询问用户
- `Allow` — 允许一切（跳过检查）

### 6.2 权限策略

```rust
pub struct PermissionPolicy {
    pub active_mode: PermissionMode,
    pub tool_requirements: HashMap<String, PermissionMode>,
    pub denied_tools: Vec<String>,
}
```

### 6.3 权限检查流程

```
fn check(tool_name, tool_input):
    1. if denied_tools.contains(tool_name): return Deny
    2. required = tool_requirements.get(tool_name) or default
    3. if active_mode == Allow: return Allow
    4. if active_mode == Ask: return Ask
    5. if active_mode >= required: return Allow
    6. return Ask  // 需要升级权限
```

### 6.4 权限提示器

```rust
pub trait PermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionDecision;
}

pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
}
```

## 7. LLM 供应商抽象

### 7.1 供应商路由

```rust
pub enum Provider {
    OpenAI,
    Anthropic,
}

pub fn detect_provider(model: &str) -> Provider {
    // claude* -> Anthropic
    // gpt*/o1*/o3*/o4* -> OpenAI
            // else -> OpenAI Chat Completions
}
```

### 7.2 模型别名

```text
opus/sonnet/haiku -> Claude 系列
gpt-4/gpt-4o/gpt-4o-mini -> OpenAI 系列
其它 model id -> OpenAI Chat Completions（可改 base_url）
```

### 7.3 OpenAI Chat Completions 客户端

- 端点：`POST {base_url}/chat/completions`
- 认证：`Authorization: Bearer {api_key}`
- 默认 base URL：
  - OpenAI: `https://api.openai.com/v1`
  - 兼容端点：配置 `openai.base_url`（例如本地或第三方代理）
- SSE 流式：`data: {json}\n\n`，`data: [DONE]`

### 7.4 Anthropic 客户端

- 端点：`POST {base_url}/v1/messages`
- 认证：`x-api-key: {api_key}` + `anthropic-version: 2023-06-01`
- 请求体：`{ model, system, messages, tools, max_tokens, stream: true }`
- SSE 流式：`event: {type}\ndata: {json}\n\n`
- 消息格式：`content: [{ type: "text"|"tool_use", ... }]`


## 7.6 协议实现参考绑定

每种线格式在实现/加固时固定对照 1–3 个参考项目，避免“凭空猜协议”：

| 协议 | pigs 配置 `api` | 主要参考 |
|---|---|---|
| **Anthropic Messages** | `anthropic` | 1) `claw-code/rust/crates/api`（SSE framing、tool_result、types） 2) `pi/packages/ai/src/api/anthropic-messages.ts`（headers/事件） |
| **OpenAI Chat Completions** | `openai-chat` | 1) `claw-code/.../openai_compat.rs`（system 首条 message、orphan tool 清理、`stream_options`、`max_completion_tokens`） 2) `pi/.../openai-completions.ts`（兼容字段） |
| **OpenAI Responses** | `openai`（默认） | 1) `codex/codex-rs/codex-api`（`ResponsesApiRequest`、SSE `response.*` 事件） 2) `pi/.../openai-responses.ts`（`store:false`、`max_output_tokens`、tools） 3) `fugu`（产品侧单 model 走 Responses/Chat 接入形态） |

共享辅助：`pigs-llm/src/http_util.rs`（状态映射、Retry-After、非重试错误、URL join）。

## 8. 工具系统

### 8.1 工具注册表

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, handler: Box<dyn ToolHandler>);
    pub fn definitions(&self) -> Vec<ToolSpec>;
    pub fn execute(&self, name: &str, input: Value) -> Result<ToolResult, ToolError>;
    pub fn names(&self) -> Vec<String>;
}
```

### 8.2 内置工具

| 工具 | 权限 | 说明 |
|---|---|---|
| `bash` | DangerFullAccess | 执行 shell 命令（带超时、工作目录） |
| `read_file` | ReadOnly | 读取文件（带行号、范围、大小限制） |
| `write_file` | WorkspaceWrite | 写入文件（创建或覆盖，自动创建父目录） |
| `edit_file` | WorkspaceWrite | 精确字符串替换（支持 `replace_all`） |
| `apply_patch` | WorkspaceWrite | 应用 unified diff 补丁（支持 dry-run） |
| `grep_search` | ReadOnly | 正则搜索文件内容（递归、glob 过滤、`.pigsignore`） |
| `glob_search` | ReadOnly | 文件名模式匹配（`.pigsignore`） |
| `list_files` | ReadOnly | 列出目录内容（支持递归、`.pigsignore`） |
| `git_diff` | ReadOnly | 查看 git unstaged/staged 变更 |
| `web_fetch` | ReadOnly | HTTP GET 抓取网页内容 |
| `web_search` | ReadOnly | DuckDuckGo Instant Answer 搜索 |
| `http_request` | ReadOnly | 通用 HTTP 请求（GET/POST/PUT/PATCH/DELETE/HEAD，headers/json/body） |
| `ask_user` | ReadOnly | 结构化用户提问（带选项） |
| `todo_write` | ReadOnly | 任务跟踪（共享状态） |
| `sleep` | ReadOnly | 暂停执行（带上下限 clamp） |
| `agent` | ReadOnly | 子代理委派（独立上下文 + 只读工具） |

### 8.4 `.pigsignore`

工作区根目录的 `.pigsignore` 使用与 `.gitignore` 相同的格式。  
`grep_search` / `glob_search` / `list_files` 在搜索时会自动排除匹配路径，并始终排除默认目录（`target/`、`node_modules/`、`.git/` 等）。

### 8.3 工具输入 schema

每个工具用 `serde_json::json!({...})` 定义 JSON Schema，例如：

```json
{
    "type": "object",
    "properties": {
        "command": { "type": "string", "description": "The shell command to execute" },
        "timeout": { "type": "integer", "description": "Timeout in seconds", "default": 120 }
    },
    "required": ["command"]
}
```

## 9. 会话持久化

### 9.1 Session 结构

```rust
pub struct Session {
    pub session_id: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub workspace_root: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_usage: TokenUsage,
}
```

### 9.2 JSONL 持久化

- 存储路径：`~/.pigs/sessions/{session_id}.jsonl`
- 每行一条 JSON 消息
- `save()`：追加新消息到文件
- `load()`：读取全部消息重建 Session

### 9.3 上下文压缩

- 阈值：累计 input tokens > 100,000
- 策略：将最早的 N 条消息替换为一条系统摘要消息
- 保留最近 4 条消息不变

### 9.4 会话标题

- 首条用户消息自动生成 `title`（最多 60 字符）
- `/title <name>` 可手动设置
- `/sessions` 与 `--list-sessions` 展示标题

## 10. 配置管理

### 10.1 配置文件

路径：`~/.pigs/config.toml`

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



```

### 10.2 配置优先级

CLI 参数 > 环境变量 > 项目配置 `{cwd}/.pigs/config.toml` > 全局配置 `~/.pigs/config.toml` > 默认值

压缩相关配置：

- `compact_token_threshold`（默认 100000）
- `compact_keep_recent`（默认 4）
- `/compact` 强制压缩（force=true）

一次性输出：

- `--output text|json`（json 仅 one-shot 模式）

项目配置通过 `merge_project_overrides` 合并：标量字段覆盖，`mcp_servers` / hooks 列表追加。

### 10.3 项目记忆（CLAUDE.md / AGENTS.md）

工作区根目录下按优先级加载**一个**项目记忆文件（全文注入 system prompt）：

1. `CLAUDE.md`（优先）
2. `AGENTS.md`（仅当不存在有效 `CLAUDE.md` 时）

分段标题为 `--- Project Context (<filename>) ---`。不向上递归父目录。

### 10.4 热重载与日志

- `/reload`：从磁盘和 env 重新加载配置，并重建 LLM 客户端 / 权限策略 / 系统提示词
- 日志默认写入 `~/.pigs/logs/pigs.log.YYYY-MM-DD`（`log_to_file = true`）
- 可通过 `PIGS_LOG_LEVEL` / `PIGS_LOG_TO_FILE` 覆盖

### 10.5 MCP 配置

```toml
[[mcp_servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
enabled = true
```

- 启动时自动连接 `enabled = true` 的服务器
- 工具注册为 `mcp_{server}_{tool}`
- 运行时可用 `/mcp connect` / `/mcp disconnect` 管理

### 10.6 Skills

从以下目录加载 markdown 技能（**先出现的同名技能优先**）：

1. `~/.pigs/skills/` — pigs 用户技能
2. `~/.agents/skills/` — 通用用户 Agent 技能
3. `{workspace}/.pigs/skills/` — pigs 项目技能
4. `{workspace}/.agents/skills/` — 通用项目技能（如 ARIS 的 `SKILL.md` 目录）
5. `{workspace}/skills/` — 工作区根目录技能

支持：

- 直接 `*.md` 文件
- 子目录 `skill-name/SKILL.md`
- YAML frontmatter：`name` / `description`

系统提示词仅注入 **技能目录**（名称 + 短描述）。完整正文通过工具 `skill` 按需加载（对齐 claw-code），避免把全部 SKILL.md 塞进上下文。

### 10.7 Hooks

```toml
[[hooks.pre_tool_use]]
matcher = "bash"          # 精确名 / 前缀* / *
command = "echo check $PIGS_TOOL_NAME"
timeout = 10
enabled = true

[[hooks.post_tool_use]]
matcher = "*"
command = "echo done $PIGS_TOOL_NAME"
enabled = true
```

环境变量：

- `PIGS_HOOK_EVENT`：`pre_tool_use` / `post_tool_use`
- `PIGS_TOOL_NAME`
- `PIGS_TOOL_INPUT`（JSON）
- `PIGS_TOOL_OUTPUT`（仅 post）
- `PIGS_TOOL_IS_ERROR`（仅 post）

Pre-tool hook 退出码非 0 时拒绝工具执行。

### 10.8 Project Rules

从 `{workspace}/.pigs/rules/*.md` 加载项目规则，并注入系统提示词 `--- Project Rules ---` 段。
可用 `/rules` 查看，`/rules reload` 重载。

### 10.9 Status Dashboard

`/status` 汇总：session title/id、model、permission、messages、tokens、cost、tools、skills、rules、memory、MCP servers、workspace、logs。

### 10.10 Memory

跨会话记忆笔记：

- 全局：`~/.pigs/memory.md`
- 项目：`.pigs/memory.md`

命令：

- `/memory list`
- `/memory add [--global] <note>`
- `/memory rm [--global] <substring>`
- `/memory reload`

Memory 会注入系统提示词 `--- Memory Notes ---` 段。

## 11. CLI

### 11.1 参数

```
pigs [OPTIONS] [PROMPT]

Arguments:
    [PROMPT]  一次性提示（不进入 REPL）

Options:
    --model <MODEL>          指定模型
    --mode <MODE>           权限模式 (readonly, workspace_write, danger, ask, allow)
    --system-prompt <TEXT>  自定义系统提示词
    --resume <ID>           恢复会话
    --no-tools              禁用工具
    --max-turns <N>         最大循环次数 (默认 50)
    --list-sessions         列出保存的会话（无需 API key）
    --output <FORMAT>       一次性模式输出：text（默认）或 json
    --help                  显示帮助
    --version               显示版本
```

### 11.2 斜杠命令

| 命令 | 说明 |
|---|---|
| `/help` | 显示帮助 |
| `/model <name>` | 切换模型（opus/sonnet/haiku/gpt-4o 或完整 id） |
| `/mode <permission>` | 切换权限模式 |
| `/tools` | 列出可用工具 |
| `/todo` | 显示当前任务列表 |
| `/status` | 状态仪表盘 |
| `/info` | 显示会话信息（含 token 估算） |
| `/cost` | 显示 token 用量与估算费用 |
| `/title [name]` | 显示或设置会话标题 |
| `/history` | 显示会话消息历史摘要 |
| `/mcp` | 管理 MCP 服务器（list/tools/connect/disconnect） |
| `/skills [reload]` | 列出或重载 skills |
| `/rules [reload]` | 列出或重载项目规则 |
| `/memory ...` | 管理跨会话记忆笔记 |
| `/export [path]` | 导出会话为 markdown |
| `/undo [list]` | 撤销最近写工具变更（快照持久化到 `.pigs/undo/`，启动时自动加载） |
| `/hooks` | 显示工具生命周期 hooks |
| `/doctor` | 环境与配置健康检查 |
| `/models` | 列出已知模型别名 |
| `/init` | 创建默认配置文件 |
| `/reload` | 从磁盘/环境变量热重载配置 |
| `/compact` | 手动压缩会话上下文 |
| `/clear` | 清除当前会话 |
| `/save` | 保存会话 |
| `/sessions` | 列出保存的会话 |
| `/quit` | 退出 |

### 11.3 REPL 循环

```
loop:
    line = rustyline.readline("pigs> ")
    if line.starts_with("/"):
        handle_slash_command(line)
    else if line.trim().is_empty():
        continue
    else:
        run_turn(line)
```

## 12. 关键依赖

| 依赖 | 用途 |
|---|---|
| `tokio` | 异步运行时 |
| `reqwest` | HTTP 客户端（带 streaming） |
| `futures-util` | SSE 流式处理（`StreamExt`） |
| `serde` / `serde_json` | 序列化 |
| `toml` | 配置解析 |
| `rustyline` | REPL 行编辑 |
| `clap` | CLI 参数解析 |
| `regex` | grep 工具 |
| `glob` | glob 工具 |
| `dirs` | 跨平台目录路径 |
| `uuid` | 会话 ID |
| `chrono` | 时间戳 |
| `tracing` / `tracing-subscriber` | 控制台日志 |
| `tracing-appender` | 按日滚动文件日志（`~/.pigs/logs/`） |
| `async-trait` | 异步 trait |
| `anyhow` | 错误处理 |
| `thiserror` | 错误类型派生 |

## 12.5. 流式输出（SSE）

### StreamEvent 类型

```rust
pub enum StreamEvent {
    TextDelta(String),
    ToolUseStart { id: String, name: String },
    ToolUseInputDelta { id: String, partial_json: String },
    ToolUseEnd { id: String },
    Usage(TokenUsage),
    Done { stop_reason: Option<String> },
}
```

### StreamCallback trait

```rust
pub trait StreamCallback: Send + Sync {
    fn on_event(&self, event: &StreamEvent);
}
```

### 实现

- **OpenAI**：`send_message_streaming` 使用 `stream: true`，解析 `data: {json}` SSE 行，
  累积 `delta.content`（文本）和 `delta.tool_calls[].function.arguments`（工具输入 JSON 片段），
  最终组装为 `ApiResponse`。
- **Anthropic**：`send_message_streaming` 解析 `event: <type>\ndata: <json>` 格式的 SSE 事件，
  处理 `content_block_start`（工具开始）、`content_block_delta`（文本/工具输入增量）、
  `content_block_stop`（工具结束）、`message_delta`（stop_reason + usage）。
- **默认实现**：`ApiClient::send_message_streaming` 提供 fallback，将非流式响应分解为 StreamEvent。
- **CLI**：`StreamPrinter` 实现了 `StreamCallback`，实时将 `TextDelta` 打印到 stdout。

## 13. Clippy Lint 规则

```toml
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
uninlined_format_args = "warn"
needless_borrow = "warn"
redundant_clone = "warn"
# unsafe_code = "forbid" (在 lib.rs 中设置)
```

测试模块可使用局部放行（仅 `mod tests` 内）：

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    // ...
}
```

生产代码仍禁止 `unwrap` / `expect`。`cargo clippy --workspace --all-targets` 应通过。

## 14. 冒烟与无 API key 路径

以下路径**不依赖**真实 LLM API key，适合本地/CI 快速检查：

| 命令 / 路径 | 说明 |
|---|---|
| `pigs --help` / `pigs -V` | clap 解析与二进制可执行 |
| `pigs --list-sessions` | 在创建 `Agent` 之前返回，只读会话目录 |
| `Agent::new` + 假 OpenAI key / 自定义 base_url | 构造客户端不发起真实请求；用于 doctor 冒烟 |
| `/doctor`（或 `doctor::run_doctor`） | 检查配置/目录/工具注册/git 等；凭证缺失记为 ERR 项而非 panic |

集成冒烟测试位于 `crates/pigs-cli/tests/cli_smoke.rs`。
