//! 相位 payload 组装：模板 `include_str!` 嵌入，语言按用户问题自动选择。
//!
//! 模板原文沿用 legacy `pigs-prompts`，只做了两处增强（legacy 缺失的输入补上）：
//! - Executor 追加 Post 评审反馈块（legacy 占位参数未使用，回环时输入不变）；
//! - Post 追加执行草稿块（legacy 未把 Executor 草稿交给评审，无从审起）。

use crate::lang::Lang;

fn norm(s: &'static str) -> String {
    // Windows 检出会让 include_str! 带上 CRLF，统一归一为 LF
    s.replace("\r\n", "\n")
}

fn pre_template(lang: Lang) -> String {
    norm(match lang { Lang::Zh => include_str!("../prompts/pre_user_zh.txt"),
                      Lang::En => include_str!("../prompts/pre_user_en.txt") })
}
fn executor_template(lang: Lang) -> String {
    norm(match lang { Lang::Zh => include_str!("../prompts/executor_user_zh.txt"),
                      Lang::En => include_str!("../prompts/executor_user_en.txt") })
}
fn post_template(lang: Lang) -> String {
    norm(match lang { Lang::Zh => include_str!("../prompts/post_user_zh.txt"),
                      Lang::En => include_str!("../prompts/post_user_en.txt") })
}
fn failure_paths_template(lang: Lang) -> String {
    norm(match lang { Lang::Zh => include_str!("../prompts/failure_paths_zh.txt"),
                      Lang::En => include_str!("../prompts/failure_paths_en.txt") })
}

/// Pre pig 的 user payload：原问题 + 分析指令（含历次失败路径）。
pub fn pre_payload(user_question: &str, lang: Lang, failure_paths: &[String]) -> String {
    let template = pre_template(lang);
    let fp = failure_paths_block(lang, failure_paths);
    format!("{}

{}", user_question, template.replace("{failure_paths}", &fp))
}

/// Executor pig 的 user payload：原问题 + 执行指令 + Pre 产物（+ 上轮评审反馈）。
pub fn executor_payload(user_question: &str, lang: Lang, pre_output: &str, post_feedback: &str) -> String {
    let template = executor_template(lang);
    let mut payload = format!("{}\n\n{}", user_question, template.replace("{pre_output}", pre_output));
    if !post_feedback.trim().is_empty() {
        payload.push_str(&match lang {
            Lang::Zh => format!("\n\n---\n以下是上轮评审反馈，请据此修正：\n{}", post_feedback),
            Lang::En => format!("\n\n---\nThe following is the previous round's review feedback; revise accordingly:\n{}", post_feedback),
        });
    }
    payload
}

/// Post pig 的 user payload：原问题 + 验收指令 + 执行草稿。
pub fn post_payload(user_question: &str, lang: Lang, pre_output: &str, executor_draft: &str) -> String {
    let template = post_template(lang);
    let _ = pre_output; // 模板目前不引用 Pre 产物；保留参数以对齐管线语义
    format!(
        "{}\n\n{}\n\n---\n{}",
        user_question,
        template,
        match lang {
            Lang::Zh => format!("以下是需要验收的执行结果：\n{}", executor_draft),
            Lang::En => format!("The following is the execution result to review:\n{}", executor_draft),
        }
    )
}

/// 失败路径块：空列表 → 空串；非空 → 模板 + 编号列表。
fn failure_paths_block(lang: Lang, paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let template = failure_paths_template(lang);
    let entries = paths
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}. {}", i + 1, p))
        .collect::<Vec<_>>()
        .join("\n");
    template.replace("{failures}", &entries)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn pre_payload_embeds_failure_paths() {
        let p = pre_payload("任务", Lang::Zh, &[]);
        assert!(p.contains("任务"));
        assert!(p.contains("执行前分析"));
        assert!(!p.contains("曾失败过"));

        let p = pre_payload("任务", Lang::Zh, &[String::from("第一次尝试报告")]);
        assert!(p.contains("曾失败过"));
        assert!(p.contains("1. 第一次尝试报告"));
    }

    #[test]
    fn executor_payload_includes_pre_output_and_feedback() {
        let p = executor_payload("任务", Lang::Zh, "计划X", "");
        assert!(p.contains("计划X"));
        assert!(!p.contains("评审反馈"));

        let p = executor_payload("任务", Lang::Zh, "计划X", "漏了第二步");
        assert!(p.contains("计划X"));
        assert!(p.contains("漏了第二步"));
    }

    #[test]
    fn post_payload_includes_draft() {
        let p = post_payload("任务", Lang::Zh, "计划X", "草稿Y");
        assert!(p.contains("PIGEND"));
        assert!(p.contains("草稿Y"));
    }
}
