//! Chat history rendering — user messages, assistant messages (markdown), tool calls.
//!
//! Supports sub-agent session switching (pigs = pig + s):
//! - `main` focus: shows the main agent's conversation
//! - `sub-xxx` focus: shows the specified sub-agent's conversation
//! - A header line indicates which agent's conversation is being viewed

use std::collections::HashMap;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::markdown;

/// A single chat entry (user message, assistant message, or tool call).
#[derive(Debug, Clone)]
pub enum ChatEntry {
    /// User input.
    User(String),
    /// Assistant response (may include thinking blocks).
    Assistant {
        text: String,
        thinking: Option<String>,
    },
    /// Tool call result.
    ToolCall {
        name: String,
        args: String,
        result: String,
        is_error: bool,
    },
    /// System / status message.
    System(String),
    /// Bash command output (! mode).
    Bash { command: String, output: String, exit_code: i32 },
    /// Sub-agent created notification.
    SubAgentCreated { id: String, task: String, mode: String },
    /// Sub-agent completed notification.
    SubAgentDone { id: String, success: bool, result: String },
}

/// The chat history state — supports focus switching between main and sub-agents.
pub struct ChatState {
    /// Main agent's chat entries.
    pub main_entries: Vec<ChatEntry>,
    /// Per-sub-agent chat entries (keyed by sub-agent ID like "sub-001").
    pub sub_entries: HashMap<String, Vec<ChatEntry>>,
    /// Current focus: "main" or a sub-agent ID.
    pub focus: String,
    pub scroll_offset: usize,
}

impl ChatState {
    pub fn new() -> Self {
        Self {
            main_entries: Vec::new(),
            sub_entries: HashMap::new(),
            focus: "main".to_string(),
            scroll_offset: 0,
        }
    }

    /// Push an entry to the currently focused conversation.
    pub fn push(&mut self, entry: ChatEntry) {
        if self.focus == "main" {
            self.main_entries.push(entry);
        } else {
            self.sub_entries
                .entry(self.focus.clone())
                .or_default()
                .push(entry);
        }
        self.scroll_offset = 0;
    }

    /// Push an entry to the main conversation (regardless of current focus).
    pub fn push_to_main(&mut self, entry: ChatEntry) {
        self.main_entries.push(entry);
        self.scroll_offset = 0;
    }

    /// Push an entry to a specific sub-agent's conversation.
    pub fn push_to_sub(&mut self, sub_id: &str, entry: ChatEntry) {
        self.sub_entries
            .entry(sub_id.to_string())
            .or_default()
            .push(entry);
    }

    /// Switch focus to a sub-agent (or back to main).
    pub fn switch_focus(&mut self, target: &str) -> bool {
        if target == "main" {
            self.focus = "main".to_string();
            self.scroll_offset = 0;
            true
        } else if self.sub_entries.contains_key(target) {
            self.focus = target.to_string();
            self.scroll_offset = 0;
            true
        } else {
            false
        }
    }

    /// Get the current focus identifier.
    pub fn current_focus(&self) -> &str {
        &self.focus
    }

    /// Check if currently viewing the main agent.
    pub fn is_main_focus(&self) -> bool {
        self.focus == "main"
    }

    /// Get a mutable reference to the last entry of the focused conversation.
    pub fn last_focused_entry_mut(&mut self) -> Option<&mut ChatEntry> {
        if self.focus == "main" {
            self.main_entries.last_mut()
        } else {
            self.sub_entries.get_mut(&self.focus).and_then(|v| v.last_mut())
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    /// Get the entries for the currently focused conversation.
    fn focused_entries(&self) -> &[ChatEntry] {
        if self.focus == "main" {
            &self.main_entries
        } else {
            self.sub_entries
                .get(&self.focus)
                .map(|v| v.as_slice())
                .unwrap_or(&[])
        }
    }

    /// Render the chat history into a list of lines for the given width.
    pub fn render_lines(&self, width: usize) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // If viewing a sub-agent, show a header banner
        if self.focus != "main" {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" ── {} ", self.focus),
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "(use /sub back to return to main)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::raw(""));
        }

        let entries = self.focused_entries();

        for entry in entries {
            match entry {
                ChatEntry::User(text) => {
                    lines.push(Line::from(vec![
                        Span::styled(" > ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    ]));
                    let wrapped = wrap_text(text, width.saturating_sub(2));
                    for w in wrapped {
                        lines.push(Line::from(vec![
                            Span::raw("   "),
                            Span::styled(w, Style::default().fg(Color::Cyan)),
                        ]));
                    }
                    lines.push(Line::raw(""));
                }
                ChatEntry::Assistant { text, thinking } => {
                    if let Some(think) = thinking {
                        if !think.is_empty() {
                            lines.push(Line::from(vec![
                                Span::styled(" [thinking] ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                            ]));
                            let wrapped = wrap_text(think, width.saturating_sub(2));
                            for w in wrapped {
                                lines.push(Line::from(vec![
                                    Span::styled(format!("   {w}"), Style::default().fg(Color::DarkGray)),
                                ]));
                            }
                            lines.push(Line::raw(""));
                        }
                    }
                    let md_lines = markdown::render_to_lines(text, width);
                    lines.extend(md_lines);
                    lines.push(Line::raw(""));
                }
                ChatEntry::ToolCall { name, args, result, is_error } => {
                    let bg = if *is_error { Color::Red } else { Color::Green };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" [{name}] "), Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD)),
                    ]));
                    if !args.is_empty() {
                        let wrapped = wrap_text(args, width.saturating_sub(4));
                        for w in wrapped {
                            lines.push(Line::from(vec![
                                Span::styled(format!("   {w}"), Style::default().fg(Color::Gray)),
                            ]));
                        }
                    }
                    if !result.is_empty() {
                        let wrapped = wrap_text(result, width.saturating_sub(4));
                        for w in wrapped {
                            lines.push(Line::from(vec![
                                Span::styled(format!("   {w}"), Style::default().fg(Color::Gray)),
                            ]));
                        }
                    }
                    lines.push(Line::raw(""));
                }
                ChatEntry::System(text) => {
                    for line in text.lines() {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {line}"), Style::default().fg(Color::Yellow)),
                        ]));
                    }
                    lines.push(Line::raw(""));
                }
                ChatEntry::Bash { command, output, exit_code } => {
                    lines.push(Line::from(vec![
                        Span::styled(format!(" $ {command}"), Style::default().fg(Color::Green)),
                    ]));
                    if !output.is_empty() {
                        for line in output.lines() {
                            lines.push(Line::raw(format!("   {line}")));
                        }
                    }
                    if *exit_code != 0 {
                        lines.push(Line::from(vec![
                            Span::styled(format!(" [exit: {exit_code}]"), Style::default().fg(Color::Red)),
                        ]));
                    }
                    lines.push(Line::raw(""));
                }
                ChatEntry::SubAgentCreated { id, task, mode } => {
                    lines.push(Line::from(vec![
                        Span::styled(format!(" [spawn] "), Style::default().fg(Color::White).bg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{id} [{mode}] "), Style::default().fg(Color::Magenta)),
                        Span::raw(task.clone()),
                    ]));
                    lines.push(Line::raw(""));
                }
                ChatEntry::SubAgentDone { id, success, result } => {
                    let (icon, color) = if *success {
                        ("✓", Color::Green)
                    } else {
                        ("✗", Color::Red)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {icon} [{id}] done "), Style::default().fg(Color::White).bg(color).add_modifier(Modifier::BOLD)),
                    ]));
                    if !result.is_empty() {
                        let preview: String = result.chars().take(200).collect();
                        lines.push(Line::from(vec![
                            Span::styled(format!("   {preview}"), Style::default().fg(Color::Gray)),
                        ]));
                    }
                    lines.push(Line::raw(""));
                }
            }
        }

        // Apply scroll offset
        if self.scroll_offset > 0 {
            let total = lines.len();
            let visible = total.saturating_sub(self.scroll_offset);
            lines.truncate(visible);
        }

        lines
    }
}

/// Wrap text to fit within a given width, respecting unicode width.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let wrapper = textwrap::Options::new(width)
        .break_words(true)
        .word_separator(textwrap::WordSeparator::AsciiSpace);
    textwrap::wrap(text, &wrapper)
        .into_iter()
        .map(|cow| cow.into_owned())
        .collect()
}

impl Default for ChatState {
    fn default() -> Self {
        Self::new()
    }
}
