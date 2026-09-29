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

/// 该行（去掉首尾空白后）是否就是一个控制标记。
pub fn is_control_marker_line(line: &str) -> bool {
    control_marker(line).is_some()
}

/// 删除整行控制标记，但保留其它行与换行布局（用于流式增量转发）。
pub fn strip_control_lines_preserving_layout(text: &str) -> String {
    text.split('\n')
        .filter(|line| !is_control_marker_line(line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 流式增量过滤器：把上游增量按块喂进来，吐出"确认安全"的可见文本。
///
/// 控制标记可能跨多个增量到达（"PIG" + "END"），所以最后一行永远留在缓冲里，
/// 直到看到换行或流结束才敢断定它不是标记；其余已确认的行立即转发。
/// 移植自 legacy `http_runtime.rs` 的 `MarkerLineBuffer`。
#[derive(Debug, Default)]
pub struct MarkerFilter {
    /// 尚未决定是否转发的待处理文本。
    pending: String,
    /// 是否已经发出过至少一段增量。
    emitted: bool,
}

impl MarkerFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段（含标记的）原始增量，返回可以转发给客户端的可见文本（可能为空串）。
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let Some(last_start) = last_nonempty_line_start(&self.pending) else {
            return String::new();
        };
        // 只转发到"最后一个非空行"之前的内容，该行留着继续观察
        let keep_from = self.pending[..last_start].rfind('\n').unwrap_or(last_start);
        if keep_from == 0 {
            return String::new();
        }
        let prefix: String = self.pending.drain(..keep_from).collect();
        let visible = strip_control_lines_preserving_layout(&prefix);
        if visible.trim().is_empty() {
            return String::new();
        }
        self.emitted = true;
        visible
    }

    /// 流结束：把缓冲里剩下的文本去标记后一次吐出。
    pub fn finish(&mut self) -> String {
        let visible = strip_markers(&self.pending);
        let out = if visible.is_empty() {
            String::new()
        } else if self.emitted && self.pending.starts_with('\n') {
            // 之前发过增量且剩余以换行开头 → 补一个前导换行保持排版
            format!("\n{visible}")
        } else {
            visible
        };
        self.pending.clear();
        out
    }
}

/// 返回最后一个非空行的起始字节偏移（决定保留多少待处理文本）。
fn last_nonempty_line_start(text: &str) -> Option<usize> {
    let mut offset = 0usize;
    let mut last = None;
    for segment in text.split_inclusive('\n') {
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
        offset += segment.len();
    }
    // 末尾没有换行的最后一段
    if offset < text.len() {
        let segment = &text[offset..];
        if !segment.trim().is_empty() {
            last = Some(offset);
        }
    }
    last
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

    /// 把一整段原始文本按任意粒度切片喂进过滤器，拼接结果必须等于"去标记后的文本"。
    fn filtered(raw: &str, step: usize) -> String {
        let mut filter = MarkerFilter::new();
        let mut out = String::new();
        for chunk in raw.split_inclusive(|_| true).collect::<Vec<_>>().chunks(step.max(1)) {
            let piece: String = chunk.concat();
            out.push_str(&filter.push(&piece));
        }
        out.push_str(&filter.finish());
        out
    }

    #[test]
    fn filter_never_leaks_markers_and_preserves_layout() {
        let cases = [
            ("第一行\nPIGEND", "第一行"),
            ("第一行\n第二行\nPIGFAIL", "第一行\n第二行"),
            ("分析\n\n结论\nPIGEND\n", "分析\n\n结论"),
            ("没有标记的普通回答", "没有标记的普通回答"),
            // 标记词出现在句中/行尾之外都不算标记
            ("说明 PIGEND 的用法\n正文", "说明 PIGEND 的用法\n正文"),
            ("PIGEND 不是最后一行\n结尾", "PIGEND 不是最后一行\n结尾"),
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
        // 换行会作为"下一行的前导"在后续增量里发出，所以这里只拿到已确认的一行
        assert_eq!(filter.push("答案完成\nPIG"), "答案完成");
        assert_eq!(filter.push("E"), "");
        assert_eq!(filter.push("ND\n"), ""); // 整行是标记 → 丢掉
        assert_eq!(filter.finish(), "");
    }
}
