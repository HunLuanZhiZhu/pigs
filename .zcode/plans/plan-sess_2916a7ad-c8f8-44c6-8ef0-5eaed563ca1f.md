# 配置拆分计划：config.toml（API）+ config-cli.toml（CLI）

## 目标

将当前单一配置文件拆分为两个独立配置文件：
- **`config.toml`** → 仅 API/代理配置（`[server]`、`[log]`、`[[provider]]` + `language`）
- **`config-cli.toml`** → CLI 专属配置（所有 `AppConfig` 字段）

CLI 配置采用三层分层加载（方式3，全部带 `-cli` 后缀）：
1. `~/.pigs/config-cli.toml`（全局用户级）
2. `{workspace}/.pigs/config-cli.toml`（项目级覆盖）
3. `{workspace}/.pigs/config-cli.local.toml`（机器本地，gitignored）

`language` 字段两边各自保留（API 和 CLI 可独立配置）。

---

## 变更清单

### 1. `crates/pigs-config/src/config.rs` — 修改 CLI 配置路径

**`config_path()`（行 451-453）**：全局路径从 `~/.pigs/config.toml` 改为 `~/.pigs/config-cli.toml`

```rust
pub fn config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".pigs").join("config-cli.toml")
}
```

**`load_layered()`（行 342-357）**：项目级和本地级路径改为带 `-cli` 后缀

```rust
let project_path = workspace.join(".pigs").join("config-cli.toml");
// ...
let local_path = workspace.join(".pigs").join("config-cli.local.toml");
```

同时更新方法上的文档注释（行 338-341）。

**文档注释更新**（行 178-179, 327, 350-351）：把 `config.toml` → `config-cli.toml`、`.pigs/config.toml` → `.pigs/config-cli.toml`、`.pigs/config.local.toml` → `.pigs/config-cli.local.toml`。

### 2. `crates/pigs-proxy/src/config.rs` — 从 `Config` 移除 CLI 死字段

从 `Config` 结构体（行 11-57）移除以下字段及其 `default_*` 函数（行 59-79）：
- `permission_mode` + `default_permission_mode()`
- `max_turns` + `default_max_turns()`
- `max_tokens` + `default_max_tokens()`
- `temperature` + `default_temperature()`
- `system_prompt`
- `compact_token_threshold` + `default_compact_threshold()`
- `compact_keep_recent` + `default_compact_keep()`

**保留 `language`**（行 20-21 + `default_language()`，行 59-61），因为 `serve()` 在 `lib.rs:272` 读取它来构建 `HttpPhasedRuntime`。

更新结构体上方的注释（行 17），改为仅说明 `language` 的用途。

### 3. `config.toml`（工作区根）— 移除 CLI 顶层字段

从文件中移除以下行（行 10-16）：
```toml
permission_mode = "workspace_write"
max_turns = 50
max_tokens = 4096
temperature = 0.2
compact_token_threshold = 100000
compact_keep_recent = 10
# system_prompt = ""
```

**保留** `language = "zh"`（行 9）—— API 运行时需要它。

### 4. 新建 `config-cli.toml`（工作区根）— CLI 默认配置

创建新文件 `config-cli.toml`，包含从原 `config.toml` 移出的 CLI 字段，作为项目级 CLI 配置示例/默认值：

```toml
# pigs CLI 专属配置文件
# CLI-specific configuration for pigs-cli (REPL / one-shot agent)
# 全局层: ~/.pigs/config-cli.toml
# 项目层: {workspace}/.pigs/config-cli.toml（本文件）
# 本地层: {workspace}/.pigs/config-cli.local.toml（gitignored）

language = "zh"                    # UI / 回复语言：zh（默认）或 en
permission_mode = "workspace_write" # CLI REPL 权限模式
max_turns = 50                     # Agent 循环最大轮次
max_tokens = 4096                  # LLM 最大输出 token 数
temperature = 0.2                 # LLM 温度参数
compact_token_threshold = 100000  # 压缩阈值（token 数）
compact_keep_recent = 10          # 压缩时保留的最近消息数
# system_prompt = ""              # 自定义系统提示词（可选，留空用默认）
# model = ""                      # 默认模型（如 claude-sonnet-4-20250514）
```

### 5. 迁移现有 `.pigs/config.toml` → `.pigs/config-cli.toml`

将 `.pigs/config.toml` 的内容移到 `.pigs/config-cli.toml`（内容不变，仅文件名变更），删除旧的 `.pigs/config.toml`。

### 6. 迁移现有 `.pigs/config.local.toml` → `.pigs/config-cli.local.toml`

将 `.pigs/config.local.toml` 的内容移到 `.pigs/config-cli.local.toml`，删除旧文件。

### 7. `.gitignore` — 更新路径

行 27-28：
```
.pigs/config.local.toml    →  .pigs/config-cli.local.toml
.pigs/*.local.toml         →  保留（已覆盖 config-cli.local.toml）
```

### 8. `crates/pigs-cli/src/doctor.rs` — 更新项目配置路径

- 行 32：`agent.workspace_root.join(".pigs").join("config.toml")` → `join("config-cli.toml")`
- 行 171-172：`config_paths()` 返回的项目路径从 `.pigs/config.toml` → `.pigs/config-cli.toml`

### 9. `crates/pigs-cli/src/commands.rs` — 更新显示文本

- 行 681：`"Configure hooks in ~/.pigs/config.toml:"` → `"Configure hooks in ~/.pigs/config-cli.toml:"`
- 行 836：`"Config example (~/.pigs/config.toml):"` → `"Config example (~/.pigs/config-cli.toml):"`
- 行 859：`"Or configure [[mcp_servers]] in ~/.pigs/config.toml"` → `"Or configure [[mcp_servers]] in ~/.pigs/config-cli.toml"`

### 10. `crates/pigs-cli/src/models.rs` — 更新显示文本

- 行 261：`"写入 ~/.pigs/config.toml"` → `"写入 ~/.pigs/config-cli.toml"`
- 行 264：`"~/.pigs/config.toml"` → `"~/.pigs/config-cli.toml"`

### 11. 文档更新

- **`AGENTS.md`** 行 21：`config.toml` 说明改为 API 配置；行 108：CLI 路径改为 `~/.pigs/config-cli.toml` + `.pigs/config-cli.toml`
- **`README.md`**：更新配置说明段落
- **`docs/agent-design.md`** 行 457, 482：更新路径
- **`.pigs/local/README.md`**：更新 `config.local.toml` 引用为 `config-cli.local.toml`

---

## 验证步骤

1. `cargo build` — 确保编译通过
2. `cargo clippy` — 无新 warning
3. `cargo test -p pigs-config` — 配置测试通过
4. `cargo test -p pigs-cli` — CLI 测试通过（doctor 测试等）
5. 手动确认：`config.toml` 不含 CLI 字段；`config-cli.toml` 存在且含 CLI 字段；`pigs-proxy::Config` 不再有 CLI 死字段

## 不变的部分

- `PIGS_CONFIG` 环境变量和 `config.toml` 作为 API 配置的加载逻辑不变
- `AppConfig` 结构体本身的所有字段不变（只是加载路径变了）
- `~/.pigs/sessions/`、`~/.pigs/logs/`、`~/.pigs/memory.md`、`~/.pigs/skills/` 等非配置路径不变
- 环境变量覆盖逻辑 `with_env_overrides()` 不变
- 分层合并逻辑 `merge_project_overrides()` 不变
