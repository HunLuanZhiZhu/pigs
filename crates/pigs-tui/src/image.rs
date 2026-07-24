//! Image display — inline image rendering in terminal.
//!
//! Supports the Kitty graphics protocol and iTerm2 inline image protocol.
//! Falls back to a placeholder `[image: filename]` when neither is available.
//!
//! Detection is done by checking terminal capabilities via environment
//! variables and the TERM_PROGRAM variable.

use std::path::{Path, PathBuf};

/// Supported terminal image protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Kitty graphics protocol (ESC ] 52 G ...).
    Kitty,
    /// iTerm2 inline image protocol (ESC ] 1337 ...).
    Iterm2,
    /// Sixel graphics (DCS ... ST).
    Sixel,
    /// No image protocol detected — show placeholder.
    None,
}

/// Detect the best available terminal image protocol.
pub fn detect_protocol() -> ImageProtocol {
    // Check TERM_PROGRAM for known terminals
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();

    // Kitty
    if term_program == "kitty" || term.starts_with("xterm-kitty") {
        return ImageProtocol::Kitty;
    }

    // iTerm2 / WezTerm (supports iTerm2 protocol)
    if term_program == "iTerm.app" || term_program == "WezTerm" {
        return ImageProtocol::Iterm2;
    }

    // Check for sixel support via TERM capa
    if term.contains("sixel") {
        return ImageProtocol::Sixel;
    }

    // Check kitty keyboard protocol support (implies graphics support)
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageProtocol::Kitty;
    }

    ImageProtocol::None
}

/// An image entry for display in the chat history.
#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub alt_text: String,
}

impl ImageEntry {
    /// Create a new image entry from a file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let alt_text = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string();
        Self {
            path,
            width: 0,
            height: 0,
            alt_text,
        }
    }

    /// Check if the file exists and is a supported image format.
    pub fn is_valid(&self) -> bool {
        if !self.path.exists() {
            return false;
        }
        let ext = self
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    }

    /// Render the image for terminal display.
    /// Returns a string that can be printed to the terminal.
    /// Uses the best available protocol, or a placeholder if none.
    pub fn render(&self, protocol: ImageProtocol) -> String {
        if !self.is_valid() {
            return format!("[image not found: {}]", self.path.display());
        }

        match protocol {
            ImageProtocol::Kitty => self.render_kitty(),
            ImageProtocol::Iterm2 => self.render_iterm2(),
            ImageProtocol::Sixel => format!("[sixel image: {}]", self.alt_text),
            ImageProtocol::None => format!("[image: {}]", self.alt_text),
        }
    }

    /// Render using Kitty graphics protocol.
    /// Reads the file, base64-encodes it, and wraps in the escape sequence.
    fn render_kitty(&self) -> String {
        match std::fs::read(&self.path) {
            Ok(data) => {
                use std::fmt::Write;
                let b64 = base64_encode(&data);
                let mut result = String::new();
                // Kitty graphics protocol: ESC ] 52 G ; a=T ; <base64> \
                // We send it in chunks if needed
                let chunk_size = 4096;
                let mut offset = 0;
                while offset < b64.len() {
                    let end = (offset + chunk_size).min(b64.len());
                    let chunk = &b64[offset..end];
                    let is_first = offset == 0;
                    let is_last = end == b64.len();

                    let mut params = String::new();
                    if is_first {
                        params.push_str("a=T,f=100");
                        if self.width > 0 {
                            let _ = write!(params, ",c={}", self.width);
                        }
                        if self.height > 0 {
                            let _ = write!(params, ",r={}", self.height);
                        }
                    } else {
                        params.push_str("m=1");
                    }
                    if is_last {
                        params.push_str(",m=0");
                    } else if !is_first {
                        params.push_str("m=1");
                    }

                    let _ = write!(result, "\x1b]52;G;{}\x1b\\", chunk);
                    offset = end;
                }
                result
            }
            Err(e) => format!("[image read error: {e}]"),
        }
    }

    /// Render using iTerm2 inline image protocol.
    fn render_iterm2(&self) -> String {
        match std::fs::read(&self.path) {
            Ok(data) => {
                let b64 = base64_encode(&data);
                let filename = self
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image");
                let dims = if self.width > 0 && self.height > 0 {
                    format!(";width={}px;height={}px", self.width, self.height)
                } else {
                    String::new()
                };
                format!(
                    "\x1b]1337;File=name={};inline=1{}:{}\x07",
                    base64_encode(filename.as_bytes()),
                    dims,
                    b64
                )
            }
            Err(e) => format!("[image read error: {e}]"),
        }
    }
}

/// Simple base64 encoder (avoids adding a base64 dependency).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push(CHARS[((n >> 6) & 63) as usize] as char);
        result.push(CHARS[(n & 63) as usize] as char);
        i += 3;
    }
    let remaining = data.len() - i;
    if remaining == 1 {
        let n = (data[i] as u32) << 16;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push('=');
        result.push('=');
    } else if remaining == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push(CHARS[((n >> 6) & 63) as usize] as char);
        result.push('=');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_basic() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn image_protocol_detection() {
        // detect_protocol should not panic
        let _ = detect_protocol();
    }

    #[test]
    fn image_entry_validity() {
        let entry = ImageEntry::new("/nonexistent/image.png");
        assert!(!entry.is_valid());

        let entry = ImageEntry::new("/nonexistent/file.txt");
        assert!(!entry.is_valid()); // wrong extension
    }

    #[test]
    fn image_entry_render_placeholder() {
        let entry = ImageEntry::new("/nonexistent/test.png");
        let rendered = entry.render(ImageProtocol::None);
        // File doesn't exist, so it should show "not found" or "[image:" placeholder
        assert!(rendered.contains("[image") || rendered.contains("not found"));
    }

    #[test]
    fn image_entry_render_kitty_fallback() {
        let entry = ImageEntry::new("/nonexistent/test.png");
        let rendered = entry.render(ImageProtocol::Kitty);
        // Should show "not found" since file doesn't exist
        assert!(rendered.contains("[image not found:") || rendered.contains("read error"));
    }
}
