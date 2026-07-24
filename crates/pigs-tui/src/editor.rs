//! Editor component — multi-line input with Emacs-style and Vim-style keybindings.
//!
//! Uses the `tui-textarea` crate for a full-featured textarea widget.
//! Supports both Emacs mode (default) and Vim mode (toggle with Escape in
//! normal mode or via a vim_mode flag).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use tui_textarea::TextArea;

/// Vim mode state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Insert mode — all keys go directly to the textarea (Emacs-style).
    #[default]
    Insert,
    /// Normal mode — vim-style navigation (hjkl, w, b, dd, etc.).
    Normal,
}

pub struct EditorState {
    pub textarea: TextArea<'static>,
    pub is_bash_mode: bool,
    /// Current vim mode (Insert or Normal). Insert is the default.
    pub vim_mode: VimMode,
    /// Buffer for multi-key vim commands (e.g., 'dd' to delete line).
    pub vim_pending: String,
    /// Whether vim mode is enabled at all (toggled via config or Ctrl+V).
    pub vim_enabled: bool,
}

impl EditorState {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" pig > "),
        );
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        textarea.set_style(Style::default());
        textarea.set_line_number_style(Style::default().fg(Color::DarkGray));

        Self {
            textarea,
            is_bash_mode: false,
            vim_mode: VimMode::Insert,
            vim_pending: String::new(),
            vim_enabled: false,
        }
    }

    /// Toggle vim mode on/off.
    pub fn toggle_vim(&mut self) {
        self.vim_enabled = !self.vim_enabled;
        if !self.vim_enabled {
            self.vim_mode = VimMode::Insert;
            self.vim_pending.clear();
        }
    }

    /// Check if the current input starts with `/` (slash command).
    pub fn is_slash_command(&self) -> bool {
        self.textarea.lines().first().map_or(false, |l| l.starts_with('/'))
    }

    /// Check if the current input starts with `!` (bash mode).
    pub fn is_bash(&self) -> bool {
        self.textarea.lines().first().map_or(false, |l| l.starts_with('!'))
    }

    /// Get the full input text.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Clear the editor.
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        let mut ta = TextArea::default();
        ta.set_block(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" pig > "),
        );
        ta.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        self.textarea = ta;
        self.is_bash_mode = false;
        self.vim_mode = VimMode::Insert;
        self.vim_pending.clear();
    }

    /// Submit the current input and return it. Clears the editor.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        self.clear();
        text
    }

    /// Handle a key event, routing to vim normal mode or inserting directly.
    /// Returns true if the key was consumed (vim mode handled it).
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Ctrl+V toggles vim mode
        if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.toggle_vim();
            return true;
        }

        // If vim is not enabled, all keys go to textarea (Emacs mode)
        if !self.vim_enabled {
            return false;
        }

        match self.vim_mode {
            VimMode::Insert => {
                // Escape switches to Normal mode
                if key.code == KeyCode::Esc {
                    self.vim_mode = VimMode::Normal;
                    self.vim_pending.clear();
                    return true;
                }
                // All other keys go to textarea
                false
            }
            VimMode::Normal => {
                self.handle_vim_normal(key);
                true // Always consume keys in normal mode
            }
        }
    }

    /// Handle vim normal mode keybindings.
    fn handle_vim_normal(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        use tui_textarea::CursorMove;

        // If we have a pending command (e.g., first 'd' of 'dd')
        if !self.vim_pending.is_empty() {
            let pending = self.vim_pending.clone();
            self.vim_pending.clear();

            match (pending.as_str(), key.code) {
                // dd — delete current line content (head to end)
                ("d", KeyCode::Char('d')) => {
                    self.textarea.delete_line_by_head();
                    self.textarea.delete_line_by_end();
                }
                // dw — delete word forward
                ("d", KeyCode::Char('w')) => {
                    self.textarea.delete_next_word();
                }
                // db — delete word backward
                ("d", KeyCode::Char('b')) => {
                    self.textarea.delete_word();
                }
                _ => {
                    // Unknown sequence, ignore
                }
            }
            return;
        }

        match key.code {
            // Switch to insert mode
            KeyCode::Char('i') => {
                self.vim_mode = VimMode::Insert;
            }
            // Insert at beginning of line
            KeyCode::Char('I') => {
                self.textarea.move_cursor(CursorMove::Head);
                self.vim_mode = VimMode::Insert;
            }
            // Insert after cursor
            KeyCode::Char('a') => {
                self.textarea.move_cursor(CursorMove::Forward);
                self.vim_mode = VimMode::Insert;
            }
            // Insert at end of line
            KeyCode::Char('A') => {
                self.textarea.move_cursor(CursorMove::End);
                self.vim_mode = VimMode::Insert;
            }
            // Open new line below
            KeyCode::Char('o') => {
                self.textarea.move_cursor(CursorMove::End);
                self.textarea.insert_newline();
                self.vim_mode = VimMode::Insert;
            }
            // Open new line above
            KeyCode::Char('O') => {
                self.textarea.move_cursor(CursorMove::Head);
                self.textarea.insert_newline();
                self.textarea.move_cursor(CursorMove::Up);
                self.vim_mode = VimMode::Insert;
            }
            // Movement: h/j/k/l
            KeyCode::Char('h') => {
                self.textarea.move_cursor(CursorMove::Back);
            }
            KeyCode::Char('j') => {
                self.textarea.move_cursor(CursorMove::Down);
            }
            KeyCode::Char('k') => {
                self.textarea.move_cursor(CursorMove::Up);
            }
            KeyCode::Char('l') => {
                self.textarea.move_cursor(CursorMove::Forward);
            }
            // Word movement: w/b
            KeyCode::Char('w') => {
                self.textarea.move_cursor(CursorMove::WordForward);
            }
            KeyCode::Char('b') => {
                self.textarea.move_cursor(CursorMove::WordBack);
            }
            // Line movement: 0/$
            KeyCode::Char('0') => {
                self.textarea.move_cursor(CursorMove::Head);
            }
            KeyCode::Char('$') => {
                self.textarea.move_cursor(CursorMove::End);
            }
            // G — go to last line
            KeyCode::Char('G') => {
                self.textarea.move_cursor(CursorMove::Bottom);
            }
            // gg — go to first line (needs pending)
            KeyCode::Char('g') => {
                self.vim_pending = "g".to_string();
            }
            // x — delete char under cursor
            KeyCode::Char('x') => {
                self.textarea.delete_next_char();
            }
            // d — start delete command (needs second key)
            KeyCode::Char('d') => {
                self.vim_pending = "d".to_string();
            }
            // u — undo
            KeyCode::Char('u') => {
                self.textarea.undo();
            }
            // Enter — submit (same as insert mode)
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                // In normal mode, Enter still submits
                // The submit will be handled by the caller
            }
            // Escape — stay in normal mode (no-op)
            KeyCode::Esc => {}
            _ => {}
        }
    }

    /// Update border color and title based on input type and vim mode.
    pub fn update_border(&mut self) {
        let border_color = if self.is_slash_command() {
            Color::Blue
        } else if self.is_bash() {
            Color::Green
        } else {
            Color::Cyan
        };
        let title = if self.is_bash() {
            " bash > "
        } else if self.is_slash_command() {
            " cmd > "
        } else {
            " pig > "
        };

        // Append vim mode indicator if vim is enabled
        let title = if self.vim_enabled {
            match self.vim_mode {
                VimMode::Insert => format!("{title}[INSERT] "),
                VimMode::Normal => format!("{title}[NORMAL] "),
            }
        } else {
            title.to_string()
        };

        self.textarea.set_block(
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title),
        );
    }

    /// Insert text (for paste).
    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.textarea.insert_char(ch);
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        self.update_border();
        use ratatui::widgets::Widget;
        self.textarea.render(area, buf);
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}
