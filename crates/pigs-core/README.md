# pigs-core

核心类型与 trait 定义，为所有其他 crate 提供共享基础抽象。

## 核心内容

- `Message` / `ContentBlock` / `MessageRole` — 消息模型，统一表示多轮对话内容
- `ToolSpec` / `ToolHandler` / `ToolResult` / `ToolRegistry` — 工具系统抽象与注册表
- `ApiClient` / `ApiRequest` / `ApiResponse` / `StreamEvent` — LLM API 调用与流式响应抽象
- `TokenUsage` — token 用量跟踪

## 依赖

- 无内部 crate 依赖（零内部依赖的基础 crate）
- `serde` / `serde_json` / `thiserror` / `async-trait`

## 在 workspace 中的角色

Layer 0 — 全局基础层，定义所有核心 trait 与共享类型，不依赖任何内部 crate。
