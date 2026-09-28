# pigs —— 只做编排的 AI 请求前置代理

> 本仓库已从多功能 coding agent 收缩为单一职责：**接收请求 → 判断 `-pig` → 直通或编排**。
> 旧工程完整归档在 [`legacy/`](legacy/)，参考项目在 [`references/`](references/)（gitignore）。

## 架构

```text
客户端（Claude Code / 其他 Agent）
  │
  ▼
pigs（本仓库，默认 127.0.0.1:3927）
  │  按 model id 分流：
  ├─ 无 -pig 后缀 ──→ 原样透传 ──→ 上游
  └─ 有 -pig 后缀 ──→ 编排（Pre→Executor→Post）
                          │  子请求经 loopback 回环
                          ▼
                       上游
```

**上游是谁 pigs 不关心**：可以串联 [mini-proxy](../../../ProgramGitHub/new-api/mini-proxy)（多拿会话头/模型映射/重试），
也可以直连任意上游 API（`base_url` + 协议路径 append，三协议共用一个地址）。
 pigs 内**没有重试、没有模型映射、没有多供应商路由**——唯一的分流依据是 model id 的 `-pig` 后缀。
传输层的健壮性由下游 Agent 自己负责，或由链路上的 mini-proxy 负责。

## crate 一览（依赖严格单向）

| crate | 职责 | 依赖 |
|---|---|---|
| [`crates/pigs`](crates/pigs) | 可执行入口：CLI 参数、加载配置、启动服务 | pigs-proxy |
| [`crates/pigs-proxy`](crates/pigs-proxy) | HTTP 入口：三协议路由、`-pig` 分流、SSE 透传 | pigs-orchestrator, pigs-protocol |
| [`crates/pigs-orchestrator`](crates/pigs-orchestrator) | 编排引擎：相位编排、子请求组装、会话头贯穿 | pigs-protocol |
| [`crates/pigs-protocol`](crates/pigs-protocol) | 三协议消息模型与 JSON/SSE 编解码、`-pig` 规则 | （无） |

## 配置

只有部署位置需要配置（`config.toml`，3 行）：

```toml
listen = "127.0.0.1:3927"
base_url = "http://127.0.0.1:7946"   # mini-proxy 或任意上游
# key = ""                            # 留空透传客户端 key，填了则覆盖
```

提示词模板、相位定义、`-pig` 规则全部是代码（`include_str!` / 常量），不进配置。
