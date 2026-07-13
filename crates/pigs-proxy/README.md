# pigs-proxy

多协议 HTTP 前置代理 + 相位化 Agent 路由器。监听单一端口（默认 3927），按 model ID 分流：无 `-pig` 后缀透传上游 LLM，有 `-pig` 后缀走 `pigs-api` 相位运行时。

## 架构与数据流

```
客户端请求 (OpenAI Chat / Anthropic / OpenAI Responses)
  │
  ▼
server (端口 3927)
  │
  ├─ model 无 -pig 后缀 ──→ upstream + retry::dispatch
  │                           （透传，含重试 + body 清洗 + 思考强度注入）
  │
  └─ model 有 -pig 后缀 ──→ pigs-api PhasedRuntime
                               （Pre→Executor→Post，LLM 请求回走 dispatch_in_process）
```

三相位的 LLM 请求不走 HTTP loopback，而是通过 `dispatch_in_process` 直接进入 `retry::dispatch`，自动享受重试与 body 清洗。

## 核心内容

- `server` — Axum HTTP 服务器，路由 `/chat/completions`、`/v1/messages`、`/responses`、`/v1/models`
- `config::Config` — 代理配置（provider / endpoint / 日志 / language 等）
- `protocol::Protocol` — 协议枚举（`OpenAI` / `Anthropic` / `Responses`）
- `upstream::UpstreamClient` — 上游 HTTP 客户端（reqwest + rustls）
- `retry::dispatch` / `retry::DispatchOutcome` — 重试调度与结果
- `proxy_client::ProxyApiClient` — 实现 `ApiClient` trait，相位运行时通过它发起 LLM 请求
- `fn serve()` — 启动代理服务器（阻塞式）
- `fn build_phased_runtime()` — 从配置构建 `PhasedRuntime`
- `fn dispatch_in_process()` — 进程内调度（绕过 HTTP，直走重试逻辑）

## 依赖

- `pigs-api`、`pigs-core`、`pigs-config`（均 workspace）
- `axum` 0.7 / `hyper` 1 / `tower` 0.5 / `reqwest` 0.12 / `tracing-subscriber` / `tracing-appender`

## 在 workspace 中的角色

前置路由层 — 项目的网络入口与协议适配层，将外部多协议请求统一路由到上游 LLM 或 pigs-api 相位运行时。
