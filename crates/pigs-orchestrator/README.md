# pigs-orchestrator

**编排引擎——本项目的灵魂。** 接收一个协议请求（model 带 `-pig`），产出"要发往上游的若干子请求 + 合成逻辑"。

## 计划内容

- **相位编排**：Pre → Executor → Post 相位管线；每个相位 = 组装一个子请求 + 处理其响应
- **子请求组装**：按相位生成协议原生的 HTTP subrequest（改写 model、注入提示词、裁剪/扩展消息）
- **loopback 客户端**：子请求经本机回环走 proxy 的透传通道发往上游（防递归的内部令牌）
- **会话头贯穿**：编排开始时生成稳定的 `x-opencode-session`（或继承客户端自带值），
  塞进本次编排的**所有**子请求——mini-proxy 见客户端已带就不覆盖，prompt cache 亲和不断
- **提示词模板**：`include_str!` 嵌入的 Pre/Post 模板（协议 × 语言），不进配置文件

## 边界（红线）

- **不知道 HTTP 服务器的存在**：不依赖 axum，输入是"协议请求"，输出是"子请求序列"，可独立单测
- **不做重试**：子请求失败就失败，向上抛给 proxy 返回给客户端；传输层健壮性归下游 Agent 或 mini-proxy
- **不知道上游是谁**：只管把子请求交出去，mini-proxy 还是直连上游与它无关
- **不做工具执行**：没有 bash/file/MCP，编排只通过协议消息与模型交互

## 从 legacy 收编

- `legacy/crates/pigs-api/src/orchestration.rs`、`phased_runtime.rs`、`phased_phase.rs`、`phased_prompts.rs`
- `legacy/crates/pigs-prompts/prompts/*.txt` 的模板文件
- `legacy/crates/pigs-proxy/src/loopback.rs` 的回环与令牌机制
