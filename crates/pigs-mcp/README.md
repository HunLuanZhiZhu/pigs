# pigs-mcp

最小 MCP（Model Context Protocol）客户端，通过 stdio JSON-RPC 连接外部 MCP 服务器，将其工具桥接为 `pigs-core` 的 `ToolHandler`。

## 核心内容

- `McpClient` — stdio JSON-RPC 客户端，支持 `initialize` / `tools/list` / `tools/call` 三个 MCP 子集命令
- `McpServerConfig` — MCP 服务器启动配置（命令、参数、环境变量）
- `McpToolInfo` — 从 `tools/list` 获取的工具元数据
- `McpToolHandler` — 桥接器，将 MCP 工具适配为 `ToolHandler` trait，无缝接入 `ToolRegistry`
- `McpError` — MCP 通信与协议错误类型

## 依赖

- `pigs-core`（workspace）
- `serde` / `serde_json` / `tokio` / `tracing` / `thiserror` / `async-trait`

## 在 workspace 中的角色

扩展层 — 让 Agent 能动态挂载外部 MCP 服务器并复用其工具，由 `pigs-cli` 在启动时根据配置自动连接。
