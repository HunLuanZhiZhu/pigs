//! 控制面标记。相位结束条件为工具循环空闲；标记仅用于路由。
//! Control-plane markers. Phase end is tool-loop idle; markers only route.

/// 成功结束标记（整轮完成）。
/// Successful turn-end marker (whole turn done).
pub const PIGEND: &str = "PIGEND";

/// 路径失败标记（清空本轮状态并回到 PRE 重规划）。
/// Path-failed marker (clear turn-local state and replan back to pre).
pub const PIGFAILED: &str = "PIGFAILED";

/// 标记类型枚举。
/// Marker type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// PIGEND — 整轮正常结束。
    /// PIGEND — turn ends successfully.
    End,
    /// PIGFAILED — 路径失败，需要重规划。
    /// PIGFAILED — path failed, needs replan.
    Failed,
}

/// 检测文本中的控制标记。仅精确匹配；未匹配/垃圾文本 → None（默认边）。
/// 如同时出现两种标记，Failed 优先。
///
/// Detect control markers in text. Positive match only; unmatched/garbage →
/// None (default edge). If both appear, Failed wins.
pub fn detect_marker(text: &str) -> Option<Marker> {
    let mut saw_end = false;
    let mut saw_failed = false;
    for raw in text.lines() {
        let line = raw.trim();
        // 允许尾部标点噪声："PIGEND" / "PIGEND." / "`PIGEND`"
        // Allow trailing punctuation noise: "PIGEND" / "PIGEND." / "`PIGEND`"
        let cleaned = line
            .trim_matches('`')
            .trim_matches('*')
            .trim_end_matches(['.', '!', '。', '！'])
            .trim();
        if cleaned == PIGFAILED {
            saw_failed = true;
        } else if cleaned == PIGEND {
            saw_end = true;
        }
    }
    // 兜底：检查最后非空段落尾部的分词
    // Fallback: check last non-empty paragraph's whitespace-split tokens
    if !saw_end && !saw_failed {
        if let Some(last) = text
            .split_whitespace()
            .rev()
            .find(|t| t.contains("PIGEND") || t.contains("PIGFAILED"))
        {
            let t = last
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .trim();
            if t == PIGFAILED {
                saw_failed = true;
            } else if t == PIGEND {
                saw_end = true;
            }
        }
    }
    // Failed 优先于 End / Failed wins over End
    if saw_failed {
        Some(Marker::Failed)
    } else if saw_end {
        Some(Marker::End)
    } else {
        None
    }
}

/// 从用户可见的最终文本中清除路由标记行。
/// Strip routing marker lines from user-visible final text.
pub fn strip_markers(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let cleaned = line
                .trim()
                .trim_matches('`')
                .trim_matches('*')
                .trim_end_matches(['.', '!', '。', '！'])
                .trim();
            cleaned != PIGEND && cleaned != PIGFAILED
        })
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
    fn detects_end_and_failed() {
        assert_eq!(detect_marker("hello\nPIGEND\n"), Some(Marker::End));
        assert_eq!(detect_marker("x\nPIGFAILED"), Some(Marker::Failed));
        assert_eq!(detect_marker("no markers here"), None);
    }

    #[test]
    fn failed_wins_over_end() {
        assert_eq!(
            detect_marker("PIGEND\nPIGFAILED"),
            Some(Marker::Failed)
        );
    }

    #[test]
    fn strip_keeps_body() {
        assert_eq!(strip_markers("answer\nPIGEND"), "answer");
    }
}
