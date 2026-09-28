//! Tool call display — renders tool call arguments and results.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Format a tool call for display in the chat history.
pub fn format_tool_call<'a>(name: &'a str, args: &'a str) -> Vec<Span<'a>> {
    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("[{name}]"),
            Style::default()
                .fg(Color::White)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if !args.is_empty() {
        // Try to pretty-print JSON args
        let pretty = if let Ok(json) = serde_json::from_str::<serde_json::Value>(args) {
            serde_json::to_string_pretty(&json).unwrap_or_else(|_| args.to_string())
        } else {
            args.to_string()
        };
        spans.push(Span::styled(pretty, Style::default().fg(Color::Gray)));
    }
    spans
}

/// Format a tool result for display.
pub fn format_tool_result<'a>(result: &'a str, is_error: bool) -> Vec<Span<'a>> {
    let color = if is_error { Color::Red } else { Color::Gray };
    let prefix = if is_error { "Error: " } else { "" };
    vec![
        Span::raw("   "),
        Span::styled(format!("{prefix}{result}"), Style::default().fg(color)),
    ]
}

/// Truncate a string for display, showing first and last few lines.
pub fn truncate_output(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= max_lines {
        return output.to_string();
    }
    let half = max_lines / 2;
    let mut result = lines[..half].join("\n");
    result.push_str(&format!("\n  ... ({} lines omitted) ...\n", lines.len() - max_lines));
    result.push_str(&lines[lines.len() - half..].join("\n"));
    result
}
