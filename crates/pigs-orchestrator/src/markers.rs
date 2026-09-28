//! 控制面标记：PIGEND / PIGFAIL。相位结束条件是"模型答完"；标记仅用于路由。

/// 成功结束标记（整轮完成）。
pub const PIGEND: &str = "PIGEND";
/// 失败路径标记（回到 Pre 重规划）。
pub const PIGFAIL: &str = "PIGFAIL";

/// 标记类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// PIGEND —— 整轮正常结束。
    End,
    /// PIGFAIL —— 路径失败，需要重规划。
    Failed,
}

fn control_marker(line: &str) -> Option<Marker> {
    match line.trim() {
        PIGEND => Some(Marker::End),
        PIGFAIL => Some(Marker::Failed),
        _ => None,
    }
}

/// 仅当**最后一个非空行**是控制标记时才算检测到，且要求前面存在非标记的实质行。
/// （防止模型只输出一个光秃秃的标记。移植自 legacy `phased_markers.rs`。）
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

/// 从文本中剔除所有控制标记行。
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
    fn detects_only_on_last_non_empty_line() {
        assert_eq!(detect_marker("分析……\nPIGEND\n"), Some(Marker::End));
        assert_eq!(detect_marker("x\nPIGFAIL"), Some(Marker::Failed));
        assert_eq!(detect_marker("PIGEND"), None); // 无实质行
        assert_eq!(detect_marker("PIGFAIL\n理由\nPIGEND"), Some(Marker::End));
        assert_eq!(detect_marker("普通回答，无标记"), None);
    }

    #[test]
    fn strips_marker_lines() {
        assert_eq!(strip_markers("报告内容\nPIGEND"), "报告内容");
        assert_eq!(strip_markers("a\nPIGFAIL\nb\n"), "a\nb");
        assert_eq!(strip_markers("干净文本"), "干净文本");
    }
}
