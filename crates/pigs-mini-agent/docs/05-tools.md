# 内置工具详解

> 对应源文件: [`src/tools/`](../src/tools/)

## 这个模块解决什么问题？

内置工具是 Agent "动手"的具体能力。本 crate 精选 4 个最核心的工具，覆盖了编程 Agent 的"读-改-验证"循环：

```
读代码        改代码         验证
  │            │             │
  ▼            ▼             ▼
read_file → edit_file   →  bash
           write_file
```

## 四个工具一览

| 工具 | 文件 | 功能 | 借鉴来源 |
|---|---|---|---|
| `bash` | `bash.rs` | 执行 shell 命令 | CoreCoder `bash.py` + pigs-tools `bash.rs` |
| `read_file` | `read_file.rs` | 读取文件内容 | CoreCoder `read.py` |
| `write_file` | `write_file.rs` | 写入文件 | CoreCoder `write.py` |
| `edit_file` | `edit_file.rs` | 搜索替换编辑 | CoreCoder `edit.py`（核心创新） |

## bash 工具

### 功能
执行 shell 命令并返回输出（stdout + stderr + 退出码）。

### 关键设计

| 特性 | 值 | 原因 |
|---|---|---|
| 超时 | 120 秒 | 防止 `tail -f` 等永不结束的命令 |
| 输出截断 | 20000 字符 | 防止编译日志撑爆上下文 |
| 跨平台 | `cmd /C` (Windows) / `sh -c` (Unix) | 自动选择 shell |
| 异步 | `tokio::process::Command` | 不阻塞 Agent 循环 |

### 参数

```json
{
    "command": "echo hello"    // 必需：要执行的命令
}
```

### 输出示例

```
hello
[退出码: 0]
```

### 安全提示

真正的生产级 Agent 应该有命令白名单/黑名单、沙箱隔离等安全措施。参考 claw-code 的 `PermissionEnforcer` 和 codex 的沙箱系统。本教学版不做安全限制，因为重点是展示 Agent 循环而非安全工程。

## read_file 工具

### 功能
读取文件内容，带行号返回，支持范围读取。

### 关键设计

- **行号格式**: `{行号}\t{行内容}`，来自 CoreCoder 的设计
- **范围读取**: `offset`（起始行）+ `limit`（行数）
- **截断**: 10000 字符上限

### 参数

```json
{
    "path": "/tmp/test.txt",   // 必需：文件路径
    "offset": 1,               // 可选：起始行（默认 1）
    "limit": 2000              // 可选：读取行数（默认 2000）
}
```

## write_file 工具

### 功能
将内容写入文件（覆盖已有内容），自动创建不存在的父目录。

### 关键设计

- **自动建目录**: `create_dir_all` 递归创建父目录
- **覆盖模式**: 如果文件已存在则完全覆盖

### 参数

```json
{
    "path": "/tmp/test.txt",       // 必需：文件路径
    "content": "hello world"       // 必需：文件内容
}
```

### 何时用 write_file vs edit_file？

来自 CoreCoder 的行为规则：
- **write_file**: 新建文件 或 完全重写（内容大幅变化）
- **edit_file**: 小范围修改（改几行、改几个词）

## edit_file 工具

### 功能
通过搜索替换编辑文件——**CoreCoder 的核心创新**。

### 工作方式

```
1. LLM 提供 old_string 和 new_string
2. 在文件中搜索 old_string
3. 恰好匹配 1 次 → 执行替换 ✓
4. 匹配 0 次 → 返回错误 + 文件开头片段（帮 LLM 重新锚定）
5. 匹配多次 → 返回错误（要求 LLM 加更多上下文）
```

### 参数

```json
{
    "path": "/tmp/test.rs",
    "old_string": "println!(\"hello\");",
    "new_string": "println!(\"world\");"
}
```

### 为什么不用行号编辑？

来自 CoreCoder README 第 130 行的设计哲学：

> "行号是陷阱，模型数错一行就悄悄改错地方。用唯一片段锚定，失败可恢复、成功可验证。"

行号编辑的问题：LLM 经常数错行号，改错地方后可能不会报错——悄悄引入 bug。
唯一匹配的优势：
- 匹配不唯一 → 报错，LLM 自己加上下文重试
- 匹配 0 次 → 报错，返回文件开头帮 LLM 锚定
- 恰好 1 次 → 执行，可以验证改对了

### 错误处理策略

| 情况 | 返回 | 原因 |
|---|---|---|
| 0 次匹配 | 错误 + 文件开头 500 字符 | 帮 LLM 重新定位 |
| 1 次匹配 | 成功替换 | 唯一锚定 |
| 多次匹配 | 错误 + "加入更多上下文" | LLM 需要更精确的 old_string |
| old == new | 错误 | 无意义的替换 |

## 借鉴对比

| 工具 | CoreCoder (Python) | pigs-tools (Rust) | 本 crate |
|---|---|---|---|
| bash | 危险命令黑名单 + cd 跟踪 | 50KB 截断 | 超时 + 20K 截断 |
| read_file | 53 行，行号 | 164 行，100KB 截断 | 211 行，10K 截断 |
| write_file | 38 行，mkdir parents | 130 行 | 200 行，自动建目录 |
| edit_file | 92 行，唯一匹配 | 190 行，+replace_all | 277 行，唯一匹配 |

## 测试覆盖

每个工具都有测试：
- 正常执行
- 缺少参数报错
- 边界情况（文件不存在、多次匹配等）
- edit_file 的多行替换


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
