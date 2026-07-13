# pigs-permissions

权限系统，控制 Agent 可执行哪些工具及在何种条件下执行。

## 核心内容

- `PermissionMode` — 5 级权限模式，按升级级别排序，支持 `>=` 比较
- `PermissionPolicy` / `PermissionOutcome` / `PermissionRequest` — 权限策略与决策结果
- `PermissionPrompter` / `CliPermissionPrompter` / `PermissionDecision` — 交互式权限提示器

## 依赖

- `pigs-core`（workspace 内部依赖）
- `serde` / `serde_json` / `thiserror`

## 在 workspace 中的角色

Layer 1 — 在 core 之上提供工具执行的安全边界，供 pigs-tools、pigs-cli 等调用以实现分级权限控制。
