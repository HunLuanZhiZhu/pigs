//! 相位指令组装：按任务语言选择模板并组合 Pre 片段。 / Assemble phase instructions by task language and compose Pre fragments.
//!
//! 这里只组装指令正文，不包含原始用户问题；编排层负责消息拼接。 / Build instruction text only; the orchestrator attaches the original request.
//! - 自适应 Pre：共享分析、路由许可与失败历史。 / Adaptive Pre combines analysis, routing permission, and failure history.
//! - 强制完整 Pre：只使用分析片段与失败历史。 / Forced-full Pre uses only analysis and failure history.
//! - Exec：插入 Pre 分析 `{pre_output}`。 / Exec interpolates the Pre analysis.
//! - Post：纯模板；Exec 草稿另以助手消息保留。 / Post is template-only; the Exec draft stays in an assistant message.

use crate::lang::Lang;

fn norm(s: &'static str) -> String {
    // Windows 检出可能产生 CRLF，统一转为 LF。 / Normalize Windows CRLF to LF.
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

/// Post 缺少合法终止标记时追加的协议提醒。 / Protocol reminder appended as a new user message when Post lacks a valid terminal marker.
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

/// 通用 Pre 指令入口。 / Generic Pre instruction entry point.
pub fn pre_instruction(lang: Lang, failure_paths: &[String]) -> String {
    pre_instruction_for_model(lang, failure_paths, "")
}

/// 自适应 Pre：组合分析与路由片段；模型参数保留供调用方兼容。 / Adaptive Pre combines analysis and routing; the model argument remains for caller compatibility.
pub fn pre_instruction_for_model(lang: Lang, failure_paths: &[String], _model: &str) -> String {
    let analysis = pre_analysis_template(lang)
        .replace("{failure_paths}", &failure_paths_block(lang, failure_paths));
    format!("{}{}", analysis, pre_route_template(lang))
}

/// 强制完整 Pre：复用相同分析片段，不追加路由标记说明。 / Forced-full Pre reuses the same analysis without routing-marker instructions.
pub fn pre_analysis_instruction(lang: Lang, failure_paths: &[String]) -> String {
    pre_analysis_template(lang)
        .replace("{failure_paths}", &failure_paths_block(lang, failure_paths))
}

/// Exec 指令追加到最后一条用户消息，包含 Pre 分析。 / Append the Exec instruction to the latest user message, including Pre analysis.
pub fn exec_instruction(lang: Lang, pre_output: &str) -> String {
    exec_template(lang).replace("{pre_output}", pre_output)
}

/// Post 指令作为新用户消息追加；草稿已在助手消息中。 / Append Post as a new user message; the draft is already in an assistant message.
pub fn post_instruction(lang: Lang) -> String {
    post_template(lang)
}


/// 失败路径块：空列表返回空串，否则生成编号历史。 / Failure-history block: empty when no failures, otherwise a numbered list.
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
        // 不包含原始问题；它留在用户消息中。 / The original user question stays in its user message.
        assert!(!p.contains("帮我完成任务"));
        // 不引入额外的历史自定义区块。 / Do not introduce extra legacy-style sections.
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
