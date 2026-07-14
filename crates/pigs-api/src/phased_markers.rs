//! 控制面标记。相位结束条件为工具循环空闲；标记仅用于路由。
//! Control-plane markers. Phase end is tool-loop idle; markers only route.

/// 成功结束标记（整轮完成）。
/// Successful turn-end marker (whole turn done).
pub const PIGEND: &str = "PIGEND";

/// Failed-path marker (return to Pre for replanning).
pub const PIGFAIL: &str = "PIGFAIL";

/// 标记类型枚举。
/// Marker type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// PIGEND — 整轮正常结束。
    /// PIGEND — turn ends successfully.
    End,
    /// PIGFAIL — 路径失败，需要重规划。
    /// PIGFAIL — path failed, needs replan.
    Failed,
}

/// Detects a control marker only when it is the final non-empty line.
/// A completion marker is valid only when an earlier non-marker line gives a reason.
pub fn detect_marker(text: &str) -> Option<Marker> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let last = lines.last()?;
    if lines[..lines.len() - 1]
        .iter()
        .all(|line| control_marker(line).is_some() || line.trim().is_empty())
    {
        return None;
    }
    control_marker(last)
}

pub(crate) fn is_control_marker_line(line: &str) -> bool {
    control_marker(line).is_some()
}

fn control_marker(line: &str) -> Option<Marker> {
    let cleaned = line
        .trim()
        .trim_matches('`')
        .trim_matches('*')
        .trim_end_matches(['.', '!', '。', '！'])
        .trim();
    match cleaned {
        PIGEND => Some(Marker::End),
        PIGFAIL => Some(Marker::Failed),
        _ => None,
    }
}

/// 从用户可见的最终文本中清除路由标记行。
/// Strip routing marker lines from user-visible final text.
pub fn strip_markers(text: &str) -> String {
    text.lines()
        .filter(|line| control_marker(line).is_none())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn detects_end_and_fail_only_on_the_last_non_empty_line() {
        assert_eq!(detect_marker("hello\nPIGEND\n"), Some(Marker::End));
        assert_eq!(detect_marker("x\nPIGFAIL"), Some(Marker::Failed));
        assert_eq!(detect_marker("no markers here"), None);
        assert_eq!(detect_marker("PIGEND\nstill working"), None);
        assert_eq!(detect_marker("PIGFAIL\nrecovered"), None);
    }

    #[test]
    fn marker_requires_a_non_marker_reason() {
        assert_eq!(detect_marker("PIGEND"), None);
        assert_eq!(detect_marker("PIGFAIL"), None);
        assert_eq!(detect_marker("\nPIGEND\n"), None);
        assert_eq!(detect_marker("reason\nPIGEND"), Some(Marker::End));
    }

    #[test]
    fn strip_removes_only_exact_control_lines() {
        assert_eq!(strip_markers("answer\nPIGEND"), "answer");
        assert_eq!(strip_markers("PIGFAIL\nretry"), "retry");
        assert_eq!(
            strip_markers("PIGEND appears in this sentence\nPIGEND"),
            "PIGEND appears in this sentence"
        );
    }
}
