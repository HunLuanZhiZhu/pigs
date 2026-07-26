# pigs-api crate 详细文档

> 本文档是 `crates/pigs-api` 的完整参考手册，与源码中的中英双语注释配套。
> This document is the complete reference for `crates/pigs-api`, paired with
> the bilingual (Chinese + English) comments in the source code.

## 1. 概述 / Overview

`pigs-api` 是 pigs 项目的**相位化 Agent 运行时模块库**。它实现了 pig 的核心理念：
对单个 LLM API 请求施加 **Pre → Executor → Post** 三阶段结构。每个智能体（主或子）
的每一次"轮次"都经过这三个相位：

- **Pre**：规划 / 分流 / GOAL 声明。简单任务可直接 `PIGEND` 结束；复杂任务产出计划交给 Executor。
- **Executor**：信息收集 + 起草答复。可调用工具，完成后进入 Post。
- **Post**：审阅 + GOAL 验收 + 路由。可 `PIGEND` 结束、`PIGFAIL` 回 Pre 重规划，或无标记反馈给 Executor 重试。

该 crate 提供**两条运行时路径**：

| 路径 / Path | 入口 / Entry | 适用场景 / Use case | 传输 / Transport |
|---|---|---|---|
| **CLI 本地运行时** | `phased_runtime::PhasedRuntime` | CLI / `--once` 进程内执行 | 直接 `ApiClient`（通常 `ProxyApiClient`） |
| **HTTP 相位运行时** | `http_runtime::HttpPhasedRuntime` | `pigs-proxy` 的 `-pig` 路由 | `PhaseTransport`（loopback 回代理） |

两条路径共享同一套相位编排逻辑（`orchestration`）、控制标记（`phased_markers`）、
相位标识（`phased_phase`）和提示词（`phased_prompts`）。

## 2. 模块结构 / Module Structure

```
crates/pigs-api/
├── Cargo.toml                      # crate 清单 / manifest
├── src/
│   ├── lib.rs                      # 模块入口与 re-export / entry & re-exports
│   ├── phased_phase.rs             # Phase 枚举 / Phase enum
│   ├── phased_markers.rs           # PIGEND/PIGFAIL 标记检测与清理 / marker detection & stripping
│   ├── phased_prompts.rs           # 从 pigs_prompts re-export 提示词 / re-exported prompts
│   ├── phased_tools.rs             # 相位运行时工具注册表 / tool registry
│   ├── orchestration.rs            # 纯相位状态机 / pure state machine
│   ├── transport.rs                # 传输抽象 trait / transport abstraction
│   ├── continuation.rs             # 有界内存 continuation 存储 / bounded continuation store
│   ├── phased_api_convert.rs       # OpenAI 请求 → 运行时输入 / request conversion (CLI)
│   ├── phased_runtime.rs           # CLI 本地相位运行时 / CLI local phased runtime
│   ├── protocol.rs                 # 协议原生请求信封与 codec / native envelope & codec
│   ├── http_runtime.rs             # HTTP 相位运行时 / HTTP phased runtime
│   ├── format.rs                   # 三格式请求解析与响应构造 / 3-format parse & build (CLI)
│   └── output.rs                   # 协议原生 JSON 与 SSE 编码 / native JSON & SSE encoding
└── tests/
    ├── orchestration.rs            # 状态机测试 / state machine tests
    ├── output.rs                   # SSE/JSON 编码测试 / encoding tests
    ├── protocol_codec.rs           # 协议 codec 测试 / codec tests
    └── http_runtime.rs             # HTTP 运行时测试 / HTTP runtime tests
```

## 3. 核心概念 / Core Concepts

### 3.1 三相位状态机 / Three-Phase State Machine

状态机定义在 `orchestration.rs` 的 `OrchestrationState` 中，是纯逻辑、无传输依赖的：

```
        ┌─────────── PIGFAIL ───────────┐
        ▼                               │
      ┌─────┐  无标记   ┌──────────┐    │
      │ Pre │ ───────▶ │ Executor │ ──▶ │
      └─────┘          └──────────┘    │
        │  PIGEND                        │
        ▼                               │
      完成                            ┌──────┐
                                       │ Post │
                                       └──────┘
                                         │  PIGEND → 完成
                                         │  PIGFAIL → 回 Pre
                                         │  无标记 → 回 Executor（受预算约束）
```

**预算 / Budgets**（`OrchestrationLimits`）：
- `max_post_iterations`：Post 无标记重试上限（默认 3）。
- `max_pre_replans`：PIGFAIL 回 Pre 重规划上限（默认 2）。

超预算会返回明确的 `OrchestrationError`（`PostBudgetExceeded` / `ReplanBudgetExceeded`），
**绝不**被视为成功完成。

### 3.2 控制标记 / Control Markers

定义在 `phased_markers.rs`：

| 标记 / Marker | 含义 / Meaning | 检测规则 / Detection rule |
|---|---|---|
| `PIGEND` | 整轮正常结束 | 必须是最后一个非空行，且前面有非标记的"原因"行 |
| `PIGFAIL` | 路径失败，需重规划 | 同上 |

检测函数 `detect_marker` 容忍标记周围的 Markdown 装饰（反引号、星号、末尾标点），
但严格要求"原因行"存在——单独一行 `PIGEND` 不会被识别为有效标记。`strip_markers`
从用户可见文本中删除整行标记，但保留句子中出现的标记词（如 "PIGEND appears in prose"）。

### 3.3 三种 API 协议 / Three API Protocols

定义在 `protocol.rs` 的 `Protocol` 枚举：

| 协议 / Protocol | 端点 / Endpoint | system 位置 / system location | 消息集合 / message collection |
|---|---|---|---|
| `OpenAiChat` | `/chat/completions` | messages 数组中 role=system | `messages` |
| `AnthropicMessages` | `/v1/messages` | 顶层 `system` 字段 | `messages` |
| `OpenAiResponses` | `/responses` | 顶层 `instructions` 字段 | `input`（字符串或数组）|

**核心原则 / Core principle**：输入什么格式，输出什么格式。HTTP 相位运行时**不解构**
请求体——方法、path、query、headers 和原生 JSON 全部原样保留，只定点修改 model、
当前 user 文本和相位对话记录。

## 4. 模块详解 / Module Details

### 4.1 `phased_phase.rs` — 相位枚举

```rust
pub enum Phase { Pre, Executor, Post }
```

每个变体对应一个相位。`Phase::as_str()` 返回小写字符串标识（`"pre"` / `"executor"` / `"post"`），
用于日志、事件和 SSE 帧。

### 4.2 `phased_markers.rs` — 控制标记

- `PIGEND` / `PIGFAIL`：常量字符串。
- `Marker`：`End` / `Failed` 枚举。
- `detect_marker(text)`：检测最后一个非空行是否是有效标记。
- `strip_markers(text)`：删除整行标记，返回用户可见文本。
- `is_control_marker_line(line)`：判断单行是否是控制标记（供流式缓冲区使用）。

### 4.3 `orchestration.rs` — 纯状态机

`OrchestrationState` 持有：
- 当前相位 `phase`
- 预算 `limits`
- 各相位的可见文本输出（`pre_output` / `executor_outputs` / `post_outputs` / `failure_outputs`）
- 计数器（`post_iterations` / `pre_replans`）

核心方法 `advance(&mut self, output: &str) -> Result<Advance, OrchestrationError>`：
接收一个相位的**原始**文本输出（含标记），返回 `Complete` 或 `Continue(下一相位)`。

### 4.4 `transport.rs` — 传输抽象

```rust
#[async_trait]
pub trait PhaseTransport: Send + Sync {
    async fn send(&self, request: HttpRequestEnvelope) -> Result<TransportResponse, TransportError>;
    async fn send_streaming(&self, request: HttpRequestEnvelope, text: TransportTextSink)
        -> Result<TransportResponse, TransportError>;
}
```

`PhaseTransport` 把"发送一次相位子请求"与具体 HTTP 机制解耦。实现者通常把请求转发给
`pigs-proxy` 的本地 loopback。`send_streaming` 有默认实现（退化为非流式），真正的流式
实现应覆盖它以即时转发上游 SSE 增量。

### 4.5 `continuation.rs` — 有界 continuation 存储

当相位运行时遇到工具调用时，它不能在进程内执行（工具由上游 Agent 执行）。运行时把
当前轮次的完整状态打包成 `Continuation` 存入 `ContinuationStore`，返回 `ToolPause`。

**存储特性 / Store properties**：
- **有界**：`capacity` 上限（默认 256），超限时最老条目被 FIFO 淘汰（`Evicted`）。
- **TTL**：`ttl`（默认 30 分钟），过期条目失效（`Expired`）。
- **一次性消费**：成功恢复后标记为 `Consumed`。
- **墓碑 / Tombstones**：被移除的工具 ID 留下墓碑，让后续到达的结果能区分"从未存在"
  与"已过期/已淘汰/已消费"。墓碑数量上限为容量的 8 倍。
- **永不写盘**：纯内存，进程重启即丢失。

`take_ready()` 的匹配规则：第一个 supplied_id 定位 continuation → 所有 supplied_id 必须属于
同一 continuation → 协议与真实模型必须匹配 → 合并结果，若仍有缺失返回 `Waiting`，全部就位
返回 `Ready`。

### 4.6 `phased_tools.rs` — 工具注册表

`info_tool_registry()` 复用 `pigs-tools` 的完整内置工具注册表（与 `pigs-cli` 相同的工具面），
额外添加进程级 `internal_notes` 暂存工具：

- `action=write`：写入笔记（id + content）。
- `action=read`：按 id 读取笔记。
- `action=list`：列出全部笔记（预览限 120 字符）。

笔记存储是进程级静态 `Mutex<Option<HashMap>>`，非请求级。

### 4.7 `phased_api_convert.rs` — 请求转换（CLI）

把 OpenAI Chat 格式的 `ChatCompletionsRequest` 转换为 `ConvertedTurn`：
- 保留完整的调用方消息数组（含 system 角色）。
- 相位运行时 clone 一份，去掉最后一条 user 消息，追加相位特定 user 消息。
- 原始 system / history 永不被解构或重新组装。

`run_converted_turn()` 会从 `converted.model` 剥离 `-pig` 后缀得到真实模型名，
让一个共享的 `PhasedRuntime` 实例可以服务不同的 `-pig` 模型请求。

### 4.8 `phased_runtime.rs` — CLI 本地运行时

`PhasedRuntime` 是 CLI 路径的主结构体，持有：
- `api: Arc<dyn ApiClient>`：LLLM 客户端（通常是 `ProxyApiClient`）。
- `remote_model` / `wrapped_model`：真实模型名与包装模型名（`-pig` 后缀）。
- `tools: ToolRegistry`：全量内置工具 + internal_notes。
- `limits: RuntimeLimits`：每相位工具轮次、Executor 回环、Pre 重规划、max_tokens、temperature。
- `language: Language`：提示词语言（默认 zh）。
- `is_pig: bool`：是否启用相位编排。

**核心方法 / Key method**：`run_turn_with_progress()`
1. 拆分调用方消息：system_prompt + base_history + user_question。
2. 非 pig 模式：单次 LLM 调用，返回 `DIRECT`。
3. pig 模式：Pre → Executor → Post 循环，按标记路由，受预算约束。

`run_phase()` 执行单个相位的工具循环：构建 ApiRequest → 调用 LLM（流式）→ 有 tool_use
则执行工具并追加结果回环 → 无 tool_use 则相位结束。

### 4.9 `protocol.rs` — 协议原生信封与 codec

`HttpRequestEnvelope` 是 HTTP 相位运行时数据面的"信封"，保留请求的全部原生结构：
- `method` / `path_and_query` / `headers`：传输元数据。
- `body: Value`：完整的原生 JSON（未知字段与原生块原样保留）。
- `protocol` / `client_model` / `real_model` / `stream`：解析后的语义信息。
- `current_user`：当前 user 输入在 body 中的位置（内部）。
- `tool_result_ids` / `tool_result_groups`：工具结果信息。

**三类操作 / Three operations**：
1. **解析**（`ProtocolCodec::parse_request`）：校验 body，定位当前 user。
2. **相位变更**：
   - `for_pre` / `for_executor`：clone 请求，在当前 user 文本后追加后缀。
   - `for_post`：移除当前 user，替换为原生对话记录 + 审阅消息。
   - `with_appended_transcript`：在当前 user 后追加对话记录。
3. **响应抽取**（`extract_response`）：把上游响应解析为 `NormalizedModelOutput`。

### 4.10 `http_runtime.rs` — HTTP 相位运行时

`HttpPhasedRuntime` 是 HTTP 路径的主结构体，持有：
- `transport: Arc<dyn PhaseTransport>`：底层传输。
- `config: HttpRuntimeConfig`：语言、编排预算、continuation 边界。
- `continuations: Mutex<ContinuationStore>`：有界 continuation 存储。

**执行循环 / Execution loop**（`execute()`）：
1. 从编排状态取当前相位，构建相位请求（注入相位 user payload + 传输覆盖）。
2. 追加本相位已累积的原生对话记录。
3. 发送请求（流式或非流式）。
4. 解析响应为 `NormalizedModelOutput`，累积到 Session。
5. 有工具调用 → 打包 continuation 存入存储，返回 `ToolPause`。
6. 无工具调用 → 推进状态机：`Complete` 则返回，`Continue` 则进入下一相位。

**流式缓冲区 / Streaming buffer**（`MarkerLineBuffer`）：按行缓存上游增量，仅在确认不是
控制标记后才转发。控制标记可能跨多个 SSE chunk 到达，必须缓存"尚未结束的最后一行"。

### 4.11 `output.rs` — JSON 与 SSE 编码

提供两类编码器：

**无状态函数 / Stateless functions**：
- `encode_json(protocol, model, result)`：非流式 JSON 响应体。
- `encode_sse(protocol, model, result)`：完整 SSE 帧序列。
- `encode_sse_error(protocol, message)`：流内错误帧（不含成功终止帧）。

**有状态编码器 / Stateful encoder**（`StreamingEncoder`）：在相位执行仍在进行时逐段发出
SSE 帧。生命周期：`start` → (`phase_start` → `text_delta`* → `phase_end`)* →
`finish`/`abort_phase`/`error`。内部维护索引、序列号和当前打开的内容块，保证三种协议的
事件结构合法。

### 4.12 `format.rs` — 三格式解析与构造（CLI）

`ApiFormat` 枚举（`OpenAIChat` / `Anthropic` / `OpenAIResponses`）提供：
- `from_path(path)`：从请求路径推断格式。
- `parse_request(body)`：解析为 `ConvertedTurn`。
- `build_response(result, model)`：构造非流式响应 JSON。
- `role_chunk` / `content_chunk` / `stop_chunk`：构造 SSE 帧。
- `done_sentinel()`：流结束哨兵（Chat 用 `[DONE]`，其余无）。

## 5. 数据流 / Data Flow

### 5.1 CLI 路径 / CLI Path

```
用户输入 / user input
  │
  ▼
ConvertedTurn::from_request()        # 解析为 messages + model + stream
  │
  ▼
run_converted_turn()                 # 剥离 -pig 后缀
  │
  ▼
PhasedRuntime::run_turn_with_progress()
  │
  ├─ 非 pig: 单次 LLM 调用 → DIRECT
  │
  └─ pig: Pre → Executor → Post 循环
       │
       ▼
     run_phase()                      # 工具循环
       │
       ├─ ApiClient::send_message_streaming()  # 经 ProxyApiClient → pigs-proxy
       │
       └─ tools.execute()             # 进程内工具执行
```

### 5.2 HTTP 路径 / HTTP Path

```
客户端请求 / client request (带 -pig 后缀)
  │
  ▼
pigs-proxy 路由分流
  │
  ▼ (HTTP loopback)
HttpPhasedRuntime::run()
  │
  ├─ 首次请求: execute()
  │    │
  │    ▼
  │  phase_request()                  # 注入相位 user payload
  │    │
  │    ▼
  │  PhaseTransport::send_streaming() # 回 pigs-proxy → 上游 LLM
  │    │
  │    ▼
  │  extract_response()               # 解析为 NormalizedModelOutput
  │    │
  │    ├─ 有工具调用 → ContinuationStore.insert() → 返回 ToolPause
  │    │
  │    └─ 无工具调用 → orchestration.advance() → 下一相位或完成
  │
  └─ continuation 请求: resume()
       │
       ▼
     ContinuationStore.take_ready()   # 匹配工具结果
       │
       ▼
     execute()                         # 恢复轮次
```

## 6. 关键类型索引 / Key Type Index

| 类型 / Type | 模块 / Module | 说明 / Description |
|---|---|---|
| `Phase` | `phased_phase` | 相位枚举（Pre/Executor/Post）|
| `Marker` | `phased_markers` | 标记类型（End/Failed）|
| `OrchestrationState` | `orchestration` | 纯状态机 |
| `OrchestrationLimits` | `orchestration` | 状态机预算 |
| `Advance` | `orchestration` | 推进结果（Complete/Continue）|
| `OrchestrationError` | `orchestration` | 预算耗尽错误 |
| `Continuation` | `continuation` | 暂停轮次的完整状态 |
| `ContinuationStore` | `continuation` | 有界 TTL 存储 |
| `ContinuationConfig` | `continuation` | 容量与 TTL |
| `Lookup` | `continuation` | 匹配结果（Ready/Waiting）|
| `ContinuationError` | `continuation` | 查找/校验失败 |
| `PhaseTransport` | `transport` | 传输 trait |
| `TransportResponse` / `TransportError` | `transport` | 传输响应与错误 |
| `HttpRequestEnvelope` | `protocol` | 协议原生请求信封 |
| `Protocol` | `protocol` | 三协议枚举 |
| `ProtocolCodec` | `protocol` | 单协议解析器/抽取器 |
| `NormalizedModelOutput` | `protocol` | 归一化模型输出 |
| `NativeToolCall` / `NativeTranscriptItem` | `protocol` | 原生工具调用/对话条目 |
| `CodecError` | `protocol` | 协议校验错误 |
| `HttpPhasedRuntime` | `http_runtime` | HTTP 相位运行时 |
| `HttpTurnResult` / `HttpTurnStatus` | `http_runtime` | HTTP 轮次结果与状态 |
| `RuntimeError` | `http_runtime` | HTTP 运行时错误 |
| `StreamingEncoder` | `output` | 有状态 SSE 编码器 |
| `PhasedRuntime` | `phased_runtime` | CLI 本地运行时 |
| `TurnResult` / `TurnProgress` | `phased_runtime` | CLI 轮次结果与进度 |
| `RuntimeLimits` | `phased_runtime` | CLI 运行时限制 |
| `ConvertedTurn` | `phased_api_convert` | 请求转换结果 |
| `ApiFormat` | `format` | 三格式枚举（CLI）|

## 7. 构建与测试 / Build & Test

```bash
# 构建 / build
cargo build -p pigs-api

# Lint 检查 / lint
cargo clippy -p pigs-api --all-targets

# 运行测试 / run tests
cargo test -p pigs-api
```

测试套件（55 个测试）：
- 单元测试（21 个）：`phased_markers`、`phased_tools`、`phased_api_convert`、`format`、`continuation`、`http_runtime`。
- 集成测试（34 个）：
  - `orchestration.rs`（5 个）：状态机推进、预算耗尽、失败重规划。
  - `output.rs`（9 个）：三种协议的 JSON/SSE 编码、错误流、增量编码。
  - `protocol_codec.rs`（11 个）：三协议请求校验、相位变更、响应抽取、continuation。
  - `http_runtime.rs`（9 个）：HTTP 运行时完整轮次、工具暂停与恢复。

## 8. 设计决策 / Design Decisions

### 8.1 为什么有两条运行时路径？

- **CLI 路径**（`phased_runtime`）：进程内执行，工具在本地直接运行，适合 CLI / `--once`。
  请求被解构为 `pigs_core::Message` 数组，相位运行时重建消息。
- **HTTP 路径**（`http_runtime`）：协议原生，不解构请求体，适合 `pigs-proxy` 的 `-pig` 路由。
  保留完整的原生 JSON，只定点修改必要字段。

两条路径共享 `orchestration` 状态机，保证相位编排逻辑一致。

### 8.2 为什么 HTTP 路径不解构请求体？

解构会丢失协议特有的字段（如 Anthropic 的 cache_control、Responses 的 reasoning）。
保留原生 JSON 让 pigs-api 能透明支持协议的完整功能集，只修改相位编排必需的三个字段：
model、当前 user 文本、相位对话记录。

### 8.3 为什么 continuation 用有界内存而非持久化？

- 工具调用的恢复窗口很短（通常秒级），不需要持久化。
- 内存存储避免磁盘 I/O 延迟。
- 有界（容量 + TTL）防止内存无限增长。
- 墓碑机制让上游能区分"ID 未知"与"ID 已过期"，便于错误处理。

### 8.4 为什么控制标记要求"原因行"？

单独一行 `PIGEND` 容易被模型误用为逃避任务的捷径。要求前面有非标记的"原因"行，
强制模型在结束前给出完成理由，提高输出的可信度。

## 9. 与其他 crate 的关系 / Relationship to Other Crates

```
pigs-core      ← 核心类型与 trait（Message, ApiClient, ToolHandler, StreamEvent）
pigs-llm       ← LLM 客户端实现（CLI 路径用）
pigs-config    ← 配置管理（AppConfig, ResolvedModel, Language, ApiFormat）
pigs-tools     ← 内置工具注册表（相位运行时复用）
pigs-prompts   ← 相位提示词模板（include_str! 嵌入）
pigs-proxy     ← HTTP 代理（调用 http_runtime，提供 PhaseTransport 实现）
pigs-cli       ← Agent 逻辑（调用 phased_runtime）
```

## 10. 扩展指南 / Extension Guide

### 10.1 添加新协议

1. 在 `protocol.rs` 的 `Protocol` 枚举添加变体。
2. 实现 `validate_*_request`、`trailing_*_tool_results`、`extract_*_response` 函数。
3. 在 `ProtocolCodec` 的 `parse_request` / `extract_response` 分发。
4. 在 `output.rs` 的 `encode_*` / `StreamingEncoder` 添加编码逻辑。
5. 在 `format.rs` 的 `ApiFormat` 添加变体（CLI 路径，可选）。

### 10.2 添加新工具

1. 在 `pigs-tools` 实现 `ToolHandler` trait。
2. 在 `pigs-tools::create_default_registry()` 注册。
3. 相位运行时自动通过 `info_tool_registry()` 获得。

### 10.3 调整相位预算

- **CLI 路径**：修改 `RuntimeLimits`（`phased_runtime.rs`）。
- **HTTP 路径**：修改 `OrchestrationLimits`（`orchestration.rs`），通过 `HttpRuntimeConfig` 传入。

---

> 本文档与源码注释同步维护。源码中每个公开类型、字段和函数都有中英双语注释，
> 关键实现逻辑的每一行和每个变量都有行内注释。
> This document is maintained in sync with source comments. Every public type,
> field, and function has bilingual (Chinese + English) doc-comments; key
> implementation logic has inline comments on every line and variable.
