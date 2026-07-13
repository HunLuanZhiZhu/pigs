# pigs-session

会话持久化，基于 JSONL 格式存储会话并支持自动压缩。

## 核心内容

- `Session` — 会话主体，管理消息列表与元数据
- `SessionMetadata` — 会话元数据（标题、时间等）
- `SessionError` — 会话操作错误类型
- `compact_session()` / `CompactConfig` — 会话压缩，在 token 超阈值时保留近期消息

## 依赖

- `pigs-core`（workspace 内部依赖）
- `serde` / `serde_json` / `dirs` / `uuid` / `chrono` / `thiserror`

## 在 workspace 中的角色

Layer 2 — 会话持久化层，为 pigs-cli 提供会话存储于 `~/.pigs/sessions/*.jsonl` 的读写与压缩能力。
