pigs-api 重构实施计划
目标与验收口径
按照 docs/重构pigs-api.md [blocked] 重构 API 链路，并落实本次确认的产品语义：

-pig 请求以完整的协议原生 HTTP 语义进入相位层：方法、路径/查询、必要 headers、完整 JSON 字段均保留；允许 JSON 重新序列化，不要求字节、空白或字段顺序不变。
相位层仅定点修改 model、当前用户输入和文档明确要求加入的相位轨迹；原始 system、历史消息、tools/tool_choice、推理参数、缓存字段、metadata、媒体块及未知扩展字段保持不变。
三协议均原生支持：OpenAI Chat Completions、Anthropic Messages、OpenAI Responses，输入什么协议就输出什么协议。
相位请求去掉 -pig 后通过本机 HTTP 重新进入 pigs-proxy，复用其渠道选择、模型映射、body 清洗、思考强度注入和重试，不再由 HTTP API 路径使用有损的 ConvertedTurn -> ApiRequest -> OpenAI Chat 进程内转换。
状态机严格为：Pre -> Executor -> Post；Post PIGEND -> 完成、PIGFAIL -> Pre、无标记 -> Post。只有完成标记能产生成功终态，预算耗尽返回明确错误，不能伪装成成功响应。
标记统一为 PIGEND / PIGFAIL，仅最后一个有效非空行可控制路由；标记不对外输出，PIGEND 前必须有理由/正文。
Pre/Executor 使用“原始用户问题 + 文档相位后缀”；Post 使用本次确认的独立验收提示。Post 输入保留完整 Executor 轨迹和此前所有 Post 轨迹；失败重规划保留编号失败记录。
原请求工具由上游 Agent 执行。模型返回工具调用时，相位运行时暂停并以入口协议原样返回工具调用；下一次带工具结果的 -pig 请求按 tool-call ID 恢复相位。
continuation 使用有 TTL 和容量上限的进程内存储，不写磁盘；重启、过期、错协议/错模型或未知 tool-call ID 返回明确错误。
流式和非流式响应都按入口协议合法编码；Pre、Executor、每次 Post 的可见文本按执行顺序连续输出，控制标记隐藏。流中失败发送协议原生 error/failed 事件，不能发送成功结束帧。
实施步骤
1. 建立基线与协议原生请求模型
在当前 main 工作区实施，这是用户明确选择；不创建 worktree，不触碰未跟踪参考项目。
先运行 cargo test --workspace 建立基线；若已有失败，先报告并区分既有问题。
在 pigs-api 中以传输无关类型建立 HttpRequestEnvelope：协议、方法、path/query、header 字节对、完整 serde_json::Value body。
将现有 format.rs / phased_api_convert.rs 的有损统一消息转换替换为三个协议 codec。codec 负责：
验证真实的最后 user 输入，而不是“最后非 system 消息”；
支持 string 和结构化 content，保留图片、文档、thinking、tool history 和未知块；
OpenAI Responses 同时支持 string input 和数组 input；
clone 原 body 后只修改指定 JSON 位置；
追加/读取协议原生 assistant、tool call、tool result 轨迹。
添加深比较测试：三个协议的 tools、system/history、媒体块、推理/缓存/metadata/未知字段均保持结构相等，只允许预期字段变化。
2. 重写提示词、标记与纯状态机
更新 pigs-prompts 的中英文 user payload。中文严格采用重构文档的五个 Pre 问题、Executor 说明和 Post 验收文本；英文提供语义等价版本。
相位指令只进入 user payload，用户 system 永不替换或拼接；不发送当前死代码中的“Pigs phase identity” system prompt。
Pre 失败段按“第 1 次失败…第 n 次失败…”编号并保留完整失败输出。
将 PIGFAILED 全面迁移为 PIGFAIL；检测器只接受最终有效行，并要求结束标记前有正文。清理器隐藏所有控制标记行但不误删普通包含词。
提取共享的纯 orchestration 状态与转移函数，使 HTTP 原生运行时和 CLI 本地运行时使用同一规则。
单元测试覆盖每条边：Pre 简单结束、Pre 到 Executor、Executor 到 Post、Post 完成、Post 失败重规划、Post 无标记再次 Post、重复 Post 轨迹保留、多次失败记录、预算耗尽为错误。
3. 实现协议原生相位执行与 HTTP loopback transport
在 pigs-api 定义异步 PhaseTransport trait 和规范化的模型事件/响应类型；pigs-api 不依赖 pigs-proxy 或 axum，避免循环依赖。
在 pigs-proxy 新增 LoopbackPhaseTransport，使用配置的监听地址构造本机 URL；对 0.0.0.0 / [::] 转为可连接的 loopback 地址。
每个相位 subrequest 保留原方法、path/query、端到端 headers 和完整 body，去掉一层 -pig；加入仅供路由识别的内部 header，handler 验证后直接进入 passthrough 分支，确保不会递归进入相位层，并在发往供应商前移除该 header。
passthrough 继续执行现有模型映射、清洗、思考强度注入和 retry::dispatch。入口协议动态决定 endpoint，不再固定 OpenAI Chat。
transport 同时解析三协议非流式 JSON 与 SSE，保留完整模型轨迹、文本、tool calls、usage 和终止原因；上游非成功状态和 malformed stream 转为类型化错误。
使用本地 mock provider 和临时端口做集成测试，证明相位 subrequest 确实经过 HTTP handler，并且模型无后缀、协议/路径/headers/未知 body 字段正确到达 mock provider。
4. 实现外部工具暂停与内存 continuation
新建有界 ContinuationStore，使用内部 continuation ID 管理状态，并将每个 pending tool-call ID 映射到同一 continuation；保存原始请求语义、当前相位、Pre 产物、完整 Executor/Post 轨迹、失败记录和待完成工具集合，但不持久化认证 headers。
新请求进入时先扫描协议原生 tool results：
命中有效 continuation 时，使用当前请求 headers 恢复；
校验协议、真实模型和 pending IDs；
支持一轮多个并行工具结果，缺失结果不提前恢复；
成功恢复后移除已消费映射，完成时彻底清理；
过期、容量淘汰、重复消费或未知 ID 返回明确的 409/4xx 协议错误。
三协议分别原生返回工具调用：OpenAI Chat tool_calls、Anthropic tool_use、Responses function_call，并使用相应 stop reason/status。
使用可控时钟/小容量测试 TTL、淘汰、并发恢复、重复请求、错模型/错协议和多工具结果。
5. 重建连续响应与合法 SSE
用统一的 OutputComposer 累积所有可见相位文本，按相位边界加入稳定的换行，不添加控制标记。
流式路径使用“末行缓冲器”：已确定不是控制标记的文本立即发送，最后一行在 phase 完成后检测/剥离，避免 PIGEND / PIGFAIL 泄露。
非流式响应包含到当前暂停/完成点的全部相位文本，并聚合实际 usage；模型名使用客户端请求的 X-pig。
为三协议实现完整合法序列：
Chat：role/content/tool-call chunks、finish chunk、[DONE]；
Anthropic：message/content-block start/delta/stop、message delta/stop；
Responses：response/output-item/content-part/text delta/done/completed 或 failed。
工具暂停流以工具终止原因结束并保存 continuation；运行时错误流只发 error/failed，不再无条件发 success stop。
添加逐帧解析测试，验证事件顺序、索引/ID/sequence、JSON 可解析、相位顺序、marker 不泄露、错误终止和工具暂停/恢复。
6. 保持 CLI 能力并迁移产品接线
HTTP -pig 路径切换到新的协议原生 HttpPhasedRuntime；AppState、serve、build_phased_runtime 和 pigs 产品入口相应迁移。
ProxyApiClient / dispatch_in_process 继续服务 CLI 的本地 Agent 请求，不再服务 HTTP 相位请求；这样 CLI 仍能在进程内执行 bash/file/MCP 工具。
重构现有本地 PhasedRuntime::run_phase，返回完整 assistant/tool transcript 而不只是最后文本，并应用共享状态机：markerless Post 留在 Post，PIGFAIL 回 Pre，Post 获得 Executor 与历次 Post 完整轨迹。
修正 CLI 流式展示与会话最终文本，确保连续相位输出和最终结果不重复打印。
移除或停止导出仅为旧 HTTP 转换服务的 ConvertedTurn API；修复 workspace 内所有引用并保持必要的 CLI re-export。
7. 文档、审查与最终验证
更新 README.md、AGENTS.md、crates/pigs-api/README.md、crates/pigs-proxy/README.md、crates/pigs/docs/理解与规划.md 和产品 --describe 文案：新 HTTP loopback、三协议原生请求、continuation、PIGFAIL、Post 自循环；保留说明 CLI 仍使用进程内 transport。
在 docs/重构pigs-api.md 追加本次已确认的实施决策，不改写原始需求正文。
每个实施批次依次进行规格符合性和代码质量审查，发现问题后修复并复审。
最终运行：cargo fmt --check、cargo build --workspace、cargo test --workspace、cargo clippy --workspace --all-targets。
完成审计逐条映射文档行 15-29、31-35、36-60、61-74、76-86 及本次四项选择（语义完整、上游工具、连续全部输出、内存有界 continuation）到源码、单元测试和端到端测试证据；任何未覆盖项不以“测试全绿”替代。
预期主要文件变化
crates/pigs-api/src/：新增请求信封、三协议 codec、纯状态机、HTTP runtime、continuation、输出/SSE 组合器；重写 markers/runtime，淘汰有损 HTTP conversion。
crates/pigs-prompts/prompts/ 与 src/lib.rs：精确相位 user payload 与快照测试。
crates/pigs-proxy/src/server.rs、lib.rs、新增 loopback transport：完整 HTTP 接入、内部重入、协议动态调度。
crates/pigs/src/main.rs 与 crates/pigs-cli/src/agent.rs：产品/CLI 接线和共享状态语义。
对应 crate 测试及项目中文文档。