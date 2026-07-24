//! Theme system — color schemes for the TUI.
//!
//! Provides a set of predefined color themes that control the appearance
//! of all TUI components. Themes can be switched at runtime.

use ratatui::style::{Color, Modifier, Style};

/// A complete color theme for the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    // Header
    pub header_version: Color,
    pub header_model: Color,
    pub header_language: Color,
    pub header_hint: Color,
    // Chat
    pub user_message: Color,
    pub assistant_text: Color,
    pub thinking: Color,
    pub tool_name_bg: Color,
    pub tool_error_bg: Color,
    pub system_message: Color,
    pub bash_command: Color,
    // Editor
    pub editor_border: Color,
    pub editor_border_bash: Color,
    pub editor_border_cmd: Color,
    // Footer
    pub footer_cwd: Color,
    pub footer_git: Color,
    pub footer_input_tokens: Color,
    pub footer_output_tokens: Color,
    pub footer_context_ok: Color,
    pub footer_context_warn: Color,
    pub footer_context_error: Color,
    pub footer_model: Color,
    // Status
    pub spinner: Color,
    // Markdown
    pub md_heading: Color,
    pub md_code: Color,
    pub md_code_block: Color,
    pub md_link: Color,
    pub md_quote: Color,
}

impl Theme {
    /// Default dark theme (PI-inspired).
    pub fn dark() -> Self {
        Self {
            name: "dark",
            header_version: Color::Cyan,
            header_model: Color::Green,
            header_language: Color::Blue,
            header_hint: Color::DarkGray,
            user_message: Color::Cyan,
            assistant_text: Color::Reset,
            thinking: Color::DarkGray,
            tool_name_bg: Color::Green,
            tool_error_bg: Color::Red,
            system_message: Color::Yellow,
            bash_command: Color::Green,
            editor_border: Color::Cyan,
            editor_border_bash: Color::Green,
            editor_border_cmd: Color::Blue,
            footer_cwd: Color::DarkGray,
            footer_git: Color::Magenta,
            footer_input_tokens: Color::Cyan,
            footer_output_tokens: Color::Green,
            footer_context_ok: Color::Green,
            footer_context_warn: Color::Yellow,
            footer_context_error: Color::Red,
            footer_model: Color::Blue,
            spinner: Color::Cyan,
            md_heading: Color::Blue,
            md_code: Color::Red,
            md_code_block: Color::Green,
            md_link: Color::Cyan,
            md_quote: Color::Yellow,
        }
    }

    /// Light theme for bright terminals.
    pub fn light() -> Self {
        Self {
            name: "light",
            header_version: Color::Blue,
            header_model: Color::Green,
            header_language: Color::Magenta,
            header_hint: Color::Gray,
            user_message: Color::Blue,
            assistant_text: Color::Black,
            thinking: Color::Gray,
            tool_name_bg: Color::Green,
            tool_error_bg: Color::Red,
            system_message: Color::Yellow,
            bash_command: Color::Green,
            editor_border: Color::Blue,
            editor_border_bash: Color::Green,
            editor_border_cmd: Color::Magenta,
            footer_cwd: Color::Gray,
            footer_git: Color::Magenta,
            footer_input_tokens: Color::Blue,
            footer_output_tokens: Color::Green,
            footer_context_ok: Color::Green,
            footer_context_warn: Color::Yellow,
            footer_context_error: Color::Red,
            footer_model: Color::Blue,
            spinner: Color::Blue,
            md_heading: Color::Blue,
            md_code: Color::Red,
            md_code_block: Color::Green,
            md_link: Color::Blue,
            md_quote: Color::Yellow,
        }
    }

    /// High-contrast theme for accessibility.
    pub fn high_contrast() -> Self {
        Self {
            name: "high-contrast",
            header_version: Color::White,
            header_model: Color::White,
            header_language: Color::White,
            header_hint: Color::Gray,
            user_message: Color::White,
            assistant_text: Color::White,
            thinking: Color::Gray,
            tool_name_bg: Color::White,
            tool_error_bg: Color::Red,
            system_message: Color::Yellow,
            bash_command: Color::White,
            editor_border: Color::White,
            editor_border_bash: Color::White,
            editor_border_cmd: Color::White,
            footer_cwd: Color::White,
            footer_git: Color::White,
            footer_input_tokens: Color::White,
            footer_output_tokens: Color::White,
            footer_context_ok: Color::White,
            footer_context_warn: Color::Yellow,
            footer_context_error: Color::Red,
            footer_model: Color::White,
            spinner: Color::White,
            md_heading: Color::White,
            md_code: Color::White,
            md_code_block: Color::White,
            md_link: Color::Cyan,
            md_quote: Color::Yellow,
        }
    }

    /// Get a theme by name.
    pub fn by_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "light" => Self::light(),
            "high-contrast" | "highcontrast" => Self::high_contrast(),
            _ => Self::dark(),
        }
    }

    /// List available theme names.
    pub fn available() -> &'static [&'static str] {
        &["dark", "light", "high-contrast"]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
