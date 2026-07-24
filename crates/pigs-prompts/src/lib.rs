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
pub fn executor_user_payload(lang: Language, pre_output: &str, _post_feedback: &str) -> String {
    let template = match lang {
        Language::Zh => include_str!("../prompts/executor_user_zh.txt"),
        Language::En => include_str!("../prompts/executor_user_en.txt"),
    };
    template.replace("{pre_output}", pre_output)
}

/// POST 相位 user payload / POST phase user payload.
///
/// Variables: `{pre_output}` — PRE phase output text;
/// `{executor_draft}` — EXECUTOR phase draft text.
pub fn post_user_payload(lang: Language, _pre_output: &str, _executor_draft: &str) -> String {
    match lang {
        Language::Zh => include_str!("../prompts/post_user_zh.txt").to_string(),
        Language::En => include_str!("../prompts/post_user_en.txt").to_string(),
    }
}

// --- Helpers ---

/// Format failure paths as a numbered list; empty → placeholder line.
fn format_failure_paths(lang: Language, paths: &[String]) -> String {
    if paths.is_empty() {
        match lang {
            Language::Zh => String::new(),
            Language::En => String::new(),
        }
    } else {
        match lang {
            Language::Zh => {
                let mut text = String::from("这个任务执行中曾失败过\n");
                for (index, failure) in paths.iter().enumerate() {
                    text.push_str(&format!("第 {} 次失败：\n{}\n", index + 1, failure));
                }
                text.trim_end().to_string()
            }
            Language::En => {
                let mut text = String::from("This task previously failed during execution.\n");
                for (index, failure) in paths.iter().enumerate() {
                    text.push_str(&format!("Failure {}:\n{}\n", index + 1, failure));
                }
                text.trim_end().to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn zh_payloads_match_the_documented_phase_contract() {
        let pre = pre_user_payload(Language::Zh, &["失败输出".into()]);
        assert!(pre.starts_with("以上是本任务要求"));
        assert!(pre.contains("第 1 次失败：\n失败输出"));
        assert!(pre.contains("严格按要求输出这5个问题的回答"));
        assert!(pre.contains("本任务需要哪些项目内部信息？"));
        assert!(pre.contains("当且仅当1）-4）均不需要或极其简单"));
        assert!(pre.contains("不要输出 PIGEND"));
        assert!(pre.trim_end().ends_with("PIGEND"));

        let executor = executor_user_payload(Language::Zh, "完整 PRE", "ignored");
        assert!(executor.starts_with("以下是对本次任务在"));
        assert!(executor.contains("完整 PRE"));
        assert!(executor
            .trim_end()
            .ends_with("按照执行计划全力完成任务目标"));
        assert!(!executor.contains("POST 反馈"));

        let post = post_user_payload(Language::Zh, "ignored", "ignored");
        assert!(post.starts_with("根据设定的目标，独立验收结果"));
        assert!(post.contains("PIGEND"));
        assert!(post.contains("PIGFAIL"));
        assert!(!post.contains("PRE 计划"));
        assert!(!post.contains("EXECUTOR 草稿"));
    }

    #[test]
    fn english_payloads_are_semantically_equivalent() {
        let pre = pre_user_payload(Language::En, &[]);
        assert!(pre.contains("five questions"));
        assert!(pre.contains("project-internal information"));
        assert!(pre.contains("PIGEND"));

        let executor = executor_user_payload(Language::En, "complete PRE", "ignored");
        assert!(executor.contains("complete PRE"));
        assert!(executor.contains("fully complete the task goal"));

        let post = post_user_payload(Language::En, "ignored", "ignored");
        assert!(post.contains("independently verify"));
        assert!(post.contains("PIGFAIL"));
    }
}
