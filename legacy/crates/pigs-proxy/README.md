# pigs-proxy

`pigs-proxy` 是多协议 HTTP 前置代理与 `-pig` 路由层。单一端口同时接受 OpenAI Chat、Anthropic Messages 和 OpenAI Responses。

## 路由

```text
客户端请求
  │
  ├─ model 无 -pig -> passthrough -> 渠道映射/清洗/思考强度/重试 -> provider
  │
  └─ model 有 -pig -> pigs-api HttpPhasedRuntime
                         │
                         └─ 去掉一层 -pig 的协议原生 HTTP subrequest
                              -> 本机 loopback -> 同一 handler -> passthrough -> provider
```

loopback 请求保留原方法、path/query、端到端 headers 和完整 JSON body。`LoopbackPhaseTransport` 加入随机内部令牌；handler 验证后绕过 `-pig` 分流，并在发往供应商前删除内部 header，避免递归和泄露。

监听 `0.0.0.0` 或 `[::]` 时，transport 会转换为可连接的 loopback 地址。对本机 provider 自动禁用环境 HTTP proxy；外部 provider 仍遵循原有网络配置。

## 核心模块

- `server`：三协议入口、`-pig`/passthrough 分流、JSON/SSE 返回与协议错误。
- `loopback`：`PhaseTransport` 的 HTTP 实现；解析三协议 JSON/SSE、增量文本和原生工具调用。
- `retry`：同渠道重试、HTTP/业务错误码判断。
- `upstream`：供应商请求与流式 body 处理。
- `config`：provider/endpoint/model_map/key_mode/path_mode/thinking_effort。
- `protocol`：代理侧协议到 endpoint 的映射。
- `proxy_client`：CLI 使用的 `ApiClient` 实现。

## CLI 边界

`ProxyApiClient`、`dispatch_in_process` 和 `build_phased_runtime` 继续服务 CLI 本地 Agent，因此 CLI 的 bash/file/MCP 工具循环不受 HTTP 重构影响。HTTP `-pig` 路径不再使用 `ConvertedTurn -> ApiRequest -> OpenAI Chat`，也不再使用进程内 dispatch。
