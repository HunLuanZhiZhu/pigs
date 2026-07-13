# pigs-config

配置管理，涵盖 TOML 配置文件、环境变量、AGENTS.md 解析及项目记忆加载。

## 核心内容

- `AppConfig` / `ModelConfig` / `ProviderConfig` / `ResolvedModel` — 应用与模型配置
- `load_agents_md()` / `build_system_prompt()` / `ProjectContext` — 项目记忆（AGENTS.md/CLAUDE.md）解析与系统提示构建
- `Language` — 语言设置（`zh` / `en`）
- `MemoryStore` / `load_rules()` / `load_skills()` / `Skill` — 跨会话记忆、规则与技能加载

## 依赖

- `pigs-core` / `pigs-permissions`（workspace 内部依赖）
- `serde` / `serde_json` / `toml` / `dirs` / `chrono` / `thiserror`

## 在 workspace 中的角色

Layer 2 — 配置与上下文层，为 pigs-cli、pigs-api 等提供配置读取、系统提示构建、记忆/规则/技能加载能力。
