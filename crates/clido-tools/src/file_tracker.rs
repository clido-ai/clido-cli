//! File modification tracking: detect external changes between Read and Edit/Write.
//!
//! When a file is Read through the tool, its mtime is recorded. Before any subsequent
//! Edit or Write the tracker checks whether the mtime changed — if it did, the user
//! (or some other process) modified the file in the meantime, and we return an error
//! rather than silently overwriting their changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Shared, clone-cheap file-mtime tracker.
#[derive(Clone, Default)]
pub struct FileTracker {
    inner: Arc<Mutex<HashMap<PathBuf, u64>>>,
}

impl FileTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the mtime of a file after a successful Read.
    pub fn record(&self, path: &Path, mtime_nanos: u64) {
        if mtime_nanos > 0 {
            self.inner.lock().unwrap().insert(path.to_path_buf(), mtime_nanos);
        }
    }

    /// Check whether a previously-tracked file has been modified externally.
    ///
    /// Returns `Some(error_message)` when the file's current mtime differs from
    /// what was recorded, `None` when the file is untracked or unchanged.
    pub fn check_not_stale(&self, path: &Path) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let Some(&recorded) = inner.get(path) else {
            return None; // not tracked — no protection required
        };
        let current = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        if current != 0 && current != recorded {
            Some(format!(
                "File '{}' was modified externally since it was last read. \
                 Re-read the file before editing to avoid overwriting those changes.",
                path.display()
            ))
        } else {
            None
        }
    }

    /// Update the tracker after a successful write so the next check passes.
    pub fn update(&self, path: &Path, mtime_nanos: u64) {
        if mtime_nanos > 0 {
            self.inner.lock().unwrap().insert(path.to_path_buf(), mtime_nanos);
        }
    }
}
