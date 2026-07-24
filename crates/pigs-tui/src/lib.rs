//! pig Terminal UI — ratatui + crossterm implementation.
//!
//! Layout (top to bottom, aligned with PI):
//!   1. Header      — version, model, keybinding hints
//!   2. Chat history — user messages, assistant messages (markdown), tool calls
//!   3. Status       — "Working..." spinner or idle
//!   4. Editor       — multi-line input (Emacs-style, via tui-textarea)
//!   5. Footer       — pwd, git branch, token stats, model name

pub mod app;
pub mod chat;
pub mod editor;
pub mod extensions;
pub mod footer;
pub mod header;
pub mod image;
pub mod markdown;
pub mod overlay;
pub mod theme;
pub mod tool_display;
pub mod event;

#[cfg(test)]
mod tests;

// Re-export key types for external use
pub use app::App;
pub use event::{AppEvent, EventBroker};
pub use theme::Theme;
pub use crossterm;
