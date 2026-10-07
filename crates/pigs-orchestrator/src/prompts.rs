//! 相位指令组装：模板 `include_str!` 嵌入，语言按用户问题自动选择。
//!
//! 这里只负责"指令正文"，**不含用户原问题**——命令与产物的接法由编排层决定，
//! 与 legacy `pigs-prompts` 的模板逐字节一致：
//! - Pre：模板 + 历次失败路径；
//! - Executor：模板里填 `{pre_output}`（Pre 的分析）；
//! - Post：纯模板（Executor 的草稿不走这里，而是作为 assistant 消息接回对话）。

use crate::lang::Lang;

fn norm(s: &'static str) -> String {
    // Windows 检出会让 include_str! 带上 CRLF，统一归一为 LF
    s.replace("\r\n", "\n")
}

fn pre_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/pre_user_zh.txt"),
        Lang::En => include_str!("../prompts/pre_user_en.txt"),
    })
}
fn executor_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/executor_user_zh.txt"),
        Lang::En => include_str!("../prompts/executor_user_en.txt"),
    })
}
fn post_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/post_user_zh.txt"),
        Lang::En => include_str!("../prompts/post_user_en.txt"),
    })
}
fn failure_paths_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/failure_paths_zh.txt"),
        Lang::En => include_str!("../prompts/failure_paths_en.txt"),
    })
}

/// Pre pig 的指令：追加到最后一条 user 消息文本后面（含历次失败路径）。
pub fn pre_instruction(lang: Lang, failure_paths: &[String]) -> String {
    let template = pre_template(lang);
    let fp = failure_paths_block(lang, failure_paths);
    template.replace("{failure_paths}", &fp)
}

/// Executor pig 的指令：追加到最后一条 user 消息文本后面（含 Pre 的分析）。
pub fn executor_instruction(lang: Lang, pre_output: &str) -> String {
    executor_template(lang).replace("{pre_output}", pre_output)
}

/// Post pig 的指令：作为**新的一条** user 消息追加（草稿已作为 assistant 消息在场）。
pub fn post_instruction(lang: Lang) -> String {
    post_template(lang)
}


/// 失败路径块：空列表 → 空串；非空 → 模板 + 编号列表（legacy 同款编号格式）。
fn failure_paths_block(lang: Lang, paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let template = failure_paths_template(lang);
    let entry = match lang {
        Lang::Zh => "第 {n} 次失败：\n{failure}",
        Lang::En => "Failure {n}:\n{failure}",
    };
    let entries = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            entry
                .replace("{n}", &(i + 1).to_string())
                .replace("{failure}", p)
        })
        .collect::<Vec<_>>()
        .join("\n");
    template.replace("{failures}", entries.trim_end())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn pre_instruction_embeds_failure_paths() {
        let p = pre_instruction(Lang::Zh, &[]);
        assert!(p.contains("本次需要你先思考以下问题的答案"));
        assert!(!p.contains("曾失败过"));
        assert!(p.contains("PIGEND"));
        assert!(p.contains("计划中可以加入本任务需要的执行原则"));
        assert!(p.contains("在输出给用户前会被删除"));
        assert!(!p.contains("按通常语义直接执行"));
        assert!(p
            .trim_end()
            .ends_with("输出以上问题的思考答案以及判定理由。"));

        let p = pre_instruction(Lang::Zh, &[String::from("第一次尝试报告")]);
        assert!(p.contains("曾失败过"));
        assert!(p.contains("第 1 次失败：\n第一次尝试报告"));
    }

    #[test]
    fn executor_instruction_fills_pre_output() {
        let p = executor_instruction(Lang::Zh, "计划X");
        assert!(p.starts_with("计划X\n\n"));
        assert!(p
            .trim_end()
            .ends_with("以上是本任务的执行前分析，可在执行过程中根据实际情况调整。"));
        assert!(!p.contains("自主核验") && !p.contains("完成任务目标"));
        assert!(!p.contains("多种合理理解") && !p.contains("简单、直观"));
        // 不含用户问题（问题留在原 user 消息里，由 body 手术追加）
        assert!(!p.contains("帮我完成任务"));
        // 不再有 legacy 之外的自造块
        assert!(!p.contains("评审反馈") && !p.contains("上轮"));
    }

    #[test]
    fn post_instruction_is_pure_template() {
        let p = post_instruction(Lang::Zh);
        assert!(p.starts_with("根据任务要求和任务目标，独立核验当前执行结果"));
        assert!(p.contains("PIGEND") && p.contains("PIGNEXT") && p.contains("PIGFAIL"));
        assert!(!p.contains("执行结果：") && !p.contains("以下是需要验收"));
    }


    #[test]
    fn english_templates_exist() {
        assert!(pre_instruction(Lang::En, &[])
            .contains("First think through the answers to the following questions"));
        assert!(executor_instruction(Lang::En, "PRE").contains("PRE"));
        assert!(post_instruction(Lang::En).contains("PIGNEXT"));
        assert!(post_instruction(Lang::En).contains("PIGFAIL"));
    }
}
