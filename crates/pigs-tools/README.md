# pigs-tools

Pigs Agent 的内置工具集，实现 `pigs-core` 的 `ToolHandler` trait，提供文件读写、搜索、Shell 执行、Web 抓取等 16 项能力。

## 核心内容

- `ReadFileTool` / `GrepTool` / `GlobTool` / `ListFilesTool` — 只读文件操作工具
- `WriteFileTool` / `EditFileTool` / `ApplyPatchTool` — 工作区写入工具
- `BashTool` — 高危 Shell 命令执行工具（`DangerFullAccess` 权限）
- `WebFetchTool` / `WebSearchTool` / `HttpRequestTool` — 网络访问工具
- `GitDiffTool` / `AskUserTool` / `SleepTool` — 交互与辅助工具
- `TodoWriteTool` — 有状态待办清单工具（共享 `TodoList`）
- `fn create_default_registry()` — 构建包含全部内置工具的 `ToolRegistry`
- `fn create_default_registry_with_todos()` — 返回 `ToolRegistry` + 共享 `TodoList`（供 CLI 展示）
- `fn tool_permission_modes()` — 返回每个工具对应的 `PermissionMode` 映射

## 依赖

- `pigs-core`（workspace）、`pigs-permissions`（workspace）
- `serde` / `serde_json` / `tokio` / `regex` / `glob` / `reqwest` / `tracing` / `async-trait` / `thiserror`

## 在 workspace 中的角色

工具层 — 向 `pigs-cli`（REPL agent）和 `pigs-api`（相位运行时）提供统一注册的内置工具集，是 Agent 执行力的核心实现。
