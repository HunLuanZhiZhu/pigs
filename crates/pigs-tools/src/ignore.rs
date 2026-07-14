//! .pigsignore file support — load ignore patterns that tools respect.
//!
//! When a `.pigsignore` file exists in the workspace root, its patterns are
//! loaded and used by grep_search, glob_search, and list_files to exclude
//! matching paths. The format is the same as .gitignore (one pattern per line,
//! # comments, blank lines ignored).

use std::path::{Path, PathBuf};

/// A set of ignore patterns loaded from a `.pigsignore` file.
#[derive(Debug, Clone, Default)]
pub struct IgnorePatterns {
    patterns: Vec<String>,
    #[allow(dead_code)]
    loaded_from: Option<PathBuf>,
}

impl IgnorePatterns {
    /// Load ignore patterns from a `.pigsignore` file in the given directory.
    /// Returns an empty pattern set if the file doesn't exist.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(".pigsignore");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let patterns: Vec<String> = content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect();
                IgnorePatterns {
                    patterns,
                    loaded_from: Some(path),
                }
            }
            Err(_) => IgnorePatterns::default(),
        }
    }

    /// Check if a given path should be ignored.
    /// Matches against the file/directory name and the relative path.
    pub fn is_ignored(&self, path: &Path, base: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }

        // Get the relative path from base
        let rel = path.strip_prefix(base).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for pattern in &self.patterns {
            // Direct name match
            if name == pattern {
                return true;
            }

            // Glob-style pattern match
            if let Ok(glob_pat) = glob::Pattern::new(pattern) {
                // Match against the full relative path
                if glob_pat.matches(&rel_str) {
                    return true;
                }
                // Match against just the name
                if glob_pat.matches(name) {
                    return true;
                }
                // Match with ** prefix (directory-aware)
                if let Ok(rglob) = glob::Pattern::new(&format!("**/{pattern}")) {
                    if rglob.matches(&rel_str) {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Check if a directory name should always be excluded (built-in defaults).
    pub fn is_default_ignored(name: &str) -> bool {
        matches!(
            name,
            ".git"
                | "node_modules"
                | "target"
                | "__pycache__"
                | ".next"
                | "dist"
                | "build"
                | ".cache"
                | ".venv"
                | "venv"
                | ".idea"
                | ".vscode"
                | "coverage"
                | ".nuxt"
                | ".turbo"
                | ".parcel-cache"
        ) || name.starts_with(".")
    }

    /// Get the number of patterns loaded.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Check if any patterns were loaded.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_missing_file() {
        let patterns = IgnorePatterns::load(Path::new("/nonexistent"));
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_load_and_match() {
        // Create a temp .pigsignore file
        let temp_dir = std::env::temp_dir().join("pigs_ignore_test");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let ignore_path = temp_dir.join(".pigsignore");
        let mut file = std::fs::File::create(&ignore_path).unwrap();
        writeln!(file, "# Comment line").unwrap();
        writeln!(file, "secrets.env").unwrap();
        writeln!(file, "*.log").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "temp/").unwrap();

        let patterns = IgnorePatterns::load(&temp_dir);
        assert_eq!(patterns.pattern_count(), 3);

        // Test matching
        assert!(patterns.is_ignored(&temp_dir.join("secrets.env"), &temp_dir));
        assert!(patterns.is_ignored(&temp_dir.join("app.log"), &temp_dir));
        assert!(!patterns.is_ignored(&temp_dir.join("main.rs"), &temp_dir));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_default_ignored() {
        assert!(IgnorePatterns::is_default_ignored("node_modules"));
        assert!(IgnorePatterns::is_default_ignored(".git"));
        assert!(IgnorePatterns::is_default_ignored("target"));
        assert!(!IgnorePatterns::is_default_ignored("src"));
        assert!(!IgnorePatterns::is_default_ignored("main.rs"));
    }
}
