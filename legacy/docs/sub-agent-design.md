# 子智能体设计文档

## 概念

- **pig** = 相位编排（Pre→Executor→Post）
- **pigs** = pig + 自由子智能体
- 子智能体本身也是一个完整的 pig（走相位编排、有工具、有会话），且可递归创建子智能体

## 设计目标

主智能体通过工具调用创建子智能体，支持：
1. **前台模式**：主智能体等待子智能体完成，拿到结果继续推理
2. **后台模式**：主智能体不等待，子智能体完成后通知
3. **一次创建多个**：一次 tool call 可定义多个子智能体
4. **完全对等**：子智能体使用与主智能体相同的工具、模型和 pig 相位编排
5. **递归创建**：子智能体可以继续创建子智能体
6. **上下文可选共享**：创建时由主智能体决定
7. **TUI 查看**：`/sub <id>` 切换查看，`/sub back` 返回

## 架构

### 数据结构

```rust
/// 子智能体记录
pub struct SubAgent {
    pub id: String,              // 如 "sub-001"
    pub session: Session,        // 独立会话（可选共享主智能体的消息历史）
    pub task: String,            // 任务描述
    pub mode: SubAgentMode,      // Foreground / Background
    pub status: SubAgentStatus,  // Pending / Running / Done / Error
    pub result: Option<String>,  // 完成后的结果文本
    pub parent_id: String,       // 父智能体 ID（主智能体或上级子智能体）
    pub children: Vec<String>,   // 子智能体 ID 列表（递归）
}

pub enum SubAgentMode {
    Foreground,  // 主智能体等待
    Background,  // 不等待，完成后通知
}

pub enum SubAgentStatus {
    Pending,
    Running,
    Done,
    Error(String),
}

/// 子智能体管理器
pub struct SubAgentManager {
    pub agents: HashMap<String, SubAgent>,
    pub current_focus: String,  // 当前 TUI 查看的智能体 ID（"main" 或子智能体 ID）
    pub counter: u32,           // ID 自增计数器
}
```

### Spawn 工具

主智能体通过 `spawn` 工具创建子智能体。工具 schema：

```json
{
  "name": "spawn",
  "description": "Create one or more sub-agents to work on tasks. Sub-agents are fully equivalent to the main agent (same tools, same model, same pig phased orchestration). They can recursively create their own sub-agents.",
  "input_schema": {
    "type": "object",
    "properties": {
      "agents": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "task": { "type": "string", "description": "Task description for the sub-agent" },
            "mode": { "type": "string", "enum": ["foreground", "background"], "default": "foreground" },
            "share_context": { "type": "boolean", "default": false, "description": "Whether to share the parent's conversation context" }
          },
          "required": ["task"]
        }
      }
    },
    "required": ["agents"]
  }
}
```

**行为：**
- `foreground`：工具阻塞，等待所有子智能体完成，返回所有结果
- `background`：工具立即返回子智能体 ID 列表，子智能体异步运行

### TUI 命令

| 命令 | 功能 |
|---|---|
| `/sub list` | 列出所有子智能体及状态 |
| `/sub <id>` | 切换到指定子智能体会话视图 |
| `/sub back` | 返回主智能体会话 |
| `/sub new <task>` | 用户手动创建前台子智能体（非工具调用） |

## 实现计划

### 第 1 步：SubAgentManager 核心模块

文件：`crates/pigs-cli/src/sub_agent.rs`

- `SubAgent` 结构体
- `SubAgentManager` 结构体（管理所有子智能体）
- `spawn()` — 创建子智能体（分配 ID、初始化会话）
- `run_foreground()` — 同步运行前台子智能体（调用 Agent::run_turn_with_callback）
- `run_background()` — 异步运行后台子智能体（tokio::spawn）
- `switch_to()` / `switch_back()` — 切换查看焦点
- `list()` — 列出所有子智能体

### 第 2 步：Spawn 工具

文件：`crates/pigs-cli/src/sub_agent_tool.rs`

- 实现 `ToolHandler` trait
- `execute()` 解析输入，调用 SubAgentManager::spawn()
- 前台模式：等待所有子智能体完成，聚合结果返回
- 后台模式：立即返回 ID 列表

### 第 3 步：/sub 斜杠命令

文件：`crates/pigs-cli/src/commands.rs`

- `/sub list` — 显示所有子智能体
- `/sub <id>` — 切换到子智能体会话
- `/sub back` — 返回主会话
- `/sub new <task>` — 手动创建子智能体

### 第 4 步：TUI 集成

文件：`crates/pigs-tui/src/app.rs` + `crates/pigs-cli/src/tui_repl.rs`

- ChatState 支持切换显示不同智能体的消息
- 子智能体创建时在主对话中显示 ID
- 后台完成时在主对话中显示通知
- `/sub <id>` 切换后显示子智能体的完整对话历史

### 第 5 步：Agent 集成

文件：`crates/pigs-cli/src/agent.rs`

- Agent 持有 `SubAgentManager`
- 注册 spawn 工具到 ToolRegistry
- `run_turn_with_callback` 中 spawn 工具被调用时，触发子智能体运行

## 关键设计决策

### 1. 子智能体的 Agent 实例

子智能体不创建新的 `Agent` 结构体——而是复用主 Agent 的 `api_client`、`tool_registry`、`config`，只创建独立的 `Session`。这避免了重复初始化（MCP 连接、技能加载等）。

```rust
// 子智能体运行时：
fn run_sub_agent(agent: &Agent, sub: &mut SubAgent) -> String {
    // 复用 agent.api_client, agent.tool_registry, agent.config
    // 但使用 sub.session 作为会话上下文
    // 调用 agent.run_turn_with_callback 使用 sub.session
}
```

### 2. 前台模式的等待机制

前台子智能体在工具 `execute()` 内同步等待。由于 `execute()` 返回 `ToolFuture`（async），可以使用 async 等待：

```rust
async fn execute(&self, input: Value) -> String {
    let subs = manager.spawn(input);
    // 等待所有前台子智能体完成
    for sub in &subs {
        run_sub_agent(agent, sub).await;
    }
    // 聚合结果返回
    format_results(&subs)
}
```

### 3. 后台模式的异步运行

后台子智能体使用 `tokio::spawn` 异步运行，完成后通过 channel 通知主智能体：

```rust
fn spawn_background(agent: Arc<Agent>, sub: SubAgent, tx: mpsc::Sender<SubAgentNotification>) {
    tokio::spawn(async move {
        let result = run_sub_agent(&agent, &mut sub).await;
        tx.send(SubAgentNotification { id: sub.id, result }).await;
    });
}
```

### 4. 上下文共享

```rust
// share_context = true: 复制父会话的消息
sub.session.messages = parent_session.messages.clone();

// share_context = false: 空会话，只有任务描述
sub.session.messages = vec![Message::user(&task)];
```

### 5. TUI 会话切换

TUI 的 ChatState 需要支持"焦点切换"：

```rust
struct ChatState {
    // 主智能体的聊天记录
    main_entries: Vec<ChatEntry>,
    // 每个子智能体的聊天记录
    sub_entries: HashMap<String, Vec<ChatEntry>>,
    // 当前焦点
    focus: String,  // "main" 或 sub ID
}

fn render_lines(&self) -> Vec<Line> {
    if self.focus == "main" {
        &self.main_entries
    } else {
        self.sub_entries.get(&self.focus).unwrap_or(&self.main_entries)
    }
}
```

## ID 分配

子智能体 ID 格式：`sub-001`, `sub-002`, ...（零填充 3 位）

子智能体的子智能体：`sub-001.1`, `sub-001.2`, ...

## 文件变更清单

| 文件 | 类型 | 说明 |
|---|---|---|
| `crates/pigs-cli/src/sub_agent.rs` | 新建 | SubAgent + SubAgentManager |
| `crates/pigs-cli/src/sub_agent_tool.rs` | 新建 | spawn 工具（ToolHandler 实现） |
| `crates/pigs-cli/src/lib.rs` | 修改 | 添加模块声明 |
| `crates/pigs-cli/src/agent.rs` | 修改 | 持有 SubAgentManager，注册 spawn 工具 |
| `crates/pigs-cli/src/commands.rs` | 修改 | 添加 /sub 命令 |
| `crates/pigs-cli/Cargo.toml` | 修改 | 无新依赖（复用现有） |
| `crates/pigs-tui/src/chat.rs` | 修改 | 支持焦点切换 |
| `crates/pigs-tui/src/app.rs` | 修改 | 子智能体创建/完成通知显示 |
| `crates/pigs-cli/src/tui_repl.rs` | 修改 | /sub 命令路由到 TUI |

## 不改的部分

- pigs-api（相位运行时）不变——子智能体复用现有 PhasedRuntime
- pigs-proxy 不变——子智能体通过 HTTP 连接同一个代理
- pigs-core 不变——ToolHandler trait 和 ToolRegistry 不变
- MCP/Skills/Rules/Memory/Hooks/权限系统不变——子智能体复用主智能体的

## 参考项目

- **oh-my-openagent**：Team Mode（主智能体 + 最多 8 个并行成员）
- **pi**：subagent 扩展（scout/planner/reviewer/worker 角色，single/parallel/chained 模式）
