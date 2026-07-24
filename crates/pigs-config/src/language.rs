//! UI / agent response language preference.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Preferred language for UI strings and default agent replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English.
    En,
    /// Simplified Chinese (product default).
    #[default]
    Zh,
}

impl Language {
    /// Canonical config / CLI token (`en`, `zh`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }

    /// Human-readable label for status / help.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Zh => "中文",
        }
    }

    /// Whether this is Chinese.
    pub fn is_zh(self) -> bool {
        matches!(self, Self::Zh)
    }

    /// Default system prompt for the product agent (language-aware).
    ///
    /// Aligned with PI's system prompt structure. The agent does not know
    /// it is being orchestrated — it sees itself as "pig, a coding agent harness".
    ///
    /// When the user set a custom `system_prompt` in config, that still wins;
    /// this is only the built-in fallback.
    pub fn default_system_prompt(self) -> &'static str {
        match self {
            Self::En => {
                "You are an expert coding assistant operating inside pig, a coding agent harness. \
                 You help users by reading files, executing commands, editing code, and writing new files.\n\n\
                 Available tools:\n\
                 - read: read file contents (with line numbers, offset/limit)\n\
                 - bash: execute shell commands (with timeout)\n\
                 - edit: find/replace file edits with diff\n\
                 - write: create or overwrite files\n\n\
                 In addition to the tools above, you may have access to other custom tools depending on the project.\n\n\
                 Guidelines:\n\
                 - Use bash for file operations like ls, rg, find\n\
                 - Be concise in your responses\n\
                 - Show file paths clearly when working with files\n\n\
                 Prefer English unless the user clearly writes in another language."
            }
            Self::Zh => {
                "你是一个专业的编程助手，运行在 pig 编程智能体框架内。你通过读取文件、执行命令、编辑代码和编写新文件来帮助用户。\n\n\
                 可用工具：\n\
                 - read：读取文件内容（带行号，支持偏移/限制）\n\
                 - bash：执行 shell 命令（带超时）\n\
                 - edit：查找/替换文件内容并生成 diff\n\
                 - write：创建或覆盖文件\n\n\
                 除上述工具外，根据项目配置，你可能还可以使用其他自定义工具。\n\n\
                 准则：\n\
                 - 使用 bash 进行文件操作，如 ls、rg、find\n\
                 - 回答简洁\n\
                 - 处理文件时清晰显示文件路径\n\n\
                 默认使用简体中文回复；若用户明确要求其它语言，再切换。"
            }
        }
    }

    /// Short instruction appended when a custom system prompt is set,
    /// so language preference still applies.
    pub fn language_reminder(self) -> &'static str {
        match self {
            Self::En => "\n\n--- Language ---\nPrefer English in replies unless the user clearly writes in another language.",
            Self::Zh => "\n\n--- Language ---\n默认使用简体中文回复；若用户明确要求其它语言，再切换。",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_lowercase();
        match t.as_str() {
            "en" | "english" | "eng" => Ok(Self::En),
            "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese" | "中文" | "汉语" | "简体中文" => {
                Ok(Self::Zh)
            }
            other => Err(format!(
                "unknown language '{other}' (expected en, zh / 中文)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn parses_english_and_chinese() {
        assert_eq!("en".parse::<Language>().unwrap(), Language::En);
        assert_eq!("zh".parse::<Language>().unwrap(), Language::Zh);
        assert_eq!("中文".parse::<Language>().unwrap(), Language::Zh);
        assert_eq!("zh-CN".parse::<Language>().unwrap(), Language::Zh);
    }

    #[test]
    fn rejects_unknown() {
        assert!("jp".parse::<Language>().is_err());
    }
}
