# 系统提示词详解

> 对应源文件: [`src/prompt.rs`](../src/prompt.rs)

## 这个模块解决什么问题？

LLM 本身只是"续写文本"——给它一段文字，它接着往下写。是系统提示词让它"扮演"一个 Agent。这个模块负责动态构建系统提示词。

## 核心概念

### 系统提示词的四部分

```
┌──────────────────────────────────────────────────┐
│  系统提示词 = 身份 + 环境 + 工具 + 规则             │
│                                                   │
│  1. 身份介绍                                      │
│     "你是 Pigs Mini Agent，一个 AI 助手..."       │
│                                                   │
│  2. 环境信息                                      │
│     - 工作目录: /home/user/project                │
│     - 操作系统: linux                             │
│                                                   │
│  3. 工具清单（动态生成！）                          │
│     - **bash**: 执行 shell 命令                   │
│     - **read_file**: 读取文件内容                 │
│     - **write_file**: 写入文件                    │
│     - **edit_file**: 搜索替换编辑                 │
│                                                   │
│  4. 行为规则                                      │
│     1. 先读后改                                    │
│     2. 小改用编辑                                  │
│     3. 验证你的工作                                │
│     ... 8 条规则                                   │
└──────────────────────────────────────────────────┘
```

### 为什么是动态的？

工具列表是从 `ToolRegistry` 遍历生成的。如果注册了新工具，提示词自动包含它；如果移除了工具，提示词自动不提。不需要手动维护提示词与工具列表的一致性。

## 关键设计决策

### 为什么把工具列表放在提示词里？

LLM 需要知道有哪些工具可用、每个工具做什么。虽然 OpenAI API 通过 `tools` 参数传递工具 schema，但在提示词中也列出工具有助于 LLM 更好地理解工具的用途和何时使用。

### 行为规则从哪来？

这 8 条规则来自 CoreCoder 的 `prompt.py`，是经过实战验证的好规则：

1. **先读后改** — 修改文件前先读取
2. **小改用编辑** — edit_file vs write_file 的选择
3. **验证你的工作** — 修改后运行测试
4. **简洁明了** — 代码优于废话
5. **一步一脚印** — 多步任务按序执行
6. **edit_file 唯一性** — old_string 要唯一
7. **尊重现有风格** — 匹配项目约定
8. **不确定就问** — 别猜

## 教学理念

来自 CoreCoder README：

> "改这 80 行里的一行就能看到 Agent 性格变化，是全项目最便宜的'改一处看结果'实验。"

想让 Agent 更谨慎？加一条"修改前先备份"规则。
想让 Agent 更啰嗦？改身份介绍为"你是一个详细解释的助手"。
想让 Agent 注重测试？加一条"每次修改后必须运行测试"规则。

## 借鉴对比

| 项目 | 提示词来源 | 区别 |
|---|---|---|
| CoreCoder (Python) | `prompt.py`（33 行）动态拼接 | 本 crate 的直接灵感来源 |
| pigs | `DEFAULT_SYSTEM_PROMPT` 常量 + `AGENTS.md` 拼接 | 支持 AGENTS.md 项目记忆 |
| codex | `base_instructions` + skills/plugins 注入 | 更复杂，支持动态注入 |
| claw-analog | `system_prompt()` 函数 | 类似，但参数更多（权限模式、preset 等） |
| **本 crate** | `build_system_prompt(tools)` 函数 | 最简版，只依赖工具列表 |

## API 速览

```rust
/// 构建系统提示词 —— 动态拼接工具列表。
pub fn build_system_prompt(tools: &ToolRegistry) -> String
```

唯一的公开函数。接收工具注册表引用，返回完整提示词字符串。

## 测试覆盖

- 提示词包含身份介绍
- 提示词包含环境信息
- 提示词包含工具列表
- 空注册表也能生成有效提示词
- 提示词包含行为规则


---

## 源码拆解（`src/prompt.rs`）

公开 API 只有：`build_system_prompt(tools: &ToolRegistry) -> String`。

### 1. 函数骨架

```rust
pub fn build_system_prompt(tools: &ToolRegistry) -> String {
    let identity = "...";
    let environment = format!("...");
    let tools_section = format!("...");
    let rules = "...";
    format!("{identity}\n\n{environment}\n\n{tools_section}\n\n{rules}")
}
```

四段字符串空行拼接。改性格 = 改这几段。

### 2. 身份（静态）

硬编码中文身份：终端助手、能写代码改 bug、主动用工具。

### 3. 环境（运行时）

```rust
let cwd = std::env::current_dir()
    .map(|p| p.display().to_string())
    .unwrap_or_else(|_| ".".to_string());
let os = std::env::consts::OS;
```

cwd 失败降级 `"."`，避免建 Agent 失败。`OS` 为 `windows` / `linux` / `macos`。

### 4. 工具清单（动态）

```rust
tools.names().iter().map(|name| {
    let schemas = tools.schemas();
    let desc = schemas.iter()
        .find(|s| s["function"]["name"].as_str() == Some(name.as_str()))
        .and_then(|s| s["function"]["description"].as_str())
        .unwrap_or("");
    format!("- **{name}**: {desc}")
})
```

1. 取所有注册名  
2. 在 schemas 里找匹配的 `function.name`  
3. 取 `description`  
4. 拼 Markdown 列表行  

**教学备注**：对每个 name 都调 `schemas()` 是 O(n²)；清晰优先，优化可缓存一次 schemas。

### 5. 规则（静态 8 条）

来自 CoreCoder：先读后改、小改 edit、验证、简洁、顺序、唯一匹配、风格、不确定就问。

### 6. 谁在调用

`Agent::new` 里：`let system_prompt = build_system_prompt(&tools);`  
之后 `full_messages()` 用 `Message::system(&self.system_prompt)` 放在历史最前。

### 7. 测试

含身份关键字、环境段落、假工具出现在列表、空注册表仍有规则。

读完应理解：**prompt 是字符串拼装；工具列表必须与 registry 同源。**
