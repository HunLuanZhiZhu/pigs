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

| # | 现状 | 与准则的冲突 | 合规改法 |
|---|---|---|---|
| 1 | 子请求删掉 `tools` + `tool_choice`（`crates/pigs-protocol/src/surgery.rs:44`，`crates/pigs-orchestrator/src/lib.rs:325` 每只 pig 都调） | 直接违反 | 不删；上游回的工具调用原样交给客户端；恢复 continuation 以接着同一相位 |
| 2 | 响应里的 model 名回的是剥掉后缀的真名（`crates/pigs-proxy/src/server.rs:134,158`） | 违反 | 回客户端请求的原名（legacy `pigs-proxy/src/server.rs:89,113,129,187,199`） |
| 3 | 合成响应的 `usage` 全是 0 / 空对象（`crates/pigs-protocol/src/response.rs:220,381,394,527,537,550`） | 违反 | 跨相位累加真实 usage（legacy `pigs-api/src/output.rs` 用 `result.usage`） |
| 4 | 子请求过滤掉 `accept-encoding` 头（`crates/pigs-orchestrator/src/lib.rs` `call_pig`） | 违反 | 头原样透传，改为**本地解压**：reqwest 开启 `gzip`/`brotli`/`deflate`，子请求客户端自动解压，透传客户端用 `.gzip(false)...` 保持逐字节 |
| 5 | `stream` 字段被显式覆盖（`set_stream`），取值恰好等于客户端的选择 | 写法上违反（效果等价） | 完全不写该字段：客户端要流式就流式读、边读边转 |
| 6 | 客户端没带 `x-opencode-session` 时编排层主动注入（`crates/pigs-orchestrator/src/lib.rs`） | 违反（凭空加头） | 不注入；客户端带了的随请求头自动透传；补头归 mini-proxy |
| 7 | 子请求丢失 query string（`TurnInput`/`SubRequest` 只有 path；legacy 存 `path_and_query`，`legacy/crates/pigs-api/src/protocol.rs:112`） | 违反 | 把 path+query 一起带进子请求 |
| 8 | 工具调用在响应提取时被丢弃（`crates/pigs-protocol/src/response.rs` 只认文本块/只认 `message` item） | 违反 | 原样回吐工具调用（三协议） |
| 9 | `finish_reason`/`stop_reason` 固定合成（上游因 `max_tokens` 截断也说 `stop`） | 违反 | 以最后一次上游响应的真实值回吐 |
| 10 | 把多段相位产物**合并成一条** assistant 消息（`crates/pigs-orchestrator/src/lib.rs` `transcript.join("\n\n")`） | 违反（形状被加工；legacy 逐条追加） | 回退为逐条追加，与 legacy 一致 |

## 语言约定

- **代码注释用英文**、**文档用中文**（与 legacy `AGENTS.md` 一致；本仓库当前的 `index.html` 是中文详解，供审阅）。
- 代码注释语言若与本条不符，属待修项（当前 `crates/` 下注释为中文）。
