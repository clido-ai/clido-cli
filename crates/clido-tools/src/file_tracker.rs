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
            self.inner
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), mtime_nanos);
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
            self.inner
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), mtime_nanos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_tracker() {
        let tracker = FileTracker::new();
        // Untracked path should return None
        assert!(tracker
            .check_not_stale(Path::new("/some/nonexistent/path"))
            .is_none());
    }

    #[test]
    fn record_zero_mtime_is_ignored() {
        let tracker = FileTracker::new();
        let p = Path::new("/tmp/test_file.txt");
        tracker.record(p, 0); // zero mtime should be ignored
                              // Since not tracked (zero was ignored), check should return None
        assert!(tracker.check_not_stale(p).is_none());
    }

    #[test]
    fn check_not_stale_returns_none_for_untracked_path() {
        let tracker = FileTracker::new();
        let result = tracker.check_not_stale(Path::new("/tmp/untracked_file_xyz.txt"));
        assert!(result.is_none());
    }

    #[test]
    fn check_not_stale_ok_when_mtime_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tracked.txt");
        std::fs::write(&file_path, "content").unwrap();

        // Get the actual mtime
        let meta = std::fs::metadata(&file_path).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        let tracker = FileTracker::new();
        tracker.record(&file_path, mtime);
        // No external modification → should be None (not stale)
        assert!(tracker.check_not_stale(&file_path).is_none());
    }

    #[test]
    fn check_not_stale_detects_modification() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("modified.txt");
        std::fs::write(&file_path, "original").unwrap();

        // Record with an old mtime (1 = epoch + 1 ns)
        let tracker = FileTracker::new();
        tracker.record(&file_path, 1); // old mtime

        // Now file has a much newer mtime → should detect as stale
        let result = tracker.check_not_stale(&file_path);
        assert!(result.is_some(), "expected stale detection");
        assert!(result.unwrap().contains("modified externally"));
    }

    #[test]
    fn update_refreshes_tracked_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("update_test.txt");
        std::fs::write(&file_path, "v1").unwrap();

        // Get current mtime
        let meta = std::fs::metadata(&file_path).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        let tracker = FileTracker::new();
        tracker.record(&file_path, 1); // stale old mtime

        // Now update with the real mtime
        tracker.update(&file_path, mtime);

        // Should not be stale anymore
        assert!(tracker.check_not_stale(&file_path).is_none());
    }

    #[test]
    fn update_zero_mtime_is_ignored() {
        let tracker = FileTracker::new();
        let p = Path::new("/tmp/zero_test.txt");
        tracker.record(p, 999);
        tracker.update(p, 0); // should be ignored
                              // Should still have the old recorded mtime (999)
        let inner = tracker.inner.lock().unwrap();
        assert_eq!(inner.get(p), Some(&999));
    }

    #[test]
    fn clone_shares_inner_state() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("shared.txt");
        std::fs::write(&file_path, "x").unwrap();

        let tracker1 = FileTracker::new();
        let tracker2 = tracker1.clone();

        tracker1.record(&file_path, 12345);
        // tracker2 should see the same record
        let inner = tracker2.inner.lock().unwrap();
        assert_eq!(inner.get(&file_path), Some(&12345));
    }
}
