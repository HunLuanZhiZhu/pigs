//! 相位指令组装：按模型名称选择完整 Pre 模板，再按用户问题选择中英文。
//!
//! 这里只负责"指令正文"，**不含用户原问题**——命令与产物的接法由编排层决定，
//! DeepSeek v5 / Muse v6 模板保持历史原文；未命中的模型使用两者共同原则抽象的通用版：
//! - Pre：模板 + 历次失败路径；
//! - Executor：模板里填 `{pre_output}`（Pre 的分析）；
//! - Post：纯模板（Executor 的草稿不走这里，而是作为 assistant 消息接回对话）。

use crate::lang::Lang;

fn norm(s: &'static str) -> String {
    // Windows 检出会让 include_str! 带上 CRLF，统一归一为 LF
    s.replace("\r\n", "\n")
}

fn pre_template(lang: Lang, model: &str) -> String {
    // 入口已剥离 -pig/-pigs 后缀；只决定 Pre 模板，不修改真正的上游 model。
    let name = model.to_ascii_lowercase();
    let template = if name.contains("deepseek") {
        match lang {
            Lang::Zh => include_str!("../prompts/pre_deepseek_zh.txt"),
            Lang::En => include_str!("../prompts/pre_deepseek_en.txt"),
        }
    } else if name.contains("muse") {
        match lang {
            Lang::Zh => include_str!("../prompts/pre_muse_zh.txt"),
            Lang::En => include_str!("../prompts/pre_muse_en.txt"),
        }
    } else {
        match lang {
            Lang::Zh => include_str!("../prompts/pre_generic_zh.txt"),
            Lang::En => include_str!("../prompts/pre_generic_en.txt"),
        }
    };
    norm(template)
}
fn pre_full_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/pre_full_zh.txt"),
        Lang::En => include_str!("../prompts/pre_full_en.txt"),
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

/// 通用 Pre：供未识别模型和独立调用者使用。
pub fn pre_instruction(lang: Lang, failure_paths: &[String]) -> String {
    pre_instruction_for_model(lang, failure_paths, "")
}

/// Pre pig 的指令：按模型名称选完整模板，再嵌入失败路径。
pub fn pre_instruction_for_model(lang: Lang, failure_paths: &[String], model: &str) -> String {
    let template = pre_template(lang, model);
    let fp = failure_paths_block(lang, failure_paths);
    template.replace("{failure_paths}", &fp)
}

/// 仅用于 -pig3 实验的纯规划 Pre，不提供简单路径或控制标记说明。
pub fn pre_full_instruction(lang: Lang, failure_paths: &[String]) -> String {
    pre_full_template(lang).replace("{failure_paths}", &failure_paths_block(lang, failure_paths))
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
        assert!(p.contains("判断依据是任务本身"));
        assert!(p.contains("若是简单任务"));
        assert!(p.contains("若不是简单任务"));
        assert!(p.contains("返回用户前会被删除"));
        assert!(!p.contains("{failure_paths}"));
        assert!(!p.contains("用户补充"));

        let p = pre_instruction(Lang::Zh, &[String::from("第一次尝试报告")]);
        assert!(p.contains("曾失败过"));
        assert!(p.contains("第 1 次失败：\n第一次尝试报告"));
    }

    #[test]
    fn experimental_pre_only_plans_and_has_no_control_markers() {
        for lang in [Lang::Zh, Lang::En] {
            let prompt = pre_full_instruction(lang, &[]);
            assert!(!prompt.contains("PIGEND"));
            assert!(!prompt.contains("PIGNEXT"));
            assert!(!prompt.contains("PIGFAIL"));
            assert!(!prompt.contains("{failure_paths}"));
        }
        let zh = pre_full_instruction(Lang::Zh, &[]);
        assert!(!zh.contains("判定为简单任务"));
        assert!(zh.contains("不要在本阶段执行任务"));
    }

    #[test]
    fn model_specific_pre_uses_unmodified_full_historical_templates() {
        for lang in [Lang::Zh, Lang::En] {
            let generic = pre_instruction(lang, &[]);
            let unknown = pre_instruction_for_model(lang, &[], "unknown-model-pig");
            assert_eq!(generic, unknown);

            let (deepseek_source, muse_source) = match lang {
                Lang::Zh => (
                    include_str!("../prompts/pre_deepseek_zh.txt"),
                    include_str!("../prompts/pre_muse_zh.txt"),
                ),
                Lang::En => (
                    include_str!("../prompts/pre_deepseek_en.txt"),
                    include_str!("../prompts/pre_muse_en.txt"),
                ),
            };
            let deepseek = pre_instruction_for_model(lang, &[], "DeEpSeEk-v4.1-flash");
            let muse = pre_instruction_for_model(lang, &[], "MuSe-spark-1.3-contributor");
            assert_eq!(deepseek, norm(deepseek_source).replace("{failure_paths}", ""));
            assert_eq!(muse, norm(muse_source).replace("{failure_paths}", ""));
            assert_ne!(deepseek, muse);
            assert_ne!(deepseek, generic);
            assert_ne!(muse, generic);
            assert_eq!(deepseek.matches("PIGEND").count(), norm(deepseek_source).matches("PIGEND").count());
            assert_eq!(muse.matches("PIGEND").count(), norm(muse_source).matches("PIGEND").count());

            let with_failure = pre_instruction_for_model(
                lang, &[String::from("historical failure")], "deepseek-v4",
            );
            assert!(with_failure.contains("historical failure"));
            assert!(!with_failure.contains("{failure_paths}"));
        }
    }

    #[test]
    fn executor_instruction_fills_pre_output() {
        let p = executor_instruction(Lang::Zh, "计划X");
        assert!(p.starts_with("计划X\n\n"));
        assert!(p
            .trim_end()
            .ends_with("以上是本任务的执行前分析，可在执行过程中根据实际情况调整。对于暂时无法自主获取的信息，应结合任务目标和现有上下文作出最简单的合理推定，主动推进并完成任务。"));
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
        assert!(p.starts_with("根据任务目标和任务要求，独立核验当前执行结果"));
        assert!(p.contains("已有充分依据后停止核验"));
        assert!(p.contains("现有上下文和可自主获取的信息已经不足"));
        assert!(p.contains("已经发生无法修改的错误"));
        assert!(p.contains("PIGEND") && p.contains("PIGNEXT") && p.contains("PIGFAIL"));
        assert!(!p.contains("执行结果：") && !p.contains("以下是需要验收"));
    }


    #[test]
    fn english_templates_exist() {
        let pre = pre_instruction(Lang::En, &[]);
        assert!(pre.contains("First think through the answers to the following questions"));
        assert!(pre.contains("Base the decision on the task itself"));
        assert!(pre.contains("If the task is simple"));
        assert!(pre.contains("If the task is not simple"));
        assert!(executor_instruction(Lang::En, "PRE").contains("PRE"));
        assert!(post_instruction(Lang::En).contains("PIGNEXT"));
        assert!(post_instruction(Lang::En).contains("PIGFAIL"));
    }
}
