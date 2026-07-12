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
    /// When the user set a custom `system_prompt` in config, that still wins;
    /// this is only the built-in fallback.
    pub fn default_system_prompt(self) -> &'static str {
        match self {
            Self::En => {
                "You are Pigs, a helpful and capable AI assistant. \
                 You can use tools to help the user with their tasks. \
                 When you need to perform an action (read files, write files, run commands, search code, etc.), \
                 use the available tools rather than asking the user to do it manually. \
                 Always explain what you're doing and why. \
                 If a tool fails, explain the error and suggest an alternative approach. \
                 Be concise but thorough in your explanations. \
                 Prefer English unless the user clearly writes in another language."
            }
            Self::Zh => {
                "你是 Pigs，一个有能力的通用 AI 助手。\
                 你可以使用工具帮助用户完成任务。\
                 当需要执行操作（读文件、写文件、运行命令、搜索代码等）时，请直接调用可用工具，而不是让用户手动操作。\
                 始终说明你在做什么以及为什么这样做。\
                 如果工具失败，解释错误并给出替代方案。\
                 回答简洁但完整。\
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
