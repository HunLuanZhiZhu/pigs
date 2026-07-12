//! Phased agent prompt templates as plain-text files.
//!
//! All prompts live as `.txt` files in `prompts/` next to this source file.
//! They are compiled in at build time via `include_str!`, so no runtime file
//! access is needed — but humans can read and edit them as plain text.
//!
//! Language follows `AppConfig.language` (`zh` default, or `en`).
//!
//! 模板文件为纯文本，放在 `prompts/` 目录，编译时用 `include_str!` 嵌入。
//! 人类可直接用文本编辑器查看和修改；无需运行时文件访问。
//! 语言跟随 `AppConfig.language`（默认 `zh`，或 `en`）。

use pigs_config::Language;

// --- Phase system prompts (static, no variable substitution) ---

/// PRE 相位 system prompt / PRE phase system prompt.
pub fn pre_prompt(lang: Language) -> &'static str {
    match lang {
        Language::Zh => include_str!("../prompts/pre_zh.txt"),
        Language::En => include_str!("../prompts/pre_en.txt"),
    }
}

/// EXECUTOR 相位 system prompt / EXECUTOR phase system prompt.
pub fn executor_prompt(lang: Language) -> &'static str {
    match lang {
        Language::Zh => include_str!("../prompts/executor_zh.txt"),
        Language::En => include_str!("../prompts/executor_en.txt"),
    }
}

/// POST 相位 system prompt / POST phase system prompt.
pub fn post_prompt(lang: Language) -> &'static str {
    match lang {
        Language::Zh => include_str!("../prompts/post_zh.txt"),
        Language::En => include_str!("../prompts/post_en.txt"),
    }
}

// --- Phase user payloads (variable substitution via .replace()) ---

/// PRE 相位 user payload / PRE phase user payload.
///
/// Variables: `{failure_paths}` — formatted list of prior failure paths
/// (empty → "(no failure paths)" / "(无失败路径)").
pub fn pre_user_payload(lang: Language, failure_paths: &[String]) -> String {
    let template = match lang {
        Language::Zh => include_str!("../prompts/pre_user_zh.txt"),
        Language::En => include_str!("../prompts/pre_user_en.txt"),
    };
    let fp = format_failure_paths(lang, failure_paths);
    template.replace("{failure_paths}", &fp)
}

/// EXECUTOR 相位 user payload / EXECUTOR phase user payload.
///
/// Variables: `{pre_output}` — PRE phase output text;
/// `{post_feedback}` — POST feedback block (empty → omitted).
pub fn executor_user_payload(
    lang: Language,
    pre_output: &str,
    post_feedback: &str,
) -> String {
    let template = match lang {
        Language::Zh => include_str!("../prompts/executor_user_zh.txt"),
        Language::En => include_str!("../prompts/executor_user_en.txt"),
    };
    let feedback_block = if post_feedback.is_empty() {
        String::new()
    } else {
        match lang {
            Language::Zh => format!("--- POST 反馈（请据此修订）---\n{post_feedback}"),
            Language::En => format!("--- POST feedback (revise accordingly) ---\n{post_feedback}"),
        }
    };
    template
        .replace("{pre_output}", pre_output)
        .replace("{post_feedback}", &feedback_block)
}

/// POST 相位 user payload / POST phase user payload.
///
/// Variables: `{pre_output}` — PRE phase output text;
/// `{executor_draft}` — EXECUTOR phase draft text.
pub fn post_user_payload(
    lang: Language,
    pre_output: &str,
    executor_draft: &str,
) -> String {
    let template = match lang {
        Language::Zh => include_str!("../prompts/post_user_zh.txt"),
        Language::En => include_str!("../prompts/post_user_en.txt"),
    };
    template
        .replace("{pre_output}", pre_output)
        .replace("{executor_draft}", executor_draft)
}

// --- Helpers ---

/// Format failure paths as a numbered list; empty → placeholder line.
fn format_failure_paths(lang: Language, paths: &[String]) -> String {
    if paths.is_empty() {
        match lang {
            Language::Zh => "（无失败路径）".to_string(),
            Language::En => "(no failure paths)".to_string(),
        }
    } else {
        match lang {
            Language::Zh => {
                let mut s = String::new();
                s.push_str("本轮此前失败路径（请避免重复）：\n");
                for (i, f) in paths.iter().enumerate() {
                    s.push_str(&format!("{}. {f}\n", i + 1));
                }
                s.trim_end().to_string()
            }
            Language::En => {
                let mut s = String::new();
                s.push_str("Previous failure path(s) for this turn (avoid repeating):\n");
                for (i, f) in paths.iter().enumerate() {
                    s.push_str(&format!("{}. {f}\n", i + 1));
                }
                s.trim_end().to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn zh_prompts_contain_chinese_and_markers() {
        let p = pre_prompt(Language::Zh);
        assert!(p.contains("PRE"));
        assert!(p.contains("PIGEND"));
        assert!(p.contains("简体中文") || p.contains("中文"));

        let e = executor_prompt(Language::Zh);
        assert!(e.contains("EXECUTOR") || e.contains("草稿"));

        let post = post_prompt(Language::Zh);
        assert!(post.contains("PIGEND"));
        assert!(post.contains("PIGFAILED"));
    }

    #[test]
    fn en_prompts_contain_english_and_markers() {
        let p = pre_prompt(Language::En);
        assert!(p.contains("PRE phase"));
        assert!(p.contains("PIGEND"));

        let post = post_prompt(Language::En);
        assert!(post.contains("PIGFAILED"));
    }

    #[test]
    fn payloads_switch_with_language() {
        let zh = pre_user_payload(Language::Zh, &[]);
        assert!(zh.contains("PRE"));
        let en = pre_user_payload(Language::En, &[]);
        assert!(en.contains("PRE phase"));
    }

    #[test]
    fn failure_paths_filled() {
        let p = pre_user_payload(Language::Zh, &["path A".into(), "path B".into()]);
        assert!(p.contains("path A"));
        assert!(p.contains("path B"));
        assert!(p.contains("1."));
        assert!(p.contains("2."));
    }

    #[test]
    fn executor_payload_fills_vars() {
        let p = executor_user_payload(Language::Zh, "my plan", "fix this");
        assert!(p.contains("my plan"));
        assert!(p.contains("fix this"));
        assert!(p.contains("POST 反馈"));
    }

    #[test]
    fn executor_payload_empty_feedback() {
        let p = executor_user_payload(Language::En, "my plan", "");
        assert!(p.contains("my plan"));
        // No feedback header should appear when feedback is empty.
        assert!(!p.contains("POST feedback"));
    }

    #[test]
    fn post_payload_fills_vars() {
        let p = post_user_payload(Language::Zh, "the plan", "the draft");
        assert!(p.contains("the plan"));
        assert!(p.contains("the draft"));
    }
}
