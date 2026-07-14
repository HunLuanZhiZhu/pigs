# pigs-prompts

相位 user payload 模板。活跃模板为 Pre / Executor / Post 各中英文版本，共 6 个 `.txt` 文件，经 `include_str!` 编译嵌入。

## 核心 API

- `pre_user_payload(lang, failure_paths)`：Pre 五问与编号失败记录。
- `executor_user_payload(lang, pre_output, _post_feedback)`：原始用户问题之后的执行说明和完整 Pre 输出。
- `post_user_payload(lang, _pre_output, _executor_draft)`：独立验收文本；Executor/Post 轨迹由协议原生消息承载。

相位指令只作为 user payload，不替换、拼接或新增 system prompt。`pre_*.txt`、`executor_*.txt`、`post_*.txt` 六个旧 identity 文件仍保留在目录中作为历史材料，但不再由 Rust API 导出或注入模型请求。

## 依赖与角色

仅依赖 `pigs-config` 的 `Language`。本 crate 为 `pigs-api` 的 HTTP 原生运行时和保留的 CLI 本地运行时提供中英文相位载荷。
