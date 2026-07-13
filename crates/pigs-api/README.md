# pigs-api

OpenAI 兼容的本地 HTTP API 服务器 + Pre→Executor→Post 三相位 Agent 运行时模块库。

## 架构与数据流

```
OpenAI 请求
  │
  ▼
phased_api_convert  ── 请求格式转换
  │
  ▼
phased_runtime (Pre → Executor → Post)
  │   ├─ Pre:  规划阶段，生成执行计划
  │   ├─ Executor: 执行阶段，调用工具完成任务
  │   └─ Post: 评审阶段，检查目标达成
  │
  ▼
phased_markers (PIGEND / PIGFAILED 检测与清理)
  │
  ▼
OpenAI 兼容响应
```

三相位的 LLM 请求不直接连上游，而是通过 `pigs-proxy` 的 `dispatch_in_process` 走进程内重试逻辑。

## 核心内容

- `server` — OpenAI 兼容 HTTP API（`/health`、`/v1/models`、`/v1/chat/completions`，含 SSE 流式）
- `PhasedRuntime` — 三相位 Agent 运行时核心结构体
- `phased_api_convert` — OpenAI 请求 → 相位运行时输入的转换层
- `phased_markers` — `PIGEND` / `PIGFAILED` 控制标记的检测与清理
- `Phase` 枚举 — `Pre` / `Executor` / `Post`
- `phased_tools` — 相位运行时工具注册表（复用 `pigs-tools` 全量工具）
- `format` — 三种 API 格式的请求解析与响应构造

## 依赖

- `pigs-core`、`pigs-llm`、`pigs-config`、`pigs-tools`、`pigs-prompts`（均 workspace）
- `axum` 0.7 / `tower-http` 0.5 / `serde` / `tokio` / `uuid` / `chrono` / `futures-util`

## 在 workspace 中的角色

运行时层 — 为 `pigs-proxy` 提供 `-pig` 模型路由所需的 `PhasedRuntime` 及全部相位逻辑，是项目核心创新（相位化 Agent）的实现所在。
