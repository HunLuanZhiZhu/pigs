

---

## 源码拆解（`src/tools/`）

### 0. `mod.rs` 工厂

```rust
pub fn create_default_tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BashTool));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry
}
```

四个单元结构体 `struct XxxTool;`，无字段，行为全在 `impl Tool`。

---

### 1. `bash.rs`

```text
input["command"]
  → Windows: cmd /C  |  Unix: sh -c
  → tokio::process::Command + piped stdout/stderr
  → timeout 120s
  → 拼 stdout + [stderr] + [退出码: n]
  → 超过 20000 字符截断
```

- `timeout` 包 `wait_with_output`，超时 → `ToolError`  
- 非零退出仍 `Ok(String)`，退出码写在文本里（**软错误**）  
- `from_utf8_lossy`：非法 UTF-8 不崩  

### 2. `read_file.rs`

```text
path / offset(默认1) / limit(默认2000)
  → read_to_string → lines 切片
  → "{line_num}\t{line}\n"
  → 范围提示 + 10000 字符截断
```

行号 1-based；`offset-1` 转 0-based；`.min(total)` 防越界。

### 3. `write_file.rs`

```text
path + content
  → parent 不存在则 create_dir_all
  → fs::write（覆盖）
  → 返回字符数/行数摘要
```

### 4. `edit_file.rs`（最重要）

```text
path, old_string, new_string
  → 校验 old != new
  → read_to_string
  → match_count = content.matches(old).count()
  → 0: Err + 文件头 500 字
  → 1: replacen(..., 1) + write → Ok
  → _: Err 要求更多上下文
```

对应 `match match_count { 0 => ..., 1 => ..., _ => ... }`。

- `replacen(..., 1)`：唯一时只换一处  
- 0 匹配返回文件头：帮模型重新锚定  
- 多匹配逼模型加上下文  

### 5. 共同模式

每个工具：`name` / `description` / `parameters` → `execute` 里抽参 → 业务 → `Ok(摘要字符串)`。

读完应能指出 **edit_file 三分支各自对应哪几行逻辑**。
