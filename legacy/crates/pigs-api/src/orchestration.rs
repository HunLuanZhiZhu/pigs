//! 纯相位编排状态机 —— 被传输相关的各运行时共享。
//! Pure phase orchestration state shared by transport-specific runtimes.
//!
//! 本模块只负责"状态机推进逻辑"：给定一个相位的原始文本输出，决定
//! 下一步是 `Complete`（整轮结束）还是 `Continue(下一相位)`，并对
//! 重新规划（PIGFAIL → 回 Pre）和无标记 Post 重试施加预算。
//!
//! This module only owns the state-machine advance logic: given one phase's
//! raw text output, it decides whether the next step is `Complete` (turn
//! done) or `Continue(next phase)`, and enforces budgets on replanning
//! (PIGFAIL → back to Pre) and markerless Post retries.

use crate::phased_markers::{detect_marker, strip_markers, Marker};
use crate::phased_phase::Phase;

/// 一轮相位对话的迭代预算。
/// Iteration limits for one phased turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrchestrationLimits {
    /// Post 阶段无标记输出的最大保留次数，超过即判定本轮失败。
    /// Maximum markerless Post outputs retained before failing the turn.
    pub max_post_iterations: u32,
    /// 允许 PIGFAIL 回到 Pre 重规划的最大次数。
    /// Maximum Post failures that may return to Pre.
    pub max_pre_replans: u32,
}

impl Default for OrchestrationLimits {
    /// 默认预算：Post 重试 3 次、Pre 重规划 2 次。
    /// Default budgets: 3 markerless Post retries, 2 Pre replans.
    fn default() -> Self {
        Self {
            max_post_iterations: 3,
            max_pre_replans: 2,
        }
    }
}

/// 将一次完成的模型输出应用到状态机后的结果。
/// Result of applying one completed model output to the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// 本轮通过合法的 `PIGEND` 标记正常完成。
    /// The turn has completed through a valid `PIGEND` marker.
    Complete,
    /// 需要继续执行指定的相位。
    /// Continue by executing the given phase.
    Continue(Phase),
}

/// 明确的终态失败。预算耗尽绝不会被视为成功结果。
/// Explicit terminal failures. Budget exhaustion is never a successful result.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrchestrationError {
    /// 无标记 Post 迭代超出配置预算。
    /// Markerless Post iterations exceeded their configured budget.
    #[error("Post phase budget exhausted after {limit} markerless iteration(s)")]
    PostBudgetExceeded {
        /// 触发失败的预算上限 / the limit that was exceeded.
        limit: u32,
    },
    /// 失败重规划超出配置预算。
    /// Failed replans exceeded their configured budget.
    #[error("Pre replan budget exhausted after {limit} failure(s)")]
    ReplanBudgetExceeded {
        /// 触发失败的预算上限 / the limit that was exceeded.
        limit: u32,
    },
}

/// 纯 Pre -> Executor -> Post 编排状态。
/// Pure Pre -> Executor -> Post orchestration state.
///
/// 不持有任何原生 JSON、传输句柄或可变请求体；只记录各相位的可见文本
/// 与计数器。这让 HTTP 运行时可以把原生数据面与编排控制面分离维护。
///
/// Holds no native JSON, transport handles, or mutable request bodies — only
/// per-phase visible text and counters. This lets the HTTP runtime keep the
/// native data plane and the orchestration control plane separate.
#[derive(Debug, Clone)]
pub struct OrchestrationState {
    /// 当前相位 / current phase.
    phase: Phase,
    /// 本轮预算 / limits for this turn.
    limits: OrchestrationLimits,
    /// 最近一次 Pre 的可见输出（计划/GOAL）/ latest Pre visible output (plan/GOAL).
    pre_output: String,
    /// Executor 各次输出，保留供 Post 审阅 / all Executor outputs retained for Post review.
    executor_outputs: Vec<String>,
    /// Post 各次输出，包括触发重规划的那一次 / all Post outputs, including the one that triggered a replan.
    post_outputs: Vec<String>,
    /// 完整的失败 Pre/Post 输出（用于注入下一次 Pre 提示词）/ complete failed Pre/Post outputs (injected into the next Pre prompt).
    failure_outputs: Vec<String>,
    /// 无标记 Post 重试计数器 / markerless Post retry counter.
    post_iterations: u32,
    /// PIGFAIL 回到 Pre 的重规划计数器 / PIGFAIL → Pre replan counter.
    pre_replans: u32,
}

impl OrchestrationState {
    /// 创建一个从 Pre 开始的新轮次。
    /// Creates a turn beginning in Pre.
    pub fn new(limits: OrchestrationLimits) -> Self {
        Self {
            phase: Phase::Pre,
            limits,
            pre_output: String::new(),
            executor_outputs: Vec::new(),
            post_outputs: Vec::new(),
            failure_outputs: Vec::new(),
            post_iterations: 0,
            pre_replans: 0,
        }
    }

    /// 返回下一步必须执行的相位。
    /// Returns the phase that must run next.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 将当前相位的一次完成输出应用到状态机。
    ///
    /// `output` 是该相位的**原始**文本（含控制标记），本方法内部会：
    /// 1. 用 `strip_markers` 得到可见文本；
    /// 2. 用 `detect_marker` 判断是否 PIGEND / PIGFAIL；
    /// 3. 按当前相位推进状态：Pre 可结束/重规划/进入 Executor，
    ///    Executor 直接进入 Post，Post 可结束/重规划/无标记重试。
    ///
    /// Applies the completed output of the current phase.
    ///
    /// `output` is the phase's **raw** text (with control markers). This
    /// method: (1) strips markers to get visible text; (2) detects
    /// PIGEND/PIGFAIL; (3) advances state per the current phase: Pre may
    /// end/replan/enter Executor, Executor always enters Post, Post may
    /// end/replan/retry-without-marker.
    pub fn advance(&mut self, output: &str) -> Result<Advance, OrchestrationError> {
        // 先剥离控制标记得到用户可见文本 / Strip markers to get user-visible text.
        let visible = strip_markers(output);
        match self.phase {
            // ------------------------------------------------------------
            // Pre 相位：解析 PIGEND/PIGFAIL，或进入 Executor
            // Pre phase: parse PIGEND/PIGFAIL, or enter Executor
            // ------------------------------------------------------------
            Phase::Pre => match detect_marker(output) {
                // PIGEND → 整轮直接结束（简单路径）/ PIGEND → turn ends (simple path).
                Some(Marker::End) => Ok(Advance::Complete),
                // PIGFAIL → 记录失败路径并回到 Pre 重规划 / PIGFAIL → record failure and replan.
                Some(Marker::Failed) => self.replan(visible),
                // 无标记 → 保存计划，进入 Executor / no marker → save plan, enter Executor.
                None => {
                    self.pre_output = visible;
                    self.phase = Phase::Executor;
                    Ok(Advance::Continue(self.phase))
                }
            },
            // ------------------------------------------------------------
            // Executor 相位：总是进入 Post，保留输出供审阅
            // Executor phase: always enter Post, retain output for review
            // ------------------------------------------------------------
            Phase::Executor => {
                self.executor_outputs.push(visible);
                self.phase = Phase::Post;
                Ok(Advance::Continue(self.phase))
            }
            // ------------------------------------------------------------
            // Post 相位：解析标记，或无标记重试（受预算约束）
            // Post phase: parse markers, or retry markerless (budget-bounded)
            // ------------------------------------------------------------
            Phase::Post => {
                self.post_outputs.push(visible.clone());
                match detect_marker(output) {
                    // PIGEND → 整轮正常结束 / PIGEND → turn ends successfully.
                    Some(Marker::End) => Ok(Advance::Complete),
                    // PIGFAIL → 路径失败，回到 Pre 重规划 / PIGFAIL → path failed, replan.
                    Some(Marker::Failed) => self.replan(visible),
                    // 无标记 → 视为 Post 反馈，回环重试 / no marker → treat as Post feedback, retry.
                    None => {
                        // 超出无标记 Post 重试预算 → 报错 / Exceeded markerless Post retries → error.
                        if self.post_iterations >= self.limits.max_post_iterations {
                            return Err(OrchestrationError::PostBudgetExceeded {
                                limit: self.limits.max_post_iterations,
                            });
                        }
                        // 预算内 → 计数并继续 Post / Within budget → count and continue Post.
                        self.post_iterations += 1;
                        Ok(Advance::Continue(Phase::Post))
                    }
                }
            }
        }
    }

    /// 返回最近一次 Pre 的分析（计划/GOAL）。
    /// Returns the latest Pre analysis.
    pub fn pre_output(&self) -> &str {
        &self.pre_output
    }

    /// 返回所有保留供 Post 审阅的 Executor 输出。
    /// Returns all Executor outputs retained for Post review.
    pub fn executor_outputs(&self) -> &[String] {
        &self.executor_outputs
    }

    /// 返回所有 Post 输出（含触发重规划的那一次）。
    /// Returns all Post outputs, including the output that caused a replan.
    pub fn post_outputs(&self) -> &[String] {
        &self.post_outputs
    }

    /// 返回所有失败的 Pre/Post 完整输出。
    /// Returns complete failed Post or Pre outputs.
    pub fn failure_outputs(&self) -> &[String] {
        &self.failure_outputs
    }

    /// 返回当前 Pre 重规划次数。
    /// Returns the current Pre replan count.
    pub fn pre_replan_count(&self) -> u32 {
        self.pre_replans
    }

    /// 返回当前 Post 无标记重试次数。
    /// Returns the current Post iteration count.
    pub fn post_iteration_count(&self) -> u32 {
        self.post_iterations
    }

    /// 把失败记录格式化为下一次 Pre 提示词的编号列表。
    /// Formats failures for the next Pre prompt as a numbered list.
    pub fn numbered_failures(&self) -> String {
        self.failure_outputs
            .iter()
            .enumerate()
            .map(|(index, output)| format!("第 {} 次失败：\n{output}", index + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 内部：处理 PIGFAIL —— 记录失败、计数、清空产物、回到 Pre。
    /// Internal: handle PIGFAIL — record failure, count, clear products, return to Pre.
    fn replan(&mut self, failure: String) -> Result<Advance, OrchestrationError> {
        // 超出重规划预算 → 报错 / Exceeded replan budget → error.
        if self.pre_replans >= self.limits.max_pre_replans {
            return Err(OrchestrationError::ReplanBudgetExceeded {
                limit: self.limits.max_pre_replans,
            });
        }
        // 记录失败、计数、清空 Pre 产物与 Post 重试计数 / Record failure, count, clear Pre output and Post retries.
        self.failure_outputs.push(failure);
        self.pre_replans += 1;
        self.pre_output.clear();
        self.post_iterations = 0;
        self.phase = Phase::Pre;
        Ok(Advance::Continue(self.phase))
    }
}
