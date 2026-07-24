//! Header bar — version, model, keybinding hints.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub struct HeaderState {
    pub version: String,
    pub model: String,
    pub language: String,
}

impl HeaderState {
    pub fn new(model: &str, language: &str) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            model: model.to_string(),
            language: language.to_string(),
        }
    }

    pub fn render_widget(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let line = Line::from(vec![
            Span::styled("  pig ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(format!("v{} ", self.version), Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(format!("| {} ", self.model), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(format!("| {} ", self.language), Style::default().fg(Color::Blue)),
            Span::raw(" "),
            Span::styled("/help  !bash  Ctrl+D quit", Style::default().fg(Color::DarkGray)),
        ]);
        Paragraph::new(line).render(area, buf);
    }
}
