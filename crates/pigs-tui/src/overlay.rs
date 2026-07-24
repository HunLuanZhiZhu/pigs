//! Overlay components — model selector, session selector, etc.
//!
//! These are popup-style overlays rendered on top of the main TUI layout,
//! similar to PI's model selector and session picker.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Widget};

/// Which overlay is currently active (if any).
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayKind {
    /// Model selector popup (Ctrl+L).
    ModelSelector,
    /// Session selector popup (Ctrl+S or /sessions).
    SessionSelector,
}

/// State for overlay components.
pub struct OverlayState {
    pub kind: OverlayKind,
    pub items: Vec<String>,
    pub state: ListState,
    pub title: String,
}

impl OverlayState {
    /// Create a model selector overlay.
    pub fn model_selector(models: &[String], current: &str) -> Self {
        let mut state = ListState::default();
        // Find current model index
        let selected = models.iter().position(|m| m == current).unwrap_or(0);
        state.select(Some(selected));

        Self {
            kind: OverlayKind::ModelSelector,
            items: models.to_vec(),
            state,
            title: " Select Model ".to_string(),
        }
    }

    /// Create a session selector overlay.
    pub fn session_selector(sessions: &[(String, String)], current_id: &str) -> Self {
        let items: Vec<String> = sessions
            .iter()
            .map(|(id, title)| {
                let short_id = &id[..8.min(id.len())];
                let marker = if id == current_id { " ←" } else { "" };
                format!("{} | {}{}", short_id, title, marker)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(0));

        Self {
            kind: OverlayKind::SessionSelector,
            items,
            state,
            title: " Select Session ".to_string(),
        }
    }

    /// Move selection up.
    pub fn select_up(&mut self) {
        let i = self.state.selected().unwrap_or(0);
        let new_i = if i == 0 {
            self.items.len().saturating_sub(1)
        } else {
            i - 1
        };
        self.state.select(Some(new_i));
    }

    /// Move selection down.
    pub fn select_down(&mut self) {
        let i = self.state.selected().unwrap_or(0);
        let new_i = if i + 1 >= self.items.len() {
            0
        } else {
            i + 1
        };
        self.state.select(Some(new_i));
    }

    /// Get the selected item text.
    pub fn selected(&self) -> Option<&str> {
        self.state
            .selected()
            .and_then(|i| self.items.get(i).map(|s| s.as_str()))
    }

    /// Get the selected index.
    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    /// Render the overlay popup in the center of the screen.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        // Calculate popup size: 60% width, up to 20 lines height
        let popup_width = (area.width as f64 * 0.6) as u16;
        let popup_height = self.items.len().min(15) as u16 + 3; // +3 for border + title
        let popup_height = popup_height.min(area.height.saturating_sub(4));

        // Center the popup
        let popup_area = Rect::new(
            area.x + (area.width.saturating_sub(popup_width)) / 2,
            area.y + (area.height.saturating_sub(popup_height)) / 2,
            popup_width,
            popup_height,
        );

        // Clear the background
        Clear.render(popup_area, buf);

        // Build the list items
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| ListItem::new(Line::from(vec![Span::raw(format!("  {item}"))])))
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(self.title.clone())
                    .title_alignment(Alignment::Center),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        // Render the list (mutable borrow for state)
        let mut state = self.state.clone();
        ratatui::widgets::StatefulWidget::render(list, popup_area, buf, &mut state);
        self.state = state;
    }
}
