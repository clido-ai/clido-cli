//! Local cache for model metadata snapshots.

use crate::provider::ModelsSnapshot;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_STALENESS: Duration = Duration::from_secs(60 * 60); // 60 minutes

/// Manages the local models.json cache.
pub struct ModelCache {
    path: PathBuf,
}

impl ModelCache {
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            path: config_dir.join("models.json"),
        }
    }

    /// Read cached snapshot. Returns None if file doesn't exist or is invalid.
    pub fn read(&self) -> Option<ModelsSnapshot> {
        let content = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Write a snapshot to the cache file. Creates parent dirs if needed.
    pub fn write(&self, snapshot: &ModelsSnapshot) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(snapshot)?;
        std::fs::write(&self.path, content)
    }

    /// Check if the cache is stale (older than 60 minutes).
    pub fn is_stale(&self) -> bool {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return true,
        };
        let modified = match meta.modified() {
            Ok(m) => m,
            Err(_) => return true,
        };
        modified.elapsed().unwrap_or(Duration::MAX) > CACHE_STALENESS
    }

    /// Check if a cached snapshot exists and is fresh.
    pub fn fresh_snapshot(&self) -> Option<ModelsSnapshot> {
        if !self.is_stale() {
            self.read()
        } else {
            None
        }
    }

    /// Record the fetch timestamp on a snapshot and return it.
    pub fn stamp_snapshot(snapshot: &mut ModelsSnapshot) {
        snapshot.fetched_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
    }
}
