#!/usr/bin/env python3
"""Patch pigs slash commands for language + Chinese/pinyin aliases."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
path = ROOT / "crates/pigs/src/commands.rs"
text = path.read_text(encoding="utf-8")

# Already patched?
if "canonicalize_command" in text and "bare_chinese_switch" in text:
    print("already patched")
    raise SystemExit(0)

old_header = """//! Slash command handling for the REPL.

use std::str::FromStr;

use pigs_permissions::PermissionMode;

use crate::agent::Agent;

/// Result of handling a slash command.
pub enum CommandResult {
    /// Command handled successfully, continue the REPL.
    Continue,
    /// User requested to quit.
    Quit,
}

/// Handle a slash command.
pub async fn handle_command(agent: &mut Agent, line: &str) -> anyhow::Result<CommandResult> {
    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
    let cmd = parts[0].trim_start_matches('/');
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "help" | "h" | "?" => {
            print_help();
            Ok(CommandResult::Continue)
        }

        "quit" | "q" | "exit" => {
            println!("Goodbye!");
            Ok(CommandResult::Quit)
        }
"""

new_header = """//! Slash command handling for the REPL.
//!
//! English command names remain primary. Chinese characters and pinyin aliases
//! are always accepted via [`crate::command_aliases`], independent of UI language.

use std::str::FromStr;

use pigs_config::Language;
use pigs_permissions::PermissionMode;

use crate::agent::Agent;
use crate::command_aliases::{
    canonicalize_command, canonicalize_mcp_sub, canonicalize_sessions_sub,
};
use crate::i18n;

/// Result of handling a slash command.
pub enum CommandResult {
    /// Command handled successfully, continue the REPL.
    Continue,
    /// User requested to quit.
    Quit,
}

/// Handle a slash command.
pub async fn handle_command(agent: &mut Agent, line: &str) -> anyhow::Result<CommandResult> {
    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
    let raw_cmd = parts[0].trim_start_matches('/');
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");
    let cmd = canonicalize_command(raw_cmd);

    // Bare `/中文` (or pinyin `/zhongwen`) with no args → switch language to zh.
    let bare_chinese_switch = matches!(
        raw_cmd,
        "中文" | "zhongwen" | "Zhongwen" | "ZHONGWEN"
    ) && arg.is_empty();

    match cmd {
        "help" => {
            print_help(agent.language);
            Ok(CommandResult::Continue)
        }

        "quit" => {
            println!("{}", i18n::t(agent.language, "goodbye"));
            Ok(CommandResult::Quit)
        }

        "lang" => {
            if bare_chinese_switch {
                agent.set_language(Language::Zh);
                println!("{}", i18n::t(agent.language, "lang_set_zh"));
                return Ok(CommandResult::Continue);
            }
            if arg.is_empty() {
                println!(
                    "{}: {} ({})",
                    i18n::t(agent.language, "language"),
                    agent.language.as_str(),
                    agent.language.display_name()
                );
                println!("{}", i18n::t(agent.language, "lang_usage"));
            } else {
                match arg.parse::<Language>() {
                    Ok(lang) => {
                        agent.set_language(lang);
                        match lang {
                            Language::En => println!("{}", i18n::t(agent.language, "lang_set_en")),
                            Language::Zh => println!("{}", i18n::t(agent.language, "lang_set_zh")),
                        }
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
            Ok(CommandResult::Continue)
        }
"""

if old_header not in text:
    raise SystemExit("header block not found")
text = text.replace(old_header, new_header, 1)

text = text.replace(
    'println!("Usage: /model <model-name>");',
    'println!("Usage: /model <model-name>  (alias: /模型 /moxing)");',
    1,
)

needle = 'println!("Usage: /mode <readonly|workspace_write|danger|ask|allow>");\n            } else {'
if needle not in text:
    raise SystemExit("mode usage needle not found")
text = text.replace(
    needle,
    'println!("Usage: /mode <readonly|workspace_write|danger|ask|allow>");\n'
    '                println!("       (alias: /模式 /权限 /moshi /quanxian)");\n'
    "            } else {",
    1,
)

text = text.replace(
    'println!("Session cleared.");',
    'println!("{}", i18n::t(agent.language, "session_cleared"));',
    1,
)

old_sessions = """        "sessions" | "list" => {
            let parts: Vec<&str> = arg.splitn(2, " ").collect();
            let sub = parts.first().copied().unwrap_or("");
            match sub {
                "" | "list" | "ls" => {
"""
new_sessions = """        "sessions" => {
            let parts: Vec<&str> = arg.splitn(2, " ").collect();
            let sub_raw = parts.first().copied().unwrap_or("");
            let sub = if sub_raw.is_empty() {
                "list"
            } else {
                canonicalize_sessions_sub(sub_raw)
            };
            match sub {
                "list" => {
"""
if old_sessions not in text:
    raise SystemExit("sessions block not found")
text = text.replace(old_sessions, new_sessions, 1)

text = text.replace('"rm" | "delete" => {', '"rm" => {', 1)
text = text.replace('"open" | "switch" => {', '"open" => {', 1)
text = text.replace('"todo" | "todos" => {', '"todo" => {', 1)

old_reload = """                Ok(()) => {
                    println!("Config reloaded.");
                    println!("  Model:      {}", agent.api_client.model());
                    println!("  Permission: {}", agent.permission_policy.active_mode);
                    println!("  Max turns:  {}", agent.max_turns);
                }
"""
new_reload = """                Ok(()) => {
                    println!("{}", i18n::t(agent.language, "config_reloaded"));
                    println!("  Model:      {}", agent.api_client.model());
                    println!("  Permission: {}", agent.permission_policy.active_mode);
                    println!(
                        "  {}:     {} ({})",
                        i18n::t(agent.language, "language"),
                        agent.language.as_str(),
                        agent.language.display_name()
                    );
                    println!("  Max turns:  {}", agent.max_turns);
                }
"""
if old_reload not in text:
    raise SystemExit("reload block not found")
text = text.replace(old_reload, new_reload, 1)

old_status = """            println!("Session:    {} ({})", agent.session.display_title(), agent.session.short_id());
            println!("Model:      {}", agent.api_client.model());
            println!("Permission: {}", agent.permission_policy.active_mode);
"""
new_status = """            println!("Session:    {} ({})", agent.session.display_title(), agent.session.short_id());
            println!("Model:      {}", agent.api_client.model());
            println!(
                "{}:   {} ({})",
                i18n::t(agent.language, "language"),
                agent.language.as_str(),
                agent.language.display_name()
            );
            println!("Permission: {}", agent.permission_policy.active_mode);
"""
if old_status not in text:
    raise SystemExit("status block not found")
text = text.replace(old_status, new_status, 1)

old_unknown = 'eprintln!("Unknown command: /{cmd}. Type /help for available commands.");'
new_unknown = '''eprintln!(
                "Unknown command: /{raw_cmd}. {}",
                i18n::t(agent.language, "unknown_command_hint")
            );'''
if old_unknown not in text:
    raise SystemExit("unknown command line not found")
text = text.replace(old_unknown, new_unknown, 1)

# MCP subcommand canonicalize
old_mcp = """    let parts: Vec<&str> = arg.split_whitespace().collect();
    let sub = parts.first().copied().unwrap_or("");

    match sub {
        "" | "list" | "ls" => {
"""
new_mcp = """    let parts: Vec<&str> = arg.split_whitespace().collect();
    let sub_raw = parts.first().copied().unwrap_or("");
    let sub = if sub_raw.is_empty() {
        "list"
    } else {
        canonicalize_mcp_sub(sub_raw)
    };

    match sub {
        "list" => {
"""
if old_mcp not in text:
    raise SystemExit("mcp block not found")
text = text.replace(old_mcp, new_mcp, 1)

# print_help rewrite
old_help_start = "fn print_help() {"
if old_help_start not in text:
    raise SystemExit("print_help not found")
start = text.index(old_help_start)
end = text.find("\n}\n", start)
if end < 0:
    raise SystemExit("print_help end not found")
end = end + 2  # include }
new_help = '''fn print_help(lang: Language) {
    if lang.is_zh() {
        println!("Pigs Agent — 可用命令：");
        println!();
        println!("  /help, /帮助, /bangzhu   显示帮助");
        println!("  /lang, /语言, /中文      查看或设置语言 (en|zh)");
        println!("  /model, /模型            切换模型");
        println!("  /mode, /模式, /权限      设置权限模式");
        println!("  /tools, /工具            列出工具");
        println!("  /todo, /待办             显示待办");
        println!("  /status, /状态           状态仪表盘");
        println!("  /info, /信息             当前会话信息");
        println!("  /cost, /费用             Token 用量与费用");
        println!("  /history, /历史          会话历史摘要");
        println!("  /mcp                     管理 MCP 服务器");
        println!("  /skills, /技能           技能目录（全文用 skill 工具）");
        println!("  /export, /导出           导出会话为 markdown");
        println!("  /hooks, /钩子            查看 hooks");
        println!("  /doctor, /诊断           环境健康检查");
        println!("  /models, /模型列表       供应商/模型目录");
        println!("  /init, /初始化           创建默认配置");
        println!("  /reload, /重载           热重载配置");
        println!("  /compact, /压缩          手动压缩上下文");
        println!("  /clear, /清空            清空当前会话");
        println!("  /save, /保存             保存当前会话");
        println!("  /sessions, /会话         管理会话");
        println!("  /quit, /退出             退出");
        println!();
        println!("中文与拼音别名在任何语言设置下都可用。");
        println!("输入其它文字将作为提示发送给 Agent。");
    } else {
        println!("Pigs Agent — Available Commands:");
        println!();
        println!("  /help, /h, /?            Show this help message");
        println!("  /lang <en|zh|中文>       Show or set UI/reply language");
        println!("  /model <name>            Switch model (alias: /模型 /moxing)");
        println!("  /mode <mode>             Permission mode (alias: /模式 /权限)");
        println!("  /tools                   List available tools");
        println!("  /todo                    Show the current todo list");
        println!("  /status                  Dashboard (session/model/tools/mcp)");
        println!("  /info                    Current session info");
        println!("  /cost                    Token usage and estimated cost");
        println!("  /history                 Session message history summary");
        println!("  /mcp                     Manage MCP servers (/mcp help)");
        println!("  /skills [reload]         Skill catalog (full body via `skill` tool)");
        println!("  /export [path]           Export session to markdown");
        println!("  /hooks                   Show configured tool lifecycle hooks");
        println!("  /doctor                  Environment/config health checks");
        println!("  /models                  List providers/models (with context_window)");
        println!("  /init                    Create default config at ~/.pigs/config.toml");
        println!("  /reload                  Reload config from disk and env");
        println!("  /compact                 Manually compact the session context");
        println!("  /clear                   Clear the current session");
        println!("  /save                    Save the current session");
        println!("  /sessions ...            Manage saved sessions");
        println!("  /quit, /q                Exit the agent");
        println!();
        println!("Chinese / pinyin aliases work regardless of language setting.");
        println!("  e.g. /帮助 /状态 /退出 /zhongwen /zhuangtai /tuichu");
        println!("Type any other text to send it to the agent as a prompt.");
    }
}'''
text = text[:start] + new_help + text[end + 1 :]

path.write_text(text, encoding="utf-8")
print("patched", path)
