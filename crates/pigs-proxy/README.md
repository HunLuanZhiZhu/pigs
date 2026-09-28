# pigs-proxy

HTTP 入口与传输壳：单端口接收三种协议，按 `-pig` 分流。

```text
POST /chat/completions   → OpenAI Chat
POST /v1/messages        → Anthropic Messages
POST /responses          → OpenAI Responses
GET  /v1/models          → 模型列表（每个模型 + 其 -pig 变体）
```

## 请求流

```text
进入请求
  ├─ model 无 -pig → 原样透传：upstream.base_url + 原路径，SSE/JSON 流式回传
  └─ model 有 -pig → 交 pigs-orchestrator 编排，子请求经 loopback 走透传通道
```

## 计划内容

- **axum 路由与监听**（默认 `127.0.0.1:3927`）
- **透传通道**：方法/路径/查询串/body/端到端头 原样转发；`content-encoding` 必须随 body 转发
  （legacy 版在 `upstream.rs:272` 撕掉了这个头导致压缩体被当明文解析——重建时必须带上该修复）
- **SSE 流式回传**：预读检测错误事件 + 剩余流拼合（沿 legacy 实现）
- **配置加载**：3 行 toml（listen / base_url / key），`include_str!` 模板 + `--example` 导出
- **协议错误返回**：JSON error body

## 刻意不做（与 legacy/crates/pigs-proxy 的差异）

- **无重试**：删掉 `retry.rs` 整套（双层重试会放大故障；健壮性归下游 Agent 或链路上的 mini-proxy）
- **无 model_map / models 列表 / 多供应商选择**：上游只有一个 `base_url`，分流只看 `-pig`
- **无 thinking_effort 注入**、**无 x-opencode-session 逻辑**（归 mini-proxy）
- **无 aux 未识别路径透传**（legacy 行为；需要时再加）

## 从 legacy 收编

- `legacy/crates/pigs-proxy/src/server.rs`、`upstream.rs`、`log.rs`（瘦身版）
- `config.example.toml` 模板机制
