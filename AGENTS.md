# AGENTS.md

## 当前实现契约

本文档描述 **pigs 当前代码实际行为**。在代码尚未修改前，不把“目标设计”或“legacy 行为”写成“已经实现”。

pigs 是一个 Rust 前置代理。普通请求走透传；只有同时满足以下条件时才进入 Pre → Executor → Post 编排：

1. HTTP 方法是 `POST`；
2. 路径能识别为 OpenAI Chat、OpenAI Responses 或 Anthropic Messages；
3. 请求 JSON 的 `model` 以 `-pigs` 结尾。

注意：`POST` 到已识别协议路径时，proxy 会先解析 JSON 才能读取 model。因此这类请求即使最终不带 `-pigs`，若 JSON 本身非法也会直接返回 400，而不是进入普通透传。

## 编排请求体

进入编排后，父请求 body 会先解析为 `serde_json::Value`，后续子请求重新序列化。因此编排子请求在语义上保留字段，但**不承诺与客户端原始 JSON 字节级一致**。

当前编排对子请求 body 的业务修改如下：

- 客户端模型名 `<name>-pigs` 改成 `<name>` 发给上游；回客户端时使用客户端原始模型名。
- Pre / Executor：只在进入该 pig 时，把相位指令追加到当前任务 user 文本；同一 pig 内的工具暂停/恢复不会再次注入相位指令。
- Post：不从最初请求重新构造。Executor 完成后，把 Executor 的基础请求与完整 `phase_transcript` 物化成连续对话，再只追加一条 Post 的 user 指令；因此 Post 以前一发 Executor 请求为完整消息前缀，并额外包含 Executor 最终 assistant 输出。Post 内继续执行时复用这一基础现场。
- 其它字段，例如 `tools`、`tool_choice`、`stream`、`temperature`、`max_tokens`、`thinking`、`reasoning`、`response_format`、`stream_options`、`parallel_tool_calls` 等，当前主链路不主动删除或改写。

历史中的工具调用、工具结果、图片和其它非文本块继续保留。每个 pig 持有自己的基础请求与相位内原生对话记录：相位提示只在 pig 开始时注入一次，之后模型输出、工具调用和工具结果按原生顺序追加。Executor → Post 是特例：Post 直接继承 Executor 已形成的完整协议上下文，以保持长前缀稳定、减少重复读取和前缀缓存损失。

`pigs-protocol` 中仍保留 `set_stream`、`strip_tools` 等通用函数，但当前编排主链路不会调用它们。

## 工具调用与 continuation

`tools` / `tool_choice` 会继续发给上游。上游返回工具调用后：

- 当前 pig 相位暂停；
- 工具调用按三协议各自的原生形状交给客户端执行；ToolCall 只属于当前 Paused 响应，不进入持久 TurnState；
- `ContinuationStore` 在进程内存保存现场，默认最多 64 条，TTL 30 分钟；
- 客户端把工具结果接回历史后再次请求，pigs 会在整份请求中查找当前 pending continuation 所等待的工具结果 id；结果后即使还有 reminder / 普通 user 消息，也仍可恢复；
- 若 id 与某个 continuation 的全部 pending 调用匹配，则取出该现场并继续同一只 pig；
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
- 若 `config.toml` 的 `key` 非空，会忽略客户端原有 `authorization` / `x-api-key`，改为同时写入 `authorization: Bearer <key>` 和 `x-api-key: <key>`；
- 编排子请求若缺少 `content-type`，会补 `content-type: application/json`；
- 编排开始时，若客户端没有 `x-opencode-session`，orchestrator 会生成 UUID v7，并在本次编排所有子请求中补上；若客户端已带则继承；
- loopback 子请求额外加入随机 `x-pigs-loopback`，用于让本机 handler 跳过 `-pigs` 再分流；
- **当前代码没有在进入真实上游前过滤 `x-pigs-loopback`，所以该内部头会继续被转发到上游。**
- loopback 使用默认 reqwest 客户端，具备自动压缩协商/解压行为；当客户端没有 `Accept-Encoding` 时，reqwest 可能自行补压缩协商头。

这些是当前实现事实。如果后续要收紧请求头规则，应修改代码和测试，而不是先把文档写成目标状态。

## 普通透传响应

不带 `-pigs` 的请求走 passthrough：

- 上游状态码保留；
- 响应 body 以字节流方式回传；
- 响应头只跳过 `connection`、`transfer-encoding`、`content-length`；
- `content-encoding` 会保留；
- passthrough 的 reqwest 客户端关闭 gzip / brotli / deflate 自动解压，所以压缩 body 与编码头保持对应。

## 编排响应是重新合成的

带 `-pigs` 的响应不是上游某一次响应的原样转发，而是 `pigs-protocol` 根据整轮编排产物重新合成。

当前行为：

- `model` 使用客户端原始的 `-pigs` 名称；
- 文本按执行顺序保留，控制标记 `PIGEND` / `PIGFAIL` 会被过滤；
- thinking / reasoning、工具调用和其它已解析的原生块按 `Part` 序列尽量保留；
- `stop_reason` / `finish_reason` 取最后一轮解析到的值；
- 响应 id、时间戳、协议壳由 pigs 新生成；
- 非流式走 `synthesize_json`；
- 流式走 `StreamEncoder` 合成 SSE。

因此“编排响应原样透传上游响应”不是当前代码行为。

## usage 的当前策略

当前代码**不做跨相位 token 累加**。

`TurnState::record_round` 会比较每个子请求的 `usage.input_tokens`：

- 没有历史 usage 时直接保存；
- 新 usage 的 `input_tokens` 更大时，用该 **完整 usage 对象原值** 替换；
- 相等或更小时保留已有对象。

最终完成响应使用这个被选中的 usage 对象。

注意：OpenAI Chat 常见字段名是 `prompt_tokens`，当前选择逻辑只读取 `input_tokens`。因此这类 usage 若没有 `input_tokens`，比较值会按 0 处理，通常会保留最先记录到的完整 usage 对象。

工具调用导致暂停时，proxy 在构造暂停响应时传入 `usage: None`。因此工具暂停响应不会返回“截至目前的累计 usage”。

## 流式行为

客户端 body 中 `stream:true` 时，编排采用流式处理；代码不会主动重写 `stream` 字段。

- 上游 SSE 增量会被解析；
- 文本经过 `MarkerFilter`，避免 `PIGEND` / `PIGFAIL` 泄给客户端；
- thinking / reasoning 增量按协议实时转发；
- 工具调用以及部分无法边到边还原的原生块在收尾阶段补发；
- 一旦客户端 SSE 已经开始，后续编排错误通过流内错误帧表达，HTTP 状态无法再改成错误码。

客户端未请求流式时，各子请求按整体响应读取，最终合成 JSON。

## 状态机

Pre：

- `PIGEND` → 简单路径结束；
- `PIGFAIL` → 记录失败路径并留在 Pre，最多重规划 2 次；
- 无标记 → 进入 Executor。

Executor：

- 不解析控制标记；
- 结束后进入 Post。

Post：

- `PIGEND` → 正常完成；
- `PIGFAIL` → 回 Pre 重规划；
- 无标记 → 继续 Post，最多重试 3 次。

预算耗尽返回编排错误。proxy 对预算错误返回 422；其它编排错误通常返回 502。

## 配置与职责

根 `config.toml` 当前包含：

- `listen`
- 可选 `key`
- `[logging].detail`：`off` / `basic` / `max`，当前测试阶段默认 `max`
- `[logging].directory`：HTTP 抓包目录，默认 `logs/http`
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
- `orchestration-decision` 会记录客户端会话、请求中全部工具结果 id、尾部连续工具结果 id、当时所有 pending continuation（id / phase / pending tool ids / age）、实际命中的工具结果以及最终路由判定；尾部集合只用于诊断，不再决定是否恢复；`orchestration-outcome` 会记录完成或暂停、continuation id 和本轮 tool call ids；
- **请求/响应 body 不做语义脱敏**，因此 `max` 日志可能包含用户 prompt、工具结果和模型输出，只适合受控测试环境。

crate 职责：

- `pigs`：CLI、配置加载、日志、启动；
- `pigs-proxy`：HTTP 入口、分流、透传、loopback、客户端响应流；
- `pigs-orchestrator`：Pre / Executor / Post 状态机、continuation；
- `pigs-protocol`：协议判定、JSON/SSE 解析、请求体尾部手术、响应合成。

`pigs-mini-agent/` 是独立 Git / Cargo 项目，不属于根 workspace。

## 历史实现与文档同步

`legacy/` 是历史参考实现，不再作为“当前行为必须逐字一致”的权威。文档与当前代码冲突时，以当前代码为事实基线；若决定改变行为，应先明确新契约，再修改代码和测试。

修改实现后，应同步更新本文件和 5 个 HTML 说明页，避免设计说明与实际代码再次分叉。

## 语言约定

- 文档使用中文；
- 当前 `crates/` 源码仍存在大量中文注释，这是代码现状；
- 本轮只更新文档，不以文档修改替代代码整改。
