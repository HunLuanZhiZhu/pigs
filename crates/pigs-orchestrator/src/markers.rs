//! 控制面标记只决定相位路由，相位结束仍由模型完成决定。 / Control markers determine routing; a phase still ends when model generation finishes.

/// 成功完成标记。 / Successful completion marker.
pub const PIGEND: &str = "PIGEND";
/// 修补标记：从 Post 返回 Exec。 / Repair marker: return from Post to Exec.
pub const PIGNEXT: &str = "PIGNEXT";
/// 失败标记：返回 Pre 重规划。 / Failure marker: return to Pre for replanning.
pub const PIGFAIL: &str = "PIGFAIL";

/// 控制标记类型。 / Control marker type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// PIGEND：整轮完成。 / PIGEND: turn completed.
    End,
    /// PIGNEXT：回 Exec 修补。 / PIGNEXT: return to Exec for repair.
    Next,
    /// PIGFAIL：路径失败并重规划。 / PIGFAIL: failed path requiring replanning.
    Failed,
}

fn control_marker(line: &str) -> Option<Marker> {
    match line.trim() {
        PIGEND => Some(Marker::End),
        PIGNEXT => Some(Marker::Next),
        PIGFAIL => Some(Marker::Failed),
        _ => None,
    }
}

/// 查找 Pre 的首个独占行标记，拆分前后正文。 / Find the first standalone Pre marker and split text around it.
/// 简单任务的 PIGEND 是分析与执行分界，后面允许业务正文。 / On simple tasks, PIGEND separates analysis from execution; business text may follow.
/// 复杂任务要求 PIGNEXT 是末行，由编排器检查。 / The orchestrator enforces a final-line PIGNEXT for complex tasks.
/// 标记之前必须有实质分析。 / Substantive analysis must precede the marker.
pub fn split_pre_decision(text: &str) -> Option<(Marker, String, String)> {
    let mut analysis = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(marker) = control_marker(line) {
            if analysis.iter().all(|s: &&str| s.trim().is_empty()) {
                return None;
            }
            let before = analysis.join("\n").trim().to_string();
            let after = lines.collect::<Vec<_>>().join("\n").trim().to_string();
            return Some((marker, before, after));
        }
        analysis.push(line);
    }
    None
}

/// Pre 严格检测：最后非空行必须是标记，前面必须有实质正文。 / Strict Pre detection requires a final nonempty marker line and earlier substantive text.
/// 拒绝只含标记的输出。 / Reject marker-only output.
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

/// Post 终止检测：只检查最后非空行是否为标记。 / Post terminal detection checks only the last nonempty line.
/// Post 已继承 Exec 结果，因此单独一行的控制标记也合法。 / Post inherits the Exec result, so a standalone marker is valid.
pub fn detect_terminal_marker(text: &str) -> Option<Marker> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(control_marker)
}

/// 删除所有控制标记行。 / Remove all control-marker lines.
pub fn strip_markers(text: &str) -> String {
    text.lines()
        .filter(|line| control_marker(line).is_none())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// 判断去除空白后是否为控制标记行。 / Test whether a trimmed line is a control marker.
pub fn is_control_marker_line(line: &str) -> bool {
    control_marker(line).is_some()
}

/// 判断文本是否为控制标记前缀。 / Test whether the text is a prefix of any control marker.
fn is_possible_marker_prefix(text: &str) -> bool {
    !text.is_empty()
        && (PIGEND.starts_with(text) || PIGNEXT.starts_with(text) || PIGFAIL.starts_with(text))
}

/// 删除标记行但保留其它文本的换行布局。 / Remove marker lines while preserving other line breaks.
pub fn strip_control_lines_preserving_layout(text: &str) -> String {
    text.split('\n')
        .filter(|line| !is_control_marker_line(line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 流式过滤器：接收增量并返回可立即转发的文本。 / Streaming filter: consume deltas and return immediately forwardable text.
///
/// 标记可能跨增量，因此暂缓转发以下两种尾部。 / Markers may span deltas, so hold back the following two suffix types.
/// 1. 可能构成标记的末行及前面的换行。 / 1. A potential marker tail and its preceding newline.
///    丢弃标记时同时丢弃分隔换行。 /    Discard the separator newline along with the marker.
/// 2. 尚无下行内容的末尾换行。 / 2. A trailing newline without subsequent content.
///
/// 其余文本立即流式放行，无需攒满一行。 / Forward other text immediately without waiting for a full line.
/// 安全性：所有可能成为标记的后缀都会暂缓发送。 / Safety: hold any suffix that could still form a marker.
#[derive(Debug, Default)]
pub struct MarkerFilter {
    /// 尚待决定是否转发的缓冲文本。 / Buffered text awaiting a forwarding decision.
    pending: String,
}

impl MarkerFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理原始增量并返回可见文本（可能为空）。 / Process a raw delta and return visible text (possibly empty).
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let cut = safe_forward_end(&self.pending);
        if cut == 0 {
            return String::new();
        }
        let prefix: String = self.pending.drain(..cut).collect();
        strip_control_lines_preserving_layout(&prefix)
    }

    /// 流结束时清理剩余缓冲。 / Flush the remaining buffer when the stream ends.
    ///
    /// 剩余的部分标记或分隔换行不得泄漏给客户端。 / Do not leak partial markers or reserved newlines.
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        let visible = strip_control_lines_preserving_layout(&rest);
        let trimmed = visible.trim();
        if trimmed.is_empty() || is_possible_marker_prefix(trimmed) {
            return String::new();
        }
        visible
    }
}

/// 返回可以安全转发的字节末端位置。 / Return the byte boundary up to which text is safe to forward.
fn safe_forward_end(pending: &str) -> usize {
    let line_start = pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let tail = pending[line_start..].trim();
    if is_possible_marker_prefix(tail) {
        // 暂存末行和前置换行。 / Hold the last line and its preceding newline.
        return pending[..line_start].rfind('\n').unwrap_or(0);
    }
    if pending.ends_with('\n') {
        // 暂存末尾换行直到下一行出现。 / Hold a trailing newline until the next line starts.
        return pending.len() - 1;
    }
    pending.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn splits_pre_analysis_from_simple_execution() {
        assert_eq!(
            split_pre_decision("五问分析\r\nPIGEND\r\n实际回答\n第二行"),
            Some((Marker::End, "五问分析".into(), "实际回答\n第二行".into()))
        );
        assert_eq!(
            split_pre_decision("五问分析\nPIGNEXT"),
            Some((Marker::Next, "五问分析".into(), String::new()))
        );
        assert_eq!(split_pre_decision("PIGEND\n实际回答"), None);
        assert_eq!(split_pre_decision("没有标记"), None);
    }

    #[test]
    fn detects_only_on_last_non_empty_line() {
        assert_eq!(detect_marker("分析……\nPIGEND\n"), Some(Marker::End));
        assert_eq!(detect_marker("x\nPIGNEXT"), Some(Marker::Next));
        assert_eq!(detect_marker("x\nPIGFAIL"), Some(Marker::Failed));
        assert_eq!(detect_marker("PIGEND"), None); // 无实质正文 / No substantive text
        assert_eq!(detect_marker("PIGFAIL\n理由\nPIGEND"), Some(Marker::End));
        assert_eq!(detect_marker("普通回答，无标记"), None);
    }

    #[test]
    fn detects_terminal_marker_without_substantive_text() {
        assert_eq!(detect_terminal_marker("PIGEND"), Some(Marker::End));
        assert_eq!(detect_terminal_marker("PIGNEXT"), Some(Marker::Next));
        assert_eq!(detect_terminal_marker("PIGFAIL"), Some(Marker::Failed));
        assert_eq!(
            detect_terminal_marker("核验过程\nPIGEND\n"),
            Some(Marker::End)
        );
        assert_eq!(detect_terminal_marker("普通回答，无标记"), None);
    }

    #[test]
    fn strips_marker_lines() {
        assert_eq!(strip_markers("报告内容\nPIGEND"), "报告内容");
        assert_eq!(strip_markers("a\nPIGNEXT\nb\n"), "a\nb");
        assert_eq!(strip_markers("a\nPIGFAIL\nb\n"), "a\nb");
        assert_eq!(strip_markers("干净文本"), "干净文本");
    }

    /// 任意分块过滤后的拼接结果应等于去除标记的原文。 / Arbitrarily chunked filtering must match the marker-stripped original.
    fn filtered(raw: &str, step: usize) -> String {
        let mut filter = MarkerFilter::new();
        let mut out = String::new();
        let chars: Vec<char> = raw.chars().collect();
        for chunk in chars.chunks(step.max(1)) {
            out.push_str(&filter.push(&chunk.iter().collect::<String>()));
        }
        out.push_str(&filter.finish());
        out
    }

    #[test]
    fn filter_never_leaks_markers_and_preserves_layout() {
        let cases = [
            ("第一行\nPIGEND", "第一行"),
            ("第一行\n第二行\nPIGNEXT", "第一行\n第二行"),
            ("第一行\n第二行\nPIGFAIL", "第一行\n第二行"),
            ("分析\n\n结论\nPIGEND\n", "分析\n\n结论"),
            ("没有标记的普通回答", "没有标记的普通回答"),
            // 标记必须独占整行。 / Markers must occupy an entire line.
            ("说明 PIGEND 的用法\n正文", "说明 PIGEND 的用法\n正文"),
            ("PIGEND 不是最后一行\n结尾", "PIGEND 不是最后一行\n结尾"),
            // 保留以 PIG 开头但不是标记的正文。 / Preserve ordinary words beginning with PIG.
            ("答案\nPIGX 是别的词", "答案\nPIGX 是别的词"),
        ];
        for (raw, want) in cases {
            for step in [1, 2, 3, 7, 100] {
                assert_eq!(filtered(raw, step), want, "raw={raw:?} step={step}");
            }
        }
    }

    #[test]
    fn filter_detects_marker_split_across_chunks() {
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("答案完成"), "答案完成");
        assert_eq!(filter.push("\nPIG"), ""); // 暂存可能的标记前缀 / Hold a possible marker prefix
        assert_eq!(filter.push("E"), "");
        assert_eq!(filter.push("ND\n"), ""); // 丢弃整行标记 / Discard the full marker line
        assert_eq!(filter.finish(), "");
    }

    /// 普通文本立即逐字放行。 / Forward ordinary text immediately, character by character.
    #[test]
    fn filter_forwards_plain_text_immediately() {
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("答"), "答");
        assert_eq!(filter.push("案是 4"), "案是 4");
        assert_eq!(filter.finish(), "");

        // 下一行出现后放行暂存换行。 / Release the held newline once the next line starts.
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("第一行\n"), "第一行");
        assert_eq!(filter.push("第二"), "\n第二");
        assert_eq!(filter.finish(), "");
    }

    /// 被暂存的非标记尾部不会丢失换行。 / Preserve newlines when a held suffix is not a marker.
    #[test]
    fn filter_restores_newline_when_supposed_marker_was_not_one() {
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("答案\nPIG"), "答案");
        assert_eq!(filter.push("X 结尾"), "\nPIGX 结尾");
        assert_eq!(filter.finish(), "");
    }
}
