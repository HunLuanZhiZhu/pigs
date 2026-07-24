# pig TUI 测试指南

## 启动 TUI

在真实终端中运行：

```bash
cargo run -p pigs
```

系统会自动：
1. 启动 pigs-proxy API 服务器（后台，端口 3927）
2. 启动 TUI 全屏界面（前台）

如果 TUI 初始化失败（如非 TTY 环境），会自动回退到 rustyline 行模式。

## 界面布局（从上到下）

```
┌──────────────────────────────────────────────────┐
│  pig v0.1.3 | model | lang | /help  !bash  ^D  │ ← Header
├──────────────────────────────────────────────────┤
│                                                  │
│  > user message                                  │ ← Chat
│                                                  │
│  assistant response (markdown rendered)          │
│                                                  │
│  [tool_name] args                                │
│    result output                                 │
│                                                  │
├──────────────────────────────────────────────────┤
│  ⠋ Working...                                    │ ← Status (working时)
├──────────────────────────────────────────────────┤
│  ┌─ pig > ───────────────────────────────┐      │ ← Editor
│  │                                         │      │
│  └─────────────────────────────────────────┘      │
├──────────────────────────────────────────────────┤
│  ~/path (git-branch)                             │ ← Footer
│  ↑123 ↓456 45.0%  model-name                     │
└──────────────────────────────────────────────────┘
```

## 快捷键

| 按键 | 功能 |
|---|---|
| `Enter` | 提交消息 |
| `Shift+Enter` / `Ctrl+J` | 换行（多行输入） |
| `Ctrl+C` | 中断 / 清空输入（空时退出） |
| `Ctrl+D` | 退出 pig |
| `Tab` | 自动补全 |

## 斜杠命令

在编辑器中输入 `/` 开头的命令：

| 命令 | 说明 |
|---|---|
| `/help` | 显示帮助 |
| `/quit` | 退出 |
| `/model` | 切换/添加模型 |
| `/new` | 新建会话 |
| `/copy` | 复制最后一条回复 |
| `/fork` | 分叉当前会话 |
| `/clone` | 克隆会话 |
| `/tree` | 查看会话树 |
| `/compact` | 压缩上下文 |
| `/export` | 导出会话 |
| `/share` | 分享会话（导出到文件） |
| `/settings` | 显示设置 |
| `/hotkeys` | 显示快捷键 |
| `/lang` | 切换语言（en/zh） |
| `/status` | 状态面板 |
| `/mcp` | 管理 MCP 服务器 |
| `/skills` | 查看技能 |
| `/undo` | 撤销写操作 |
| `/doctor` | 健康检查 |

中文/拼音别名始终可用（如 `/帮助`、`/退出`、`/模型`）。

## Bash 模式

在编辑器开头输入 `!` 执行 bash 命令：

```
!ls -la
!git status
!echo hello
```

命令输出显示在聊天区域，但不会发送给 LLM。

## 流式输出

当 Agent 回复时，文本会逐 token 流式显示在聊天区域。
工具调用会实时显示（工具名 + 参数 + 结果）。
状态栏显示 "Working..." 旋转指示器。

## 配置

- 全局配置：`~/.pig/config-cli.toml`
- 项目配置：`.pig/config-cli.toml`
- 会话：`~/.pig/sessions/*.jsonl`
- 日志：`~/.pig/logs/pigs.log.YYYY-MM-DD`
- 忽略文件：`.pigignore`（gitignore 格式）

## 环境变量

| 变量 | 说明 |
|---|---|
| `PIG_CONFIG` | 配置文件路径覆盖 |
| `PIG_LANGUAGE` | 语言（en/zh） |
| `PIG_MODEL` | 默认模型 |
| `PIG_PERMISSION_MODE` | 权限模式 |
| `PIG_SYSTEM_PROMPT` | 自定义系统提示词 |
