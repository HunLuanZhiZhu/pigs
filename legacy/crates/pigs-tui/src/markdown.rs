//! Markdown rendering — converts markdown text to ratatui Lines.
//!
//! Uses pulldown-cmark for parsing, then produces styled ratatui Lines
//! with headings, code blocks, lists, inline code, and bold/italic.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown text into ratatui Lines for the given terminal width.
pub fn render_to_lines(markdown: &str, width: usize) -> Vec<Line<'_>> {
    let parser = pulldown_cmark::Parser::new(markdown);
    let mut lines: Vec<Line> = Vec::new();
    let mut current_spans: Vec<Span> = Vec::new();
    let mut in_code_block = false;
    let mut code_block_lines: Vec<String> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut indent = String::new();

    for event in parser {
        match event {
            pulldown_cmark::Event::Start(tag) => {
                match tag {
                    pulldown_cmark::Tag::Heading { level, .. } => {
                        style_stack.push(Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD));
                        indent = "#".repeat(level as usize) + " ";
                    }
                    pulldown_cmark::Tag::CodeBlock(_) => {
                        in_code_block = true;
                        code_block_lines.clear();
                    }
                    pulldown_cmark::Tag::Emphasis => {
                        let s = *style_stack.last().unwrap_or(&Style::default());
                        style_stack.push(s.add_modifier(Modifier::ITALIC));
                    }
                    pulldown_cmark::Tag::Strong => {
                        let s = *style_stack.last().unwrap_or(&Style::default());
                        style_stack.push(s.add_modifier(Modifier::BOLD));
                    }
                    pulldown_cmark::Tag::List(_) => {}
                    pulldown_cmark::Tag::Item => {
                        indent = "  • ".to_string();
                    }
                    pulldown_cmark::Tag::BlockQuote(_) => {
                        indent = "  │ ".to_string();
                        let s = *style_stack.last().unwrap_or(&Style::default());
                        style_stack.push(s.fg(Color::Yellow));
                    }
                    _ => {}
                }
            }
            pulldown_cmark::Event::End(tag_end) => {
                match tag_end {
                    pulldown_cmark::TagEnd::Heading(_) => {
                        if !current_spans.is_empty() {
                            let s = *style_stack.last().unwrap_or(&Style::default());
                            let prefix = Span::styled(indent.clone(), s);
                            let mut spans = vec![prefix];
                            spans.append(&mut current_spans.clone());
                            lines.push(Line::from(spans));
                            current_spans.clear();
                        }
                        style_stack.pop();
                        indent.clear();
                    }
                    pulldown_cmark::TagEnd::CodeBlock => {
                        in_code_block = false;
                        let block_style = Style::default().fg(Color::Green);
                        for cl in &code_block_lines {
                            let wrapped = textwrap::wrap(
                                cl,
                                width.saturating_sub(4),
                            );
                            for w in wrapped {
                                lines.push(Line::from(vec![
                                    Span::raw("  "),
                                    Span::styled(w.into_owned(), block_style),
                                ]));
                            }
                        }
                        lines.push(Line::raw(""));
                        code_block_lines.clear();
                    }
                    pulldown_cmark::TagEnd::Paragraph => {
                        if !current_spans.is_empty() {
                            let s = *style_stack.last().unwrap_or(&Style::default());
                            let combined: String = current_spans.iter()
                                .map(|sp| sp.content.to_string())
                                .collect::<Vec<_>>()
                                .join("");
                            let wrapped = textwrap::wrap(&combined, width.saturating_sub(indent.len()));
                            for w in wrapped {
                                lines.push(Line::from(vec![
                                    Span::styled(indent.clone(), s),
                                    Span::styled(w.into_owned(), s),
                                ]));
                            }
                            current_spans.clear();
                        }
                        indent.clear();
                    }
                    pulldown_cmark::TagEnd::Emphasis => {
                        style_stack.pop();
                    }
                    pulldown_cmark::TagEnd::Strong => {
                        style_stack.pop();
                    }
                    pulldown_cmark::TagEnd::Item => {
                        indent.clear();
                    }
                    pulldown_cmark::TagEnd::BlockQuote(_) => {
                        indent.clear();
                        style_stack.pop();
                    }
                    _ => {}
                }
            }
            pulldown_cmark::Event::Text(text) => {
                if in_code_block {
                    for line in text.lines() {
                        code_block_lines.push(line.to_string());
                    }
                } else {
                    let s = *style_stack.last().unwrap_or(&Style::default());
                    current_spans.push(Span::styled(text.into_string(), s));
                }
            }
            pulldown_cmark::Event::Code(code) => {
                current_spans.push(Span::styled(
                    code.into_string(),
                    Style::default().fg(Color::Red),
                ));
            }
            pulldown_cmark::Event::SoftBreak | pulldown_cmark::Event::HardBreak => {
                if !current_spans.is_empty() {
                    let s = *style_stack.last().unwrap_or(&Style::default());
                    let combined: String = current_spans.iter()
                        .map(|sp| sp.content.to_string())
                        .collect::<Vec<_>>()
                        .join("");
                    let wrapped = textwrap::wrap(&combined, width.saturating_sub(indent.len()));
                    for w in wrapped {
                        lines.push(Line::from(vec![
                            Span::styled(indent.clone(), s),
                            Span::styled(w.into_owned(), s),
                        ]));
                    }
                    current_spans.clear();
                } else {
                    lines.push(Line::raw(""));
                }
            }
            pulldown_cmark::Event::Rule => {
                let w = width.saturating_sub(2);
                lines.push(Line::from(vec![
                    Span::styled("─".repeat(w), Style::default().fg(Color::DarkGray)),
                ]));
            }
            _ => {}
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    lines
}
