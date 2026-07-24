//! File snapshots for undoable write tools.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

const MAX_BATCHES: usize = 20;

/// A single file snapshot entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnapshot {
    pub path: PathBuf,
    /// None means the file did not exist before the write (or was unreadable as UTF-8).
    pub previous_content: Option<String>,
    pub existed: bool,
}

/// A batch of file snapshots from one tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotBatch {
    pub id: String,
    pub tool_name: String,
    pub created_at: String,
    pub files: Vec<FileSnapshot>,
}

/// In-memory undo stack (also optionally persisted under .pig/undo/).
#[derive(Debug, Default)]
pub struct SnapshotStore {
    batches: VecDeque<SnapshotBatch>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            batches: VecDeque::new(),
        }
    }

    pub fn push(&mut self, batch: SnapshotBatch) {
        self.batches.push_back(batch);
        while self.batches.len() > MAX_BATCHES {
            self.batches.pop_front();
        }
    }

    pub fn pop(&mut self) -> Option<SnapshotBatch> {
        self.batches.pop_back()
    }

    pub fn list(&self) -> Vec<&SnapshotBatch> {
        self.batches.iter().rev().collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.batches.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    /// Load persisted snapshot batches from `{workspace}/.pig/undo/*.json`.
    /// Newest batches end up on top of the undo stack.
    pub fn load_from_workspace(workspace: &Path) -> Self {
        let mut store = Self::new();
        let dir = workspace.join(".pigs").join("undo");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return store;
        };

        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("json"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();

        for path in files {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(batch) = serde_json::from_str::<SnapshotBatch>(&text) {
                    store.push(batch);
                }
            }
        }
        store
    }
}

/// Capture current file text for a path before mutation.
pub fn capture_file_snapshot(path: &Path) -> FileSnapshot {
    match std::fs::read_to_string(path) {
        Ok(content) => FileSnapshot {
            path: path.to_path_buf(),
            previous_content: Some(content),
            existed: true,
        },
        Err(_) => FileSnapshot {
            path: path.to_path_buf(),
            previous_content: None,
            existed: path.exists(),
        },
    }
}

/// Create a snapshot batch id.
pub fn new_batch_id() -> String {
    format!("snap-{}", Utc::now().format("%Y%m%d%H%M%S%3f"))
}

/// Restore a snapshot batch to disk.
pub fn restore_batch(batch: &SnapshotBatch) -> Result<Vec<String>, String> {
    let mut report = Vec::new();
    // Restore in reverse order for safety with multi-file ops.
    for file in batch.files.iter().rev() {
        if let Some(content) = &file.previous_content {
            if let Some(parent) = file.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&file.path, content).map_err(|e| e.to_string())?;
            report.push(format!("restored {}", file.path.display()));
        } else if file.path.exists() {
            // File was newly created by the tool; delete it on undo.
            std::fs::remove_file(&file.path).map_err(|e| e.to_string())?;
            report.push(format!("deleted {}", file.path.display()));
        } else {
            report.push(format!("skipped missing {}", file.path.display()));
        }
    }
    Ok(report)
}

/// Persist batch metadata under `.pig/undo/`.
pub fn persist_batch(workspace: &Path, batch: &SnapshotBatch) -> Result<PathBuf, String> {
    let dir = workspace.join(".pigs").join("undo");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", batch.id));
    let json = serde_json::to_string_pretty(batch).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Load a single snapshot batch from disk.
#[allow(dead_code)]
pub fn load_batch(path: &Path) -> Result<SnapshotBatch, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "pigs_undo_{label}_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".pigs")).unwrap();
        dir
    }

    #[test]
    fn test_capture_restore_existing_file() {
        let ws = temp_workspace("existing");
        let file = ws.join("note.txt");
        fs::write(&file, "before\n").unwrap();

        let snap = capture_file_snapshot(&file);
        assert!(snap.existed);
        assert_eq!(snap.previous_content.as_deref(), Some("before\n"));

        fs::write(&file, "after\n").unwrap();
        let batch = SnapshotBatch {
            id: "snap-test-existing".into(),
            tool_name: "write".into(),
            created_at: "now".into(),
            files: vec![snap],
        };
        let report = restore_batch(&batch).unwrap();
        assert!(report.iter().any(|r| r.contains("restored")));
        assert_eq!(fs::read_to_string(&file).unwrap(), "before\n");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_restore_deletes_new_file() {
        let ws = temp_workspace("newfile");
        let file = ws.join("created.txt");
        let snap = capture_file_snapshot(&file);
        assert!(!snap.existed);
        assert!(snap.previous_content.is_none());

        fs::write(&file, "brand new\n").unwrap();
        assert!(file.exists());

        let batch = SnapshotBatch {
            id: "snap-test-new".into(),
            tool_name: "write".into(),
            created_at: "now".into(),
            files: vec![snap],
        };
        let report = restore_batch(&batch).unwrap();
        assert!(report.iter().any(|r| r.contains("deleted")));
        assert!(!file.exists());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_persist_and_load_store() {
        let ws = temp_workspace("persist");
        let file = ws.join("a.txt");
        fs::write(&file, "v1\n").unwrap();
        let snap = capture_file_snapshot(&file);

        let batch = SnapshotBatch {
            id: "snap-test-persist".into(),
            tool_name: "edit".into(),
            created_at: "now".into(),
            files: vec![snap],
        };
        let path = persist_batch(&ws, &batch).unwrap();
        assert!(path.exists());

        let loaded = load_batch(&path).unwrap();
        assert_eq!(loaded, batch);

        fs::write(&file, "v2\n").unwrap();
        restore_batch(&loaded).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "v1\n");

        let mut store = SnapshotStore::load_from_workspace(&ws);
        assert_eq!(store.len(), 1);
        let popped = store.pop().unwrap();
        assert_eq!(popped.id, "snap-test-persist");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_store_max_batches() {
        let mut store = SnapshotStore::new();
        for i in 0..25 {
            store.push(SnapshotBatch {
                id: format!("snap-{i}"),
                tool_name: "write".into(),
                created_at: "now".into(),
                files: vec![],
            });
        }
        assert_eq!(store.len(), 20);
        assert_eq!(store.pop().unwrap().id, "snap-24");
    }
}
