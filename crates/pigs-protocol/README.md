# pigs-protocol

三协议的**公共词汇层**：OpenAI Chat Completions / Anthropic Messages / OpenAI Responses 的消息模型与编解码。
全仓库最底层 crate，被 `pigs-proxy` 和 `pigs-orchestrator` 共同消费，**不依赖本仓库任何其他 crate**。

## 计划内容

- **消息模型**：三种协议的请求/响应结构体（Message、ToolCall、ContentBlock、Usage 等）
- **JSON 编解码**：请求体解析与序列化
- **SSE 解析**：流式事件的增量解析（`data:` 行、event 帧、[DONE]），供透传与编排两侧共用
- **`-pig` 规则**：model id 后缀的识别与剥除（唯一一处定义，别处只调用）
- **路径规则**：裸路径 → 协议的判定与上游 URL 拼接（append 约定）

## 不放什么

- 不做 HTTP 服务器/客户端（那是 proxy / orchestrator 的事）
- 不做重试、鉴权决策、模型映射（本仓库已整体移除这些概念）
- 不认识 `-pig` 之外的任何模型名语义

## 从 legacy 收编

- `legacy/crates/pigs-api/src/protocol.rs`、`phased_api_convert.rs` 的协议转换逻辑
- `legacy/crates/pigs-core/src/message.rs` 的消息模型（去 SDK 化，纯 serde 结构体）
- `legacy/crates/pigs-proxy/src/protocol.rs` 的路由判定
