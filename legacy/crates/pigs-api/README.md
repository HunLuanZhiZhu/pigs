# pigs-api

`pigs-api` 是协议原生的相位运行时库。HTTP 服务器位于 `pigs-proxy`；本 crate 不依赖 axum 或 pigs-proxy，通过 transport trait 保持依赖单向。

## HTTP 数据流

```text
完整 HttpRequestEnvelope
  ├─ method + path/query + raw header values
  ├─ OpenAI Chat / Anthropic Messages / OpenAI Responses
  └─ 完整 JSON body
          │
          ▼
HttpPhasedRuntime
  Pre -> Executor -> Post
                    ├─ PIGEND  -> 完成
                    ├─ PIGFAIL -> Pre
                    └─ 无标记  -> Post
          │
          ▼
PhaseTransport（由 pigs-proxy 实现 HTTP loopback）
```

每个相位 clone 原始 body，只修改真实 model、当前用户输入和文档要求的相位轨迹。system/instructions、历史消息、tools/tool_choice、媒体块、推理/缓存/metadata 和未知扩展字段保持协议原生结构。

## 核心模块

- `protocol`：`HttpRequestEnvelope`、三协议 codec、原生 transcript/tool-call/tool-result 提取。
- `orchestration`：纯状态机与预算错误；只有有效 `PIGEND` 能产生成功终态。
- `http_runtime`：协议原生 Pre→Executor→Post 执行、连续文本聚合、工具暂停/恢复。
- `continuation`：有 TTL、容量上限和有界墓碑的进程内存储，不写磁盘。
- `transport`：异步 `PhaseTransport`，支持完整响应和流式文本 delta。
- `output`：三协议非流式 JSON 与合法 SSE 编码；错误流不发送成功结束帧。
- `phased_markers`：只让最后一个有效非空行控制 `PIGEND/PIGFAIL`；对外剥离控制行。
- `phased_runtime` / `phased_api_convert`：保留给 CLI 的本地 `ApiClient` 路径，不用于 HTTP `-pig` 请求。

## 工具语义

HTTP 请求中的 tools 由上游 Agent 执行。模型返回工具调用时，运行时按入口协议原样返回并保存 continuation；下一次带原生 tool result 的 `-pig` 请求按 tool-call ID 恢复同一相位。未知、过期、淘汰、重复消费、错模型或错协议会返回明确错误。

## 流式语义

内部相位请求可使用供应商 SSE。末行缓冲器立即转发已确定不是控制标记的文本，仅保留可能含 `PIGEND/PIGFAIL` 的最后有效行。外层按入口协议生成 Chat、Anthropic 或 Responses 事件序列，并聚合所有相位文本与 usage。
