//! Lightweight UI strings for the product binary.
//!
//! Slash-command Chinese / pinyin aliases are always accepted; this module
//! only localizes human-facing REPL chrome based on [`Language`].

use pigs_config::Language;

/// Look up a short UI string by key.
pub fn t(lang: Language, key: &str) -> &'static str {
    match (lang, key) {
        // REPL banner
        (Language::En, "banner_help") => "Type /help for commands, or just type a message to chat.",
        (Language::Zh, "banner_help") => "输入 /help 或 /帮助 查看命令；直接输入文字即可对话。",
        (Language::En, "model") => "Model",
        (Language::Zh, "model") => "模型",
        (Language::En, "permission") => "Permission",
        (Language::Zh, "permission") => "权限",
        (Language::En, "tools") => "Tools",
        (Language::Zh, "tools") => "工具",
        (Language::En, "enabled") => "enabled",
        (Language::Zh, "enabled") => "已启用",
        (Language::En, "disabled") => "disabled",
        (Language::Zh, "disabled") => "已禁用",
        (Language::En, "session") => "Session",
        (Language::Zh, "session") => "会话",
        (Language::En, "language") => "Language",
        (Language::Zh, "language") => "语言",
        (Language::En, "goodbye") => "Goodbye!",
        (Language::Zh, "goodbye") => "再见！",
        (Language::En, "ctrl_c") => "(Ctrl+C pressed. Type /quit to exit.)",
        (Language::Zh, "ctrl_c") => "（已按 Ctrl+C。输入 /quit 或 /退出 结束。）",
        (Language::En, "unknown_command_hint") => "Type /help for available commands.",
        (Language::Zh, "unknown_command_hint") => "输入 /help 或 /帮助 查看可用命令。",
        (Language::En, "session_cleared") => "Session cleared.",
        (Language::Zh, "session_cleared") => "会话已清空。",
        (Language::En, "config_reloaded") => "Config reloaded.",
        (Language::Zh, "config_reloaded") => "配置已重新加载。",
        (Language::En, "lang_set_en") => "Language set to English.",
        (Language::Zh, "lang_set_en") => "语言已切换为 English。",
        (Language::En, "lang_set_zh") => "Language set to 中文.",
        (Language::Zh, "lang_set_zh") => "语言已切换为中文。",
        (Language::En, "lang_usage") => "Usage: /lang <en|zh|中文>",
        (Language::Zh, "lang_usage") => "用法: /lang <en|zh|中文>  或  /语言 <en|zh|中文>",
        (Language::En, "shortcuts_label") => "Shortcuts:",
        (Language::Zh, "shortcuts_label") => "快捷键:",
        _ => "???",
    }
}
