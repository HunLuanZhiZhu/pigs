//! Command output channel — abstracts over stdout (REPL) vs in-memory buffer (TUI).
//!
//! In REPL mode the sink writes directly to stdout (`println!`).
//! In TUI mode the sink accumulates text into a `String` buffer that the
//! caller drains after `handle_command` returns and pushes into the TUI
//! chat via `App::push_to_main_chat(ChatEntry::System(...))`.
//!
//! This avoids raw `println!` calls corrupting the ratatui alternate-screen
//! differential renderer when slash commands run inside the TUI loop.

use std::fmt::Display;

/// Command output channel shared by all slash-command handlers.
pub enum OutputSink {
    /// Plain stdout (REPL mode). Calls `println!` directly.
    Stdout,
    /// In-memory buffer collected by the TUI caller.
    Buffer(String),
}

impl Default for OutputSink {
    fn default() -> Self {
        Self::Stdout
    }
}

impl OutputSink {
    /// Emit a line of output. Appends a trailing newline.
    pub fn println(&mut self, s: impl Display) {
        match self {
            OutputSink::Stdout => println!("{s}"),
            OutputSink::Buffer(buf) => {
                buf.push_str(&s.to_string());
                buf.push('\n');
            }
        }
    }

    /// Emit an empty line.
    pub fn println_empty(&mut self) {
        match self {
            OutputSink::Stdout => println!(),
            OutputSink::Buffer(buf) => buf.push('\n'),
        }
    }

    /// Emit an error line. In TUI mode errors share the same channel so
    /// they also render through `ChatEntry::System` rather than bypassing
    /// the sink by writing to stderr.
    pub fn eprintln(&mut self, s: impl Display) {
        self.println(s);
    }

    /// Take the accumulated buffer (TUI mode), clearing it.
    /// Returns an empty string for `Stdout` mode (nothing was buffered).
    pub fn take_buffer(&mut self) -> String {
        match self {
            OutputSink::Stdout => String::new(),
            OutputSink::Buffer(buf) => std::mem::take(buf),
        }
    }

    /// Returns `true` if this sink is currently buffering output.
    pub fn is_buffer(&self) -> bool {
        matches!(self, OutputSink::Buffer(_))
    }
}
