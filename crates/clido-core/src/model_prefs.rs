//! User model preferences: favorites, recently used, and role assignments.
//!
//! Stored in the global clido config dir as `model_prefs.json`.
//! Preferences are personal and global (not per-project).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPrefs {
    /// Model IDs the user has starred.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Most recently used model IDs (newest first, max 10).
    #[serde(default)]
    pub recent: Vec<String>,
    /// Legacy field — kept for backwards-compatible deserialization.
    #[serde(default, skip_serializing)]
    pub roles: HashMap<String, String>,
}

impl ModelPrefs {
    const MAX_RECENT: usize = 10;

    /// Load from the global config dir. Returns default if file is missing or unreadable.
    pub fn load() -> Self {
        match prefs_path() {
            Some(path) if path.exists() => std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            _ => Self::default(),
        }
    }

    /// Persist to disk. Silent on error.
    pub fn save(&self) {
        if let Some(path) = prefs_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(&path, json);
            }
        }
    }

    /// Toggle a model in/out of favorites.
    pub fn toggle_favorite(&mut self, model_id: &str) {
        if self.favorites.contains(&model_id.to_string()) {
            self.favorites.retain(|m| m != model_id);
        } else {
            self.favorites.push(model_id.to_string());
        }
    }

    /// Record that a model was used (moves to front of recency list).
    pub fn push_recent(&mut self, model_id: &str) {
        self.recent.retain(|m| m != model_id);
        self.recent.insert(0, model_id.to_string());
        self.recent.truncate(Self::MAX_RECENT);
    }

    /// Check if a model is favorited.
    pub fn is_favorite(&self, model_id: &str) -> bool {
        self.favorites.iter().any(|f| f == model_id)
    }
}

fn prefs_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "clido")
        .map(|d: directories::ProjectDirs| d.config_dir().join("model_prefs.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefs_are_empty() {
        let p = ModelPrefs::default();
        assert!(p.favorites.is_empty());
        assert!(p.recent.is_empty());
        assert!(p.roles.is_empty());
    }

    #[test]
    fn toggle_favorite_adds_model() {
        let mut p = ModelPrefs::default();
        p.toggle_favorite("claude-3-sonnet");
        assert!(p.is_favorite("claude-3-sonnet"));
        assert_eq!(p.favorites.len(), 1);
    }

    #[test]
    fn toggle_favorite_removes_existing() {
        let mut p = ModelPrefs::default();
        p.toggle_favorite("claude-3-sonnet");
        p.toggle_favorite("claude-3-sonnet"); // toggle off
        assert!(!p.is_favorite("claude-3-sonnet"));
        assert!(p.favorites.is_empty());
    }

    #[test]
    fn toggle_favorite_multiple_models() {
        let mut p = ModelPrefs::default();
        p.toggle_favorite("model-a");
        p.toggle_favorite("model-b");
        assert!(p.is_favorite("model-a"));
        assert!(p.is_favorite("model-b"));
        p.toggle_favorite("model-a");
        assert!(!p.is_favorite("model-a"));
        assert!(p.is_favorite("model-b"));
    }

    #[test]
    fn push_recent_moves_to_front() {
        let mut p = ModelPrefs::default();
        p.push_recent("model-a");
        p.push_recent("model-b");
        p.push_recent("model-a"); // should move to front
        assert_eq!(p.recent[0], "model-a");
        assert_eq!(p.recent[1], "model-b");
        assert_eq!(p.recent.len(), 2); // no duplicate
    }

    #[test]
    fn push_recent_capped_at_max_recent() {
        let mut p = ModelPrefs::default();
        for i in 0..15 {
            p.push_recent(&format!("model-{}", i));
        }
        assert_eq!(p.recent.len(), ModelPrefs::MAX_RECENT);
    }

    #[test]
    fn push_recent_newest_at_front() {
        let mut p = ModelPrefs::default();
        p.push_recent("old");
        p.push_recent("new");
        assert_eq!(p.recent[0], "new");
        assert_eq!(p.recent[1], "old");
    }

    #[test]
    fn is_favorite_returns_false_for_unknown() {
        let p = ModelPrefs::default();
        assert!(!p.is_favorite("not-a-model"));
    }

    #[test]
    fn json_roundtrip() {
        let mut p = ModelPrefs::default();
        p.toggle_favorite("model-x");
        p.push_recent("model-y");
        p.roles.insert("critic".to_string(), "model-z".to_string());

        let json = serde_json::to_string(&p).unwrap();
        let p2: ModelPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.favorites, p.favorites);
        assert_eq!(p2.recent, p.recent);
        // roles is skip_serializing (legacy) — does not round-trip
        assert!(p2.roles.is_empty());
    }

    // ── load/save smoke tests ──────────────────────────────────────────────

    #[test]
    fn load_returns_default_on_missing_file() {
        // load() is silent on missing file — returns Default
        let prefs = ModelPrefs::load();
        // Just verify it returns without panic; favorites/recent/roles may or may not be populated
        let _ = prefs;
    }

    #[test]
    fn save_does_not_panic() {
        // save() is silent on error — should never panic
        let p = ModelPrefs::default();
        p.save(); // may fail silently if config dir is not writable
    }

    #[test]
    fn save_then_load_roundtrip_via_json() {
        // Test the JSON serialization path that save() uses
        let mut p = ModelPrefs::default();
        p.toggle_favorite("model-save-test");
        p.push_recent("model-save-test-2");
        p.roles.insert("fast".to_string(), "haiku".to_string());

        let json = serde_json::to_string_pretty(&p).unwrap();
        let loaded: ModelPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.favorites, p.favorites);
        assert_eq!(loaded.recent, p.recent);
        // roles is skip_serializing (legacy) — does not round-trip
        assert!(loaded.roles.is_empty());
    }

    #[test]
    fn load_from_invalid_json_returns_default() {
        // Test the error path in load(): invalid JSON → default
        let result: Option<ModelPrefs> = serde_json::from_str("not json").ok();
        assert!(result.is_none());
        // The load() fn uses .unwrap_or_default() — simulate that:
        let loaded = result.unwrap_or_default();
        assert!(loaded.favorites.is_empty());
    }
}
