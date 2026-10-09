# AGENTS.md

## 当前实现契约

本文档描述 **pigs 当前代码实际行为**。在代码尚未修改前，不把“目标设计”或“legacy 行为”写成“已经实现”。

pigs 是一个 Rust 前置代理。普通请求走透传；只有同时满足以下条件时才进入 Pre → Executor → Post 编排：

1. HTTP 方法是 `POST`；
2. 路径能识别为 OpenAI Chat、OpenAI Responses 或 Anthropic Messages；
3. 请求 JSON 的 `model` 以 `-pigs`（模式 A）或 `-pig`（模式 B，兼容旧后缀 `-pigsb`）结尾。

注意：`POST` 到已识别协议路径时，proxy 会先解析 JSON 才能读取 model。因此这类请求即使最终不带 PIGS 后缀，若 JSON 本身非法也会直接返回 400，而不是进入普通透传。

## Pre 提示词按模型路由

进入 PIGS 编排后，proxy 已经剥离 `-pigs` / `-pig` / `-pigsb` 后缀。orchestrator 读取实际上游 `model` 名称，将其转为 ASCII 小写，再按优先顺序匹配：包含 `deepseek` → 完整历史 v5 Pre；否则包含 `muse` → 完整历史 v6 Pre；均未命中 → 从 v5/v6 五问、简单路径和核验共同原则抽象的通用 Pre。无后缀请求仍直接透传，不执行 Pre。匹配是大小写不敏感的**子串**匹配，不要求模型 ID 精确相等。

三组 Pre 均有中文、英文独立模板，语言按最后一条用户问题前 2000 字是否出现 CJK 汉字选取。选中的完整模板再展开 `{failure_paths}`，不在 v5/v6 后附加模型规则；Executor / Post 和 A/B 模式的状态机不变。历史 `pre_user_{zh,en}.txt` 作为旧实现资料保留，正常路由改为使用 `pre_deepseek_*`、`pre_muse_*`、`pre_generic_*`。这些提示词与测试在 Git 中独立版本化。

## 编排请求体

进入编排后，父请求 body 会先解析为 `serde_json::Value`，后续子请求重新序列化。因此编排子请求在语义上保留字段，但**不承诺与客户端原始 JSON 字节级一致**。

当前编排对子请求 body 的业务修改如下：

- 客户端模型名 `<name>-pigs` / `<name>-pig` 都改成 `<name>` 发给上游；回客户端时使用客户端原始模型名。`-pigs` 选择模式 A，`-pig` 选择模式 B。
- Pre / Executor：只在进入该 pig 时，把相位指令追加到当前任务 user 文本；同一 pig 内的工具暂停/恢复不会再次注入相位指令。
- Post：不从最初请求重新构造。Executor 完成后，先保存“Executor 已执行完毕、尚未追加 Post 指令”的完整 checkpoint，再在该 checkpoint 后只追加一条 Post user 指令进行核验。Post 只负责核验和路由，不自行修改任务结果；若输出 `PIGNEXT`，系统丢弃 Post 指令与 Post transcript，回到该 Executor checkpoint，并新增一条与普通 Executor 相同格式的 user 指令，其中 `{pre_output}` 改为 Post 去除控制标记后的核验反馈。
- 其它字段，例如 `tools`、`tool_choice`、`stream`、`temperature`、`max_tokens`、`thinking`、`reasoning`、`response_format`、`stream_options`、`parallel_tool_calls` 等，当前主链路不主动删除或改写。

历史中的工具调用、工具结果、图片和其它非文本块继续保留。每个 pig 持有自己的基础请求与相位内原生对话记录：相位提示只在 pig 开始时注入一次，之后模型输出、工具调用和工具结果按原生顺序追加。Executor → Post 直接继承 Executor 完整 checkpoint；Post → Executor(PIGNEXT) 则先砍回这个 checkpoint，Post 自身的核验提示/轨迹不进入下一次 Executor，只把最终核验反馈作为新的 Executor“执行前分析”输入。

`pigs-protocol` 中仍保留 `set_stream`、`strip_tools` 等通用函数，但当前编排主链路不会调用它们。

## 工具调用与 continuation

`tools` / `tool_choice` 会继续发给上游。若初次 Pre 请求带有非空 `tools` 且未显式设置 `tool_choice = none`，即使 Pre 输出 `PIGEND`，也不会直接走 SIMPLE_PATH；该 Pre 输出会作为计划进入 Executor，避免 Pre 的文本捷径吞掉客户端要求保留的 function-calling 语义。显式 `tool_choice = none` 时仍允许 Pre 简单路径直接结束。上游返回工具调用后：

- 当前 pig 相位暂停；
- 工具调用按三协议各自的原生形状交给客户端执行；ToolCall 只属于当前 Paused 响应，不进入持久 TurnState；
- `ContinuationStore` 在进程内存保存现场，默认最多 64 条，TTL 30 分钟；
- 客户端把工具结果接回历史后再次请求，pigs 会在整份请求中查找当前 pending continuation 所等待的工具结果 id；结果后即使还有 reminder / 普通 user 消息，也仍可恢复；
- 若 id 与某个 continuation 的全部 pending 调用匹配，且 A/B 模式与当前请求一致，则先原子 claim 该现场并继续同一只 pig；A/B 不会互相消费 continuation；
- claim 不会立即删除现场：resume 成功后才 commit 删除；resume 因上游/协议/超时等错误失败时 rollback，解除 in-flight 并保留原 continuation，使客户端重发同一批工具结果仍可恢复；
- 同一 continuation 处于 in-flight 时不会被第二个并发恢复请求再次 claim，也不会被容量/TTL 清理；
- 同一 pig 可以经历任意多轮“工具调用 → 客户端执行 → 工具结果回填”；每次暂停只返回本轮新产生的工具调用，不重放已经消费过的历史调用；
- 工具调用被结果匹配并恢复后即视为已消费；最终 Completed 响应不得再次包含这些历史 ToolCall；
- 有工具结果但找不到匹配现场时返回 HTTP 409，不重新跑整轮；
- 服务重启后 continuation 不恢复。

## 路径、方法与 query

普通透传请求保留客户端 HTTP 方法、原路径与 query string。

编排只会由客户端 `POST` 请求触发。编排子请求经本机 loopback 发送，并统一使用 `POST`；path 与 query string 沿用父请求。

上游 base 按协议选择：

- OpenAI Chat → `upstream.openai`
- OpenAI Responses → `upstream.responses`
- Anthropic Messages → `upstream.anthropic`
- 未识别路径 → `upstream.openai`

最终上游 URL = 对应 base + 客户端原路径 + 原 query string。

## 请求头的当前行为

当前实现并不是“所有请求头逐字不动”。存在明确的传输层处理：

- 转发时跳过 `host`、`content-length`、`connection`、`transfer-encoding`；
- 其它端到端头默认保留，例如 `accept-encoding`、`anthropic-version`、`user-agent`；
- 启动时配置选择优先级为 `config.local.toml` → `config.toml`；若当前生效配置的 `key` 非空，会忽略客户端原有 `authorization` / `x-api-key`，改为同时写入 `authorization: Bearer <key>` 和 `x-api-key: <key>`；
- 编排子请求若缺少 `content-type`，会补 `content-type: application/json`；
- 编排开始时，若客户端没有 `x-opencode-session`，orchestrator 会生成 UUID v7，并在本次编排所有子请求中补上；若客户端已带则继承；
- loopback 子请求额外加入随机 `x-pigs-loopback`，用于让本机 handler 跳过 PIGS 后缀再分流；
- **当前代码没有在进入真实上游前过滤 `x-pigs-loopback`，所以该内部头会继续被转发到上游。**
- loopback 使用默认 reqwest 客户端，具备自动压缩协商/解压行为；当客户端没有 `Accept-Encoding` 时，reqwest 可能自行补压缩协商头。

这些是当前实现事实。如果后续要收紧请求头规则，应修改代码和测试，而不是先把文档写成目标状态。

## 普通透传响应

不带 `-pigs` / `-pig` 的请求走 passthrough：

- 上游状态码保留；
- 响应 body 以字节流方式回传；
- 响应头只跳过 `connection`、`transfer-encoding`、`content-length`；
- `content-encoding` 会保留；
- passthrough 的 reqwest 客户端关闭 gzip / brotli / deflate 自动解压，所以压缩 body 与编码头保持对应。

## 编排响应是重新合成的

带 `-pigs` / `-pig` 的响应都不是上游某一次响应的原样转发，而是 `pigs-protocol` 根据整轮编排产物重新合成。

当前行为：

- `model` 使用客户端原始模型名（包括 `-pigs` 或 `-pig` 后缀）；
- 模式 A（`-pigs`）：普通文本按执行顺序保留，控制标记会被过滤；
- 模式 B（`-pig`）：Pre/Post 普通文本以及被 `PIGNEXT`/`PIGFAIL` 否决的 Executor 候选不会进入客户端业务正文。Simple Path 只提交 Pre 的最终答案；复杂路径通常只在 Post `PIGEND` 后提交当前 Executor candidate；若当前 Executor 已是本轮允许的最后一次执行，则该 Executor 完成后直接提交，不再进入 Post；
- thinking / reasoning、工具调用和其它已解析的原生块按 `Part` 序列尽量保留；
- `stop_reason` / `finish_reason` 取最后一轮解析到的值；
- 响应 id、时间戳、协议壳由 pigs 新生成；
- 非流式走 `synthesize_json`；
- 流式走 `StreamEncoder` 合成 SSE。

因此“编排响应原样透传上游响应”不是当前代码行为。模式 A 与模式 B 当前并存，通过 model 后缀选择；两者共用同一套 Pre/Executor/Post、PIGNEXT、工具 continuation 和核验逻辑，只在普通业务正文的可见性/提交时机上不同。

## usage 的当前策略

`[orchestration].usage_mode` 控制编排响应的 usage 语义，默认 `max`：

- `max`：比较每次真实上游调用的 `total_tokens`，缺失时用主输入字段（`input_tokens` / `prompt_tokens`）+ 主输出字段（`output_tokens` / `completion_tokens`）回退计算；返回总 token 最大的那次调用的**完整原始 usage JSON**。用于普通 coding agent 判断当前上下文占用。
- `sum`：将本次客户端 API 请求实际触发的所有上游调用 usage 按 JSON 路径递归聚合，数值字段逐项相加；只在某次调用出现的字段原样保留。用于评测统计真实模型消耗。

usage 的统计范围是**单次客户端 API 请求**。工具暂停响应也会返回截至该响应的聚合 usage；客户端回填工具结果并恢复 continuation 时，usage 从零开始重新统计，避免 `sum` 重复计费，也避免 `max` 沿用上一发响应的上下文快照。

## 流式行为

客户端 body 中 `stream:true` 时，编排采用流式处理；代码不会主动重写 `stream` 字段。

- 上游 SSE 增量会被解析；
- 模式 A 的文本经过 `MarkerFilter` 后实时转发，避免 `PIGEND` / `PIGNEXT` / `PIGFAIL` 泄给客户端；
- 模式 B 的普通业务文本不在阶段生成时转发：Simple 等 Pre `PIGEND` 后提交，Complex 等 Post `PIGEND` 后提交最后一次被接受的 Executor candidate；因此 `stream:true` 下业务正文是“验收后再以 SSE 发出”，不是 token 生成即提交；
- thinking / reasoning 增量在 A/B 中都按协议实时转发；
- 工具调用以及部分无法边到边还原的原生块在收尾阶段补发；
- 流式收尾由 `StreamEncoder::finish()` 自身保证关闭仍打开的协议块后再发送终止事件；Responses 的 `response.completed.response.output` 因此会包含已经提交的最终正文，不依赖调用方额外先发 `PigEvent::End`；
- 一旦客户端 SSE 已经开始，后续编排错误通过流内错误帧表达，HTTP 状态无法再改成错误码。

客户端未请求流式时，各子请求按整体响应读取，最终合成 JSON。

## 状态机

Pre：

- Pre 提示会要求总结任务目标和会影响结果的关键条件，并把题意不清、多种合理解释或关键条件存在较大不确定性视为难点，供简单/复杂路径判断；
- `PIGEND` 使用严格检测：标记前必须已有非控制标记的实质文本，防止简单路径只有控制词而没有用户答案；满足后通常简单路径结束，但初次 Pre 若存在可用客户端 `tools`（且未显式 `tool_choice = none`），则把 Pre 输出保存为计划并进入 Executor；
- Pre 不维护独立的重规划次数。除简单路径 `PIGEND` 外，`PIGFAIL`、`PIGNEXT` 或无标记输出都作为复杂计划进入 Executor；
- 失败反馈由此前 Post 的 `PIGFAIL` 写入 `failure_paths`，供下一次 Pre 规划使用。

Executor：

- 不解析控制标记；
- 一轮任务的执行次数只按高层 Executor 相位计数；同一 Executor 内因工具调用发生的暂停/恢复不重复计数；
- Executor 执行上限来自 `[orchestration].max_executor_runs`，默认 4；最后一次允许的 Executor 完成后直接结束，不再进入 Post；
- 尚未达到执行次数上限时，Executor 结束后进入 Post。

Post：

- Post 是纯核验器：可以用工具核验，但不自行继续执行、修改或重写任务结果；
- `PIGEND` → 接受当前 Executor 结果并正常完成；仅输出一行 `PIGEND` 也合法；
- `PIGNEXT` → 当前结果可继续修补；Post 应给出简短可执行反馈。系统砍回最近一次 Executor 完成时保存的 checkpoint，丢弃 Post 指令与 Post transcript，再用原 Executor 指令模板把 Post 反馈作为新的“执行前分析”交给下一次 Executor；
- `PIGFAIL` → 当前执行路径需要重新规划，记录失败反馈并回 Pre，由 Pre 为下一次 Executor 形成新计划；
- `PIGNEXT` 与 `PIGFAIL→Pre→Executor` 不再维护各自的次数预算，二者统一消耗同一个 Executor 执行次数上限；
- 无标记 → 视为核验器协议未完成，留在 Post 做协议重试；重试上限来自 `[orchestration].max_post_protocol_retries`，默认 3，这个协议重试计数不属于任务执行次数。

任何相位只要上游停止原因明确表示生成 token 上限截断（当前识别 OpenAI Chat 的 `length`、Anthropic 的 `max_tokens`、Responses 的 `max_output_tokens`），就返回截断错误，不把部分文本当作完整 Pre/Executor/Post 产出继续推进。

任务执行预算不再按 `PIGNEXT` 修补次数或 `PIGFAIL` 重规划次数分别统计，而只按 Executor 实际执行次数统计。达到 Executor 执行上限时，最后一次 Executor 直接结束，因此不会再因为“最后一个 Post 判定不通过”而产生 422。Post 无控制标记的协议重试若单独超限，仍返回预算错误；proxy 对这类预算错误返回 422，其它编排错误（包括 token 截断）通常返回 502。

## 配置与职责

启动时若根目录存在 `config.local.toml`，会优先读取它；否则读取 `config.toml`。两者都不存在时才生成默认 `config.toml`。`config.local.toml` 用于本机私有配置（例如固定上游 API key），并被 Git 忽略。

根 `config.toml` 仍是公开默认模板，当前包含：

- `listen`
- 可选 `key`
- `[logging].detail`：`off` / `basic` / `max`，当前测试阶段默认 `max`
- `[logging].directory`：HTTP 抓包目录，默认 `logs/http`
- `[orchestration].max_executor_runs`：Executor 高层执行次数上限，默认 `4`
- `[orchestration].max_post_protocol_retries`：Post 无控制标记时的协议重试次数上限，默认 `3`
- `[orchestration].usage_mode`：`max` / `sum`，默认 `max`
- `[upstream].openai`
- `[upstream].responses`
- `[upstream].anthropic`

`--base-url` 会临时把三个协议 base 都设置成同一个地址；`--log-detail` / `--log-dir` 可覆盖 HTTP 诊断日志配置。

## HTTP 诊断抓包

普通 tracing 日志仍输出控制台和 `logs/pigs.log.<日期>`，级别由 `RUST_LOG` 控制。除此之外，proxy 还有独立的 HTTP 抓包日志：

- `off`：不生成抓包文件；
- `basic`：每个请求/响应单独生成文件，只记录交换编号、方向、方法/状态、目标、头和 body 字节数；
- `max`：在 basic 基础上额外记录完整 body。当前内部测试阶段缺省即为 `max`；
- 文件名使用同一 exchange id 关联一组事件，例如 `client-request` / `client-response`、内部 loopback 的 `internal-request` / `internal-response`、以及 `upstream-request` / `upstream-response`；编排请求还会额外生成 `orchestration-decision` 与 `orchestration-outcome`；
- 流式响应会完整捕获实际经过 proxy 的 SSE；正常读到流末尾写 `capture_complete: true`，若连接/Body 在中途被丢弃则写 `false`；
- gzip / deflate / brotli 响应在写日志时尽量解压成明文，不改变实际转发字节；
- `authorization`、`x-api-key`、cookie、内部 loopback token 等敏感头会自动打码，query 中常见 key/token/secret/auth/password 参数也会打码；
- `orchestration-decision` 会记录客户端会话、请求中全部工具结果 id、尾部连续工具结果 id、当时所有 pending continuation（id / phase / pending tool ids / age / in_flight）、实际命中的工具结果以及最终路由判定；尾部集合只用于诊断，不再决定是否恢复；`orchestration-outcome` 会记录完成或暂停、continuation id 和本轮 tool call ids；
- **请求/响应 body 不做语义脱敏**，因此 `max` 日志可能包含用户 prompt、工具结果和模型输出，只适合受控测试环境。

crate 职责：

- `pigs`：CLI、配置加载、日志、启动；
- `pigs-proxy`：HTTP 入口、分流、透传、loopback、客户端响应流；
- `pigs-orchestrator`：Pre / Executor / Post 状态机、continuation；
- `pigs-protocol`：协议判定、JSON/SSE 解析、请求体尾部手术、响应合成。

`pigs-mini-agent/` 是独立 Git / Cargo 项目，不属于根 workspace。

## 历史实现与文档同步

`legacy/` 是历史参考实现，不再作为“当前行为必须逐字一致”的权威。文档与当前代码冲突时，以当前代码为事实基线；若决定改变行为，应先明确新契约，再修改代码和测试。

修改实现后，应同步更新本文件、根 README 中英双语文件、相关 evaluation 中英双语文件和 5 个 HTML 说明页，避免设计说明与实际代码再次分叉。

## 语言约定

- 面向项目使用者的主文档优先采用英文主文件，并使用独立 `_CH.md` 文件维护中文版本；根目录 `README.md` / `README_CH.md` 与 `evaluation/` 文档遵循这一规则；
- 本 `AGENTS.md` 是项目内部实现契约，目前继续使用中文；
- 当前 `crates/` 源码仍存在大量中文注释，这是代码现状；
- 文档修改不能替代真实代码行为，描述必须以当前实现为基线。
<!-- ARIS:BEGIN -->
## ARIS Skill Scope (ZCode)
ARIS skills installed in this project: 84 entries.
Manifest: `.aris/installed-skills.txt` (lists every skill and its upstream target).
For ARIS workflows, prefer the project-local skills under `.zcode/skills/` over global skills.
Reviewer routing: under ZCode, always prefer `Task(agent_type: gpt-reviewer)` (model and reasoning already configured) over Codex MCP; use Codex MCP only when the user explicitly requests it or when `gpt-reviewer` is unavailable.
Skill execution (ZCode): always invoke skills via the Skill tool (`/research-pipeline`, `/idea-discovery`, ...); reading SKILL.md with Read is for inspection only and never substitutes for invocation. Do not re-implement a skill's workflow by hand from its prose.
Long-run rule (ZCode-only): any task expected to exceed ~10 minutes (e.g. model training, large sweeps) MUST run via `Bash(run_in_background: true)`; either rely on the tool's persisted output log or redirect stdout/stderr to a `logs/` file yourself — never leave output only in the live session. Every log line MUST carry a wall-clock timestamp precise to the second (`%Y-%m-%d %H:%M:%S`); the main agent judges background-task state by reading the log tail, not by the session being alive.
Do not delete skill directories wholesale; edit individual SKILL.md files in place (they are hard copies owned by this project, originally from `D:\AIWorkSpace\Auto-zcode-research-in-sleep\Auto-claude-code-research-in-sleep`).
Update with: `python D:\AIWorkSpace\Auto-zcode-research-in-sleep\init.py --reconcile`  (re-runnable; reconciles new/removed skills).
<!-- ARIS:END -->
