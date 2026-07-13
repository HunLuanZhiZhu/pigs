# pigs-llm

LLM 供应商客户端，实现 `ApiClient` trait，支持 Anthropic Messages、OpenAI Responses 与 OpenAI Chat Completions 协议及 SSE 流式。

## 核心内容

- `AnthropicClient` — Anthropic Messages API 客户端
- `OpenAiResponsesClient` — OpenAI Responses API 客户端
- `OpenAiClient` — OpenAI Chat Completions API 客户端
- `create_client()` / `detect_provider()` / `resolve_model_alias()` — 供应商探测与客户端创建
- `Provider` / `ClientConfig` — 供应商枚举与客户端配置

## 依赖

- `pigs-core`（workspace 内部依赖，实现 `ApiClient` trait）
- `reqwest` / `tokio` / `futures-util` / `tracing` / `async-trait` / `serde` / `serde_json` / `thiserror`

## 在 workspace 中的角色

Layer 3 — LLM 客户端层，为 pigs-api、pigs-cli 等提供具体 LLM 供应商的 API 调用与流式响应实现。
