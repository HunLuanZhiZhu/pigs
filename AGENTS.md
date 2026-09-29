# AGENTS.md

## 最高准则（不可协商，优先级高于本文件其它一切内容）

**pigs 相对上游只允许做两处改动：① 模型名称；② 提示词的后缀拼接。**

1. **模型名称**：客户端请求的 `-pig` 模型名 → 换成上游真名发给上游；**回给客户端时用客户端原本请求的那个名字**（带 `-pig`）。
2. **提示词后缀拼接**：在客户端原始请求的**尾部追加**相位提示词——Pre/Executor 追加到最后一条 user 消息的尾部；
   Post 追加产物消息 + 新的 user 指令。**必须追加在尾部**，这样才能保住上游 prompt cache 的前缀命中（缓存率 = 成本）。

**除此之外，父请求的任何部分都不得改动、不得删除、不得新增**，包括但不限于：

- **body 的字段一个都不许动**：`tools`、`tool_choice`、`stream`、`temperature`、`max_tokens`、`thinking`、`reasoning`、
  `response_format`、`stream_options`、`parallel_tool_calls`、`n`…… 原样透传；
- **历史消息原样保留**：包含 assistant 的工具调用、`role: "tool"` 消息、Anthropic 的 `tool_use`/`tool_result` 块、
  图片等非文本块——既不许删，也不许"合并""改写"；
- **path 与 query string、HTTP 方法**原样带到上游；
- **请求头一个都不许删、不许加**：鉴权、`anthropic-beta`、`accept-encoding`、`user-agent`…… 全部原样；
- **响应侧同样不许加工**：上游给什么就回什么——不许改 model 名、不许把 `usage` 清零、不许丢弃工具调用
  （`tool_calls`/`tool_use`/`function_call`）、不许改写 `finish_reason`/`stop_reason`、不许丢非文本块。

**判定方法**：任何一处改动，先自问"这是**模型名**，还是**提示词后缀**？"——答不上来就不许做。

**推论（不是选项，是准则的一部分）**：工具的完整链路必须原样可用——`tools` 透传给上游；上游回的工具调用**原样交给客户端**执行；
客户端带工具结果回来时，**接着同一只 pig（相位）继续**，而不是重开一轮。一只 pig 是一段对话区间，
结束条件是"模型这一轮不再要工具"，中间可以包含任意多次"要工具 → 客户端执行 → 结果回填"的往返。

## 参考实现

`legacy/`（旧工程）是**参考实现**。它相对上游只覆盖 model/stream 与用户文本后缀、并为 Post 接回产物消息，
同时完整保留了 tools、`path_and_query`、真实 usage、客户端 model 名回显、以及工具调用的 continuation 机制。
新实现凡与之不符之处，一律以 legacy 为准逐条审计。

## 当前代码与此准则的差距（逐条待裁决，裁决后修正代码）

| # | 事项 | 状态 |
|---|---|---|
| 1 | `tools` / `tool_choice` 透传，不再删除；上游回的工具调用原样交给客户端；客户端带结果回来接着同一只 pig 继续（`state::Continuation`） | ✅ 已改 |
| 2 | 回客户端的 `model` 用客户端请求的原名（带 `-pig`），发给上游用真名 | ✅ 已改 |
| 3 | `usage` 跨相位累加后原样回传（上游没给才是空对象），不再清零 | ✅ 已改 |
| 4 | `accept-encoding` 头原样透传，改为**本机解压**（reqwest 开 `gzip/brotli/deflate`；透传客户端 `.gzip(false)...` 保持逐字节） | ✅ 已改 |
| 5 | 完全不写 `stream` 字段 | ✅ 已改 |
| 6 | 客户端没带 `x-opencode-session` 时注入一个（补头归 mini-proxy 才是终态） | ⏸ 按裁决暂缓 |
| 7 | `path` + `query` 一起带进子请求 | ✅ 已改 |
| 8 | 工具调用原样回吐（三协议，JSON 与 SSE 都覆盖） | ✅ 已改 |
| 9 | `finish_reason` / `stop_reason` 透传上游的真实值（以最后一轮为准） | ✅ 已改 |
| 10 | 相位产物**逐条**追加，不再合并成一条 assistant 消息 | ✅ 已改 |

## 遗留细节（已登记，未改）

- **解压的副作用**：reqwest 在请求没带 `Accept-Encoding` 时会补一个 `gzip`（它的默认行为，为本地解压服务）。
  客户端带了该头时不影响；客户端没带时这是相对父请求的一处**库级添加**。
- 尾部工具结果匹配不到现场时返回 **409**（与 legacy 的 `UnknownContinuation` 一致），不悄悄重跑一整轮。
- 工具暂停时回给客户端的 `usage` 是"到目前为止累加值"（legacy 同款），最终答复给的是全量累加值。

## 语言约定

- **代码注释用英文**、**文档用中文**（与 legacy `AGENTS.md` 一致；本仓库当前的 `index.html` 是中文详解，供审阅）。
- 代码注释语言若与本条不符，属待修项（当前 `crates/` 下注释为中文）。
