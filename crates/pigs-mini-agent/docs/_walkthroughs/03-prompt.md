

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
