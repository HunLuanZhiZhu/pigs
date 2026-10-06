//! 控制面标记：PIGEND / PIGNEXT / PIGFAIL。相位结束条件是"模型答完"；标记仅用于路由。

/// 成功结束标记（整轮完成）。
pub const PIGEND: &str = "PIGEND";
/// 可修补标记（Post 验收后回到 Executor 继续执行）。
pub const PIGNEXT: &str = "PIGNEXT";
/// 失败路径标记（回到 Pre 重规划）。
pub const PIGFAIL: &str = "PIGFAIL";

/// 标记类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// PIGEND —— 整轮正常结束。
    End,
    /// PIGNEXT —— 当前结果可继续修补，回 Executor。
    Next,
    /// PIGFAIL —— 路径失败，需要重规划。
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

/// Pre 使用的严格检测：仅当**最后一个非空行**是控制标记时才算检测到，且要求前面
/// 存在非标记的实质行，防止简单路径只输出一个光秃秃的标记。
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

/// Post 使用的终止检测：只要求最后一个非空行是控制标记。
/// Post 的任务结果已经存在于 Executor 上下文中，因此 `PIGEND` / `PIGNEXT` / `PIGFAIL` 单独一行也是合法路由信号。
pub fn detect_terminal_marker(text: &str) -> Option<Marker> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(control_marker)
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

/// 这段文本是否"还有可能长成控制标记"（即它是某个标记的开头）。
fn is_possible_marker_prefix(text: &str) -> bool {
    !text.is_empty()
        && (PIGEND.starts_with(text) || PIGNEXT.starts_with(text) || PIGFAIL.starts_with(text))
}

/// 删除整行控制标记，但保留其它行与换行布局（用于流式增量转发）。
pub fn strip_control_lines_preserving_layout(text: &str) -> String {
    text.split('\n')
        .filter(|line| !is_control_marker_line(line.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 流式增量过滤器：把上游增量按块喂进来，吐出可以立刻转发给客户端的可见文本。
///
/// 控制标记可能跨增量到达（"PIG" + "END"），所以**只压住两种尾巴**：
/// 1. 可能正在长成标记的最后一行（`PIG` → `PIGEND` / `PIGNEXT`），连它前面那个换行一起留着——
///    标记被丢弃时，这个换行也跟着丢掉，不会在答复里留下多余空行；
/// 2. 悬在末尾、还没有下一行内容的换行（等下一行有实质内容了再放行）。
///
/// 其余文本**一拿到就放行**（逐字流式，而不是攒够一整行才动）。
/// 安全性：凡是还能拼成控制标记的部分都被压住了，已经发出去的字符不可能事后变成标记。
#[derive(Debug, Default)]
pub struct MarkerFilter {
    /// 尚未决定是否转发的待处理文本。
    pending: String,
}

impl MarkerFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段（含标记的）原始增量，返回可以转发给客户端的可见文本（可能为空串）。
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let cut = safe_forward_end(&self.pending);
        if cut == 0 {
            return String::new();
        }
        let prefix: String = self.pending.drain(..cut).collect();
        strip_control_lines_preserving_layout(&prefix)
    }

    /// 流结束：处理缓冲里剩下的一点尾巴。
    ///
    /// 此时剩余内容只可能是"被截断的标记"或"为标记预留的分隔换行"——都不该发给客户端。
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

/// 返回"可以安全转发"的字节边界（此边界之前的部分可以立刻发出去）。
fn safe_forward_end(pending: &str) -> usize {
    let line_start = pending.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let tail = pending[line_start..].trim();
    if is_possible_marker_prefix(tail) {
        // 保留最后一行（含它前面的那个换行）：cut 落在上一个换行之前
        return pending[..line_start].rfind('\n').unwrap_or(0);
    }
    if pending.ends_with('\n') {
        // 末尾悬着的换行先留着，等下一行有内容再一起放行
        return pending.len() - 1;
    }
    pending.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn detects_only_on_last_non_empty_line() {
        assert_eq!(detect_marker("分析……\nPIGEND\n"), Some(Marker::End));
        assert_eq!(detect_marker("x\nPIGNEXT"), Some(Marker::Next));
        assert_eq!(detect_marker("x\nPIGFAIL"), Some(Marker::Failed));
        assert_eq!(detect_marker("PIGEND"), None); // 无实质行
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

    /// 把一整段原始文本按任意粒度切片喂进过滤器，拼接结果必须等于"去标记后的文本"。
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
            // 标记词出现在句中/行首但非独占一行都不算标记
            ("说明 PIGEND 的用法\n正文", "说明 PIGEND 的用法\n正文"),
            ("PIGEND 不是最后一行\n结尾", "PIGEND 不是最后一行\n结尾"),
            // 普通正文里以 PIG 开头但不成标记的词
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
        assert_eq!(filter.push("\nPIG"), ""); // 可能是标记开头 → 压住
        assert_eq!(filter.push("E"), "");
        assert_eq!(filter.push("ND\n"), ""); // 整行是标记 → 丢掉
        assert_eq!(filter.finish(), "");
    }

    /// 逐字放行：普通文本一拿到就出去，不必等一整行写完。
    #[test]
    fn filter_forwards_plain_text_immediately() {
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("答"), "答");
        assert_eq!(filter.push("案是 4"), "案是 4");
        assert_eq!(filter.finish(), "");

        // 末尾换行也很快放行（下一行有内容时）
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("第一行\n"), "第一行");
        assert_eq!(filter.push("第二"), "\n第二");
        assert_eq!(filter.finish(), "");
    }

    /// 被压住的尾巴若不是标记（"PIGX"），换行不会被吞掉。
    #[test]
    fn filter_restores_newline_when_supposed_marker_was_not_one() {
        let mut filter = MarkerFilter::new();
        assert_eq!(filter.push("答案\nPIG"), "答案");
        assert_eq!(filter.push("X 结尾"), "\nPIGX 结尾");
        assert_eq!(filter.finish(), "");
    }
}
