//! 相位标识枚举（控制面）。
//! Phase identity for the control plane.

/// 相位枚举：表示当前处于三个阶段中的哪一个。
/// Phase enum: indicates which of the three stages we are currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 规划 / 分流 / GOAL 声明。
    /// Plan / triage / goal declaration.
    Pre,
    /// 信息收集 + 起草答复。
    /// Information gathering + drafting.
    Executor,
    /// 审阅 + GOAL 验收 + 失败/重规划（单次调用，三个逻辑角色）。
    /// Review + goal check + fail/replan (single call, three logical roles).
    Post,
}

impl Phase {
    /// 返回相位的字符串标识（用于日志、事件、SSE）。
    /// Return the phase's string identifier (for logs, events, SSE).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::Executor => "executor",
            Self::Post => "post",
        }
    }
}
