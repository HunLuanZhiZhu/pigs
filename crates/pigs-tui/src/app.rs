//! Main TUI application — the central controller.
//!
//! Layout (top to bottom):
//!   Header      — version, model, keybinding hints
//!   Chat        — message history (scrollable)
//!   Status      — "Working..." spinner or empty
//!   Editor      — multi-line input
//!   Footer      — pwd, git branch, token stats, model

use std::io::Stdout;
use std::time::{Duration, Instant};

use crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use crate::chat::{ChatEntry, ChatState};
use crate::editor::EditorState;
use crate::footer::FooterState;
use crate::header::HeaderState;

/// The main TUI application state.
pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    chat: ChatState,
    editor: EditorState,
    header: HeaderState,
    footer: FooterState,
    is_working: bool,
    should_quit: bool,
    spinner_frame: usize,
    last_input_time: Option<Instant>,
    /// Text submitted via Enter, waiting to be picked up by the caller.
    pub pending_input: Option<String>,
    /// Active overlay (model selector, session selector, etc.)
    pub overlay: Option<crate::overlay::OverlayState>,
    /// Available models for the selector
    pub models: Vec<String>,
    /// Pending model selection (set when user picks from overlay)
    pub pending_model: Option<String>,
    /// Current color theme
    pub theme: crate::theme::Theme,
    /// Number of active sub-agents (for Team Mode status display).
    pub sub_agent_count: usize,
    /// Number of completed sub-agents.
    pub sub_agent_done: usize,
    /// Proxy log buffer (capped at 1000 lines).
    pub proxy_log: std::collections::VecDeque<String>,
    /// Whether the TUI is currently viewing the proxy log ("api" session).
    pub viewing_proxy: bool,
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl App {
    /// Initialize the terminal in raw mode + alternate screen.
    pub fn init(model: &str, language: &str, cwd: &str) -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        Ok(Self {
            terminal,
            chat: ChatState::new(),
            editor: EditorState::new(),
            header: HeaderState::new(model, language),
            footer: FooterState::new(cwd, model),
            is_working: false,
            should_quit: false,
            spinner_frame: 0,
            last_input_time: None,
            pending_input: None,
            overlay: None,
            models: Vec::new(),
            pending_model: None,
            theme: crate::theme::Theme::dark(),
            sub_agent_count: 0,
            sub_agent_done: 0,
            proxy_log: std::collections::VecDeque::with_capacity(1000),
            viewing_proxy: false,
        })
    }

    /// The main event loop — processes terminal events and renders frames.
    /// Returns when the user quits or an error occurs.
    pub async fn run(
        &mut self,
        event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::event::AppEvent>,
    ) -> anyhow::Result<()> {
        // Initial render
        self.draw()?;

        // Poll for terminal events at 50ms intervals while also checking
        // the event channel for agent/streaming events
        let mut terminal_event_interval = tokio::time::interval(Duration::from_millis(50));
        terminal_event_interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                // Terminal input events (non-blocking poll)
                _ = terminal_event_interval.tick() => {
                    while crossterm::event::poll(Duration::from_millis(0))? {
                        let event = crossterm::event::read()?;
                        self.handle_terminal_event(event);
                        if self.should_quit {
                            self.cleanup()?;
                            return Ok(());
                        }
                    }
                    // Update spinner if working
                    if self.is_working {
                        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
                        self.draw()?;
                    }
                }
                // App events from agent/streaming
                Some(app_event) = event_rx.recv() => {
                    self.handle_app_event(app_event);
                    if self.should_quit {
                        self.cleanup()?;
                        return Ok(());
                    }
                    self.draw()?;
                }
            }
        }
    }

    /// Handle a terminal input event.
    fn handle_terminal_event(&mut self, event: CEvent) {
        match event {
            CEvent::Key(key) => self.handle_key(key),
            CEvent::Resize(_, _) => {
                let _ = self.draw();
            }
            _ => {}
        }
    }

    /// Handle a key press.
    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        // If an overlay is active, route keys to it
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }

        // Ctrl+D to quit (when editor is empty)
        if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.editor.text().trim().is_empty() {
                self.should_quit = true;
                return;
            }
        }

        // Ctrl+C to clear input or quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.editor.text().trim().is_empty() {
                self.should_quit = true;
            } else {
                self.editor.clear();
            }
            return;
        }

        // Ctrl+L to open model selector overlay
        if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if !self.models.is_empty() {
                self.overlay = Some(crate::overlay::OverlayState::model_selector(
                    &self.models,
                    &self.header.model,
                ));
            }
            return;
        }

        // Ctrl+T to cycle theme
        if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.cycle_theme();
            return;
        }

        // Enter to submit (without shift)
        if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT) {
            let text = self.editor.submit();
            if !text.trim().is_empty() {
                self.chat.push(ChatEntry::User(text.clone()));
                self.is_working = true;
                self.last_input_time = Some(Instant::now());
                self.pending_input = Some(text);
            }
            return;
        }

        // Route key to editor (handles vim mode if enabled)
        // Ctrl+V toggles vim mode, other keys go through vim handler
        if !self.editor.handle_key(key) {
            // If vim handler didn't consume the key, pass to textarea directly
            self.editor.textarea.input(KeyEvent::from(key));
        }
    }

    /// Handle key presses when an overlay is active.
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
            }
            KeyCode::Up => {
                if let Some(overlay) = &mut self.overlay {
                    overlay.select_up();
                }
            }
            KeyCode::Down => {
                if let Some(overlay) = &mut self.overlay {
                    overlay.select_down();
                }
            }
            KeyCode::Enter => {
                // Confirm selection
                if let Some(overlay) = &self.overlay {
                    if let Some(selected) = overlay.selected_index() {
                        match overlay.kind {
                            crate::overlay::OverlayKind::ModelSelector => {
                                if let Some(model) = self.models.get(selected) {
                                    self.header.model = model.clone();
                                    self.footer.model = model.clone();
                                    self.pending_model = Some(model.clone());
                                }
                            }
                            crate::overlay::OverlayKind::SessionSelector => {
                                // Session selector is not wired up; no-op.
                            }
                        }
                    }
                }
                self.overlay = None;
            }
            _ => {}
        }
    }

    /// Handle an application event (from agent/streaming).
    fn handle_app_event(&mut self, event: crate::event::AppEvent) {
        use crate::event::AppEvent;
        match event {
            AppEvent::StreamText(text) => {
                // Append to the last assistant message or create a new one
                if let Some(ChatEntry::Assistant { text: ref mut t, .. }) = self.chat.last_focused_entry_mut() {
                    t.push_str(&text);
                } else {
                    self.chat.push(ChatEntry::Assistant { text, thinking: None });
                }
            }
            AppEvent::StreamThinking(text) => {
                if let Some(ChatEntry::Assistant { thinking: ref mut th, .. }) = self.chat.last_focused_entry_mut() {
                    if let Some(thinking) = th {
                        thinking.push_str(&text);
                    } else {
                        *th = Some(text);
                    }
                } else {
                    self.chat.push(ChatEntry::Assistant { text: String::new(), thinking: Some(text) });
                }
            }
            AppEvent::ToolStart { name, args } => {
                self.chat.push(ChatEntry::ToolCall { name, args, result: String::new(), is_error: false });
            }
            AppEvent::ToolEnd { name: _, result, is_error } => {
                if let Some(ChatEntry::ToolCall { result: ref mut r, is_error: ref mut e, .. }) = self.chat.last_focused_entry_mut() {
                    *r = result;
                    *e = is_error;
                }
            }
            AppEvent::TurnFinished => {
                self.is_working = false;
            }
            AppEvent::AgentError(e) => {
                self.chat.push(ChatEntry::System(format!("Error: {e}")));
                self.is_working = false;
            }
            AppEvent::Quit => {
                self.should_quit = true;
            }
            AppEvent::BashCommand(cmd) => {
                self.chat.push(ChatEntry::Bash { command: cmd.clone(), output: String::new(), exit_code: 0 });
            }
            AppEvent::Redraw => {
                let _ = self.draw();
            }
            _ => {}
        }
    }

    /// Draw the entire screen.
    fn draw(&mut self) -> anyhow::Result<()> {
        self.terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),   // Header
                    Constraint::Min(1),       // Chat
                    Constraint::Length(1),   // Status
                    Constraint::Length(3),   // Editor
                    Constraint::Length(2),   // Footer
                ])
                .split(f.area());

            // Header
            self.header.render_widget(chunks[0], f.buffer_mut());

            // Chat or Proxy log (scrollable)
            if self.viewing_proxy {
                // Show proxy log lines
                let log_lines: Vec<ratatui::text::Line> = self.proxy_log
                    .iter()
                    .map(|line| ratatui::text::Line::raw(line.clone()))
                    .collect();
                let log_para = Paragraph::new(log_lines)
                    .wrap(ratatui::widgets::Wrap { trim: false });
                f.render_widget(log_para, chunks[1]);
            } else {
                let chat_lines = self.chat.render_lines(chunks[1].width as usize);
                let chat_para = Paragraph::new(chat_lines)
                    .wrap(ratatui::widgets::Wrap { trim: false });
                f.render_widget(chat_para, chunks[1]);
            }

            // Status
            let status = if self.is_working {
                let spinner = SPINNER_FRAMES[self.spinner_frame];
                let mut spans = vec![
                    Span::raw(" "),
                    Span::styled(spinner, Style::default().fg(Color::Cyan)),
                    Span::styled(" Working...", Style::default().fg(Color::Cyan)),
                ];
                // Show sub-agent count in Team Mode
                if self.sub_agent_count > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("pigs: {}/{} agents", self.sub_agent_done, self.sub_agent_count),
                        Style::default().fg(Color::Magenta),
                    ));
                }
                Line::from(spans)
            } else if self.sub_agent_count > 0 {
                // Show sub-agent status even when not actively working
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        format!("pigs: {}/{} agents", self.sub_agent_done, self.sub_agent_count),
                        Style::default().fg(Color::Magenta),
                    ),
                ])
            } else {
                Line::raw("")
            };
            f.render_widget(Paragraph::new(status), chunks[2]);

            // Editor
            self.editor.render(chunks[3], f.buffer_mut());

            // Footer
            self.footer.render_widget(chunks[4], f.buffer_mut());

            // Overlay (if active)
            if let Some(overlay) = &mut self.overlay {
                overlay.render(f.area(), f.buffer_mut());
            }
        })?;
        Ok(())
    }

    /// Restore terminal to normal mode.
    fn cleanup(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }

    /// Get the pending user input (if submitted via Enter).
    pub fn take_pending_input(&mut self) -> Option<String> {
        self.pending_input.take()
    }

    /// Update footer stats.
    pub fn update_stats(&mut self, input: u64, output: u64, context_pct: f64) {
        self.footer.input_tokens = input;
        self.footer.output_tokens = output;
        self.footer.context_pct = context_pct;
    }

    // Public wrappers for external access (used by tui_repl.rs)

    pub fn handle_terminal_event_public(&mut self, event: CEvent) {
        self.handle_terminal_event(event);
    }

    pub fn handle_app_event_public(&mut self, event: crate::event::AppEvent) {
        self.handle_app_event(event);
    }

    pub fn draw_public(&mut self) -> anyhow::Result<()> {
        self.draw()
    }

    pub fn cleanup_public(&mut self) -> anyhow::Result<()> {
        self.cleanup()
    }

    pub fn should_quit_public(&self) -> bool {
        self.should_quit
    }

    pub fn is_working_public(&self) -> bool {
        self.is_working
    }

    pub fn set_working(&mut self, working: bool) {
        self.is_working = working;
    }

    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    /// Set available models for the model selector overlay.
    pub fn set_models(&mut self, models: Vec<String>) {
        self.models = models;
    }

    /// Take the pending model selection (if user picked one from overlay).
    pub fn take_pending_model(&mut self) -> Option<String> {
        self.pending_model.take()
    }

    /// Switch the color theme by name.
    pub fn set_theme(&mut self, name: &str) {
        self.theme = crate::theme::Theme::by_name(name);
    }

    /// Get current theme name.
    pub fn theme_name(&self) -> &str {
        self.theme.name
    }

    /// Cycle to the next theme.
    pub fn cycle_theme(&mut self) {
        let themes = crate::theme::Theme::available();
        let current_idx = themes.iter().position(|&t| t == self.theme.name).unwrap_or(0);
        let next_idx = (current_idx + 1) % themes.len();
        self.theme = crate::theme::Theme::by_name(themes[next_idx]);
    }

    /// Switch chat focus to a sub-agent (or back to main).
    /// Returns true if the switch was successful.
    pub fn switch_chat_focus(&mut self, target: &str) -> bool {
        self.chat.switch_focus(target)
    }

    /// Get the current chat focus ("main" or a sub-agent ID).
    pub fn chat_focus(&self) -> &str {
        self.chat.current_focus()
    }

    /// Check if currently viewing the main agent's chat.
    pub fn is_main_chat_focus(&self) -> bool {
        self.chat.is_main_focus()
    }

    /// Push a chat entry to the main conversation (for sub-agent notifications).
    pub fn push_to_main_chat(&mut self, entry: crate::chat::ChatEntry) {
        self.chat.push_to_main(entry);
    }

    /// Push a chat entry to a specific sub-agent's conversation.
    pub fn push_to_sub_chat(&mut self, sub_id: &str, entry: crate::chat::ChatEntry) {
        self.chat.push_to_sub(sub_id, entry);
    }

    /// Update the sub-agent status counters (for Team Mode display).
    pub fn update_sub_agent_status(&mut self, total: usize, done: usize) {
        self.sub_agent_count = total;
        self.sub_agent_done = done;
    }

    /// Append a proxy log line to the buffer (capped at 1000 lines).
    pub fn push_proxy_log(&mut self, line: String) {
        if self.proxy_log.len() >= 1000 {
            self.proxy_log.pop_front();
        }
        self.proxy_log.push_back(line);
    }

    /// Switch to viewing the proxy log ("api" session).
    pub fn view_proxy(&mut self) {
        self.viewing_proxy = true;
    }

    /// Switch back to viewing the normal chat.
    pub fn view_chat(&mut self) {
        self.viewing_proxy = false;
    }
}
