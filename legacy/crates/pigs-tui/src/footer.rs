//! Footer / status bar — pwd, git branch, token stats, model name.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub struct FooterState {
    pub cwd: String,
    pub git_branch: Option<String>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_pct: f64,
    pub is_working: bool,
}

impl FooterState {
    pub fn new(cwd: &str, model: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            git_branch: detect_git_branch(cwd),
            model: model.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            context_pct: 0.0,
            is_working: false,
        }
    }

    pub fn render_widget(&self, area: Rect, buf: &mut Buffer) {
        let cwd_display = truncate_path(&self.cwd, 40);

        let mut line1_spans = vec![
            Span::raw(" "),
            Span::styled(cwd_display, Style::default().fg(Color::DarkGray)),
        ];

        if let Some(branch) = &self.git_branch {
            line1_spans.push(Span::styled(
                format!(" ({branch})"),
                Style::default().fg(Color::Magenta),
            ));
        }

        let context_color = if self.context_pct > 90.0 {
            Color::Red
        } else if self.context_pct > 70.0 {
            Color::Yellow
        } else {
            Color::Green
        };

        let line2 = Line::from(vec![
            Span::raw(" "),
            Span::styled(format!("^{}", self.input_tokens), Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled(format!("v{}", self.output_tokens), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(
                format!("{:.0}%", self.context_pct),
                Style::default().fg(context_color),
            ),
            Span::raw("  "),
            Span::styled(
                self.model.clone(),
                Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            ),
        ]);

        Paragraph::new(vec![line1_spans.into(), line2]).render(area, buf);
    }
}

fn detect_git_branch(cwd: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() && branch != "HEAD" {
            return Some(branch);
        }
    }
    None
}

fn truncate_path(path: &str, max_len: usize) -> String {
    let path = path.replace('\\', "/");
    if path.len() <= max_len {
        return path;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        return path;
    }
    format!(".../{}", parts.last().unwrap_or(&""))
}
