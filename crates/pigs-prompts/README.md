# pigs-prompts

相位提示词模板，纯文本 `.txt` 文件经 `include_str!` 编译嵌入，支持中英双语切换。

## 核心内容

- `pre_prompt(lang)` — PRE 相位 system prompt
- `executor_prompt(lang)` — EXECUTOR 相位 system prompt
- `post_prompt(lang)` — POST 相位 system prompt
- `pre_user_payload(lang, failure_paths)` — PRE 相位 user 载荷（填充失败路径）
- `executor_user_payload(lang, pre_output, post_feedback)` — EXECUTOR 相位 user 载荷
- `post_user_payload(lang, pre_output, executor_draft)` — POST 相位 user 载荷
- `prompts/` 目录含 12 个 `.txt` 模板：`pre` / `executor` / `post` 各 `_zh` / `_en` 两个版本，及对应 `_user` 载荷模板

## 依赖

- `pigs-config`（workspace 内部依赖，用于 `Language` 类型）

## 在 workspace 中的角色

Layer 3 — 提示词层，为 pigs-api 相位运行时（Pre→Executor→Post）提供各相位的 system prompt 与 user payload 模板。
