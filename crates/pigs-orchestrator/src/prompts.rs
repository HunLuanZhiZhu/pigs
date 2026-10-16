//! 相位指令组装：按用户问题选择中英文，并组合共享的 Pre 分析片段与自适应路由片段。
//!
//! 这里只负责"指令正文"，**不含用户原问题**——命令与产物的接法由编排层决定：
//! - Adaptive Pre（-pig / -pigs）：共享分析片段 + 路由/执行许可片段 + 历次失败路径；
//! - Full-path Pre（-pig3 / -pigs3）：仅共享分析片段 + 历次失败路径；
//! - Exec：模板里填 `{pre_output}`（Pre 的分析）；
//! - Post：纯模板（Exec 的草稿不走这里，而是作为 assistant 消息接回对话）。

use crate::lang::Lang;

fn norm(s: &'static str) -> String {
    // Windows 检出会让 include_str! 带上 CRLF，统一归一为 LF
    s.replace("\r\n", "\n")
}

fn pre_analysis_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/pre_analysis_zh.txt"),
        Lang::En => include_str!("../prompts/pre_analysis_en.txt"),
    })
}

fn pre_route_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/pre_route_zh.txt"),
        Lang::En => include_str!("../prompts/pre_route_en.txt"),
    })
}
fn exec_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/exec_user_zh.txt"),
        Lang::En => include_str!("../prompts/exec_user_en.txt"),
    })
}
fn post_template(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/post_user_zh.txt"),
        Lang::En => include_str!("../prompts/post_user_en.txt"),
    })
}

/// Post 缺少合法终止标记时，作为新的 user 消息追加的协议纠正提醒。
pub fn control_protocol_retry_instruction(lang: Lang) -> String {
    norm(match lang {
        Lang::Zh => include_str!("../prompts/control_protocol_retry_zh.txt"),
        Lang::En => include_str!("../prompts/control_protocol_retry_en.txt"),
    })
    .trim()
    .to_string()
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

/// Adaptive Pre：共享分析片段 + 路由/执行许可片段；保留 model 参数仅为调用方兼容。
pub fn pre_instruction_for_model(lang: Lang, failure_paths: &[String], _model: &str) -> String {
    let analysis = pre_analysis_template(lang)
        .replace("{failure_paths}", &failure_paths_block(lang, failure_paths));
    format!("{}{}", analysis, pre_route_template(lang))
}

/// Full-path Pre：与 Adaptive Pre 共享完全相同的分析片段，但不追加路由/控制标记说明。
pub fn pre_analysis_instruction(lang: Lang, failure_paths: &[String]) -> String {
    pre_analysis_template(lang)
        .replace("{failure_paths}", &failure_paths_block(lang, failure_paths))
}

/// Exec pig 的指令：追加到最后一条 user 消息文本后面（含 Pre 的分析）。
pub fn exec_instruction(lang: Lang, pre_output: &str) -> String {
    exec_template(lang).replace("{pre_output}", pre_output)
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
        assert!(p.contains("简洁回答以下问题"));
        assert!(!p.contains("曾失败过"));
        assert!(p.contains("PIGEND"));
        assert!(p.contains("若为复杂任务，禁止直接开始执行任务"));
        assert!(p.contains("若为简单任务，回答完上述问题后"));
        assert!(p.contains("PIGNEXT"));
        assert!(p.contains("在输出前会被删除"));
        assert!(!p.contains("{failure_paths}"));
        assert!(!p.contains("用户补充"));

        let p = pre_instruction(Lang::Zh, &[String::from("第一次尝试报告")]);
        assert!(p.contains("曾失败过"));
        assert!(p.contains("第 1 次失败：\n第一次尝试报告"));
    }

    #[test]
    fn full_path_pre_reuses_analysis_and_has_no_route_markers() {
        for lang in [Lang::Zh, Lang::En] {
            let full = pre_analysis_instruction(lang, &[]);
            let adaptive = pre_instruction(lang, &[]);
            let route = pre_route_template(lang);

            assert_eq!(adaptive, format!("{full}{route}"));
            assert!(!full.contains("PIGEND"));
            assert!(!full.contains("PIGNEXT"));
            assert!(!full.contains("PIGFAIL"));
            assert!(!full.contains("{failure_paths}"));
        }
        let zh = pre_analysis_instruction(Lang::Zh, &[]);
        assert!(zh.contains("简洁回答以下问题，不允许开始执行任务"));
        assert!(zh.contains("任务最终需要完成的目标是什么"));
        assert!(!zh.contains("判断任务为简单任务"));
    }

    #[test]
    fn adaptive_pre_is_shared_by_all_models() {
        for lang in [Lang::Zh, Lang::En] {
            let generic = pre_instruction(lang, &[]);
            let unknown = pre_instruction_for_model(lang, &[], "unknown-model-pig");
            assert_eq!(generic, unknown);

            let analysis = pre_analysis_template(lang).replace("{failure_paths}", "");
            let route = pre_route_template(lang);
            let deepseek = pre_instruction_for_model(lang, &[], "DeEpSeEk-v4.1-flash");
            let muse = pre_instruction_for_model(lang, &[], "MuSe-spark-1.3-contributor");
            assert_eq!(generic, format!("{analysis}{route}"));
            assert_eq!(deepseek, generic);
            assert_eq!(muse, generic);
            assert!(generic.contains("PIGNEXT"));

            let with_failure = pre_instruction_for_model(
                lang, &[String::from("historical failure")], "deepseek-v4",
            );
            assert!(with_failure.contains("historical failure"));
            assert!(!with_failure.contains("{failure_paths}"));
        }
    }

    #[test]
    fn exec_instruction_fills_pre_output() {
        let p = exec_instruction(Lang::Zh, "计划X");
        assert!(p.starts_with("计划X\n\n"));
        assert_eq!(
            p,
            norm(include_str!("../prompts/exec_user_zh.txt"))
                .replace("{pre_output}", "计划X")
        );
        assert!(p.contains("完成任务目标"));
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
        assert_eq!(p, norm(include_str!("../prompts/post_user_zh.txt")));
        assert!(p.contains("核验过程中可以使用可用工具"));
        assert!(p.contains("已经进行任务相关的不可逆的操作"));
        assert!(p.contains("无论是否正确"));
        assert!(p.contains("PIGEND") && p.contains("PIGNEXT") && p.contains("PIGFAIL"));
        assert!(!p.contains("执行结果：") && !p.contains("以下是需要验收"));
    }


    #[test]
    fn english_templates_exist() {
        let pre = pre_instruction(Lang::En, &[]);
        assert!(pre.contains("Answer the following questions concisely. Do not begin executing the task"));
        assert!(pre.contains("classify the task as simple only if each question is simple"));
        assert!(pre.contains("only side-effect-free operations are allowed"));
        assert!(pre.contains("PIGNEXT"));
        assert!(exec_instruction(Lang::En, "PRE").contains("PRE"));
        assert!(post_instruction(Lang::En).contains("PIGNEXT"));
        assert!(post_instruction(Lang::En).contains("PIGFAIL"));
    }
}
