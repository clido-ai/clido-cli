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
    /// Role → model ID overrides (e.g. "fast" → "claude-haiku-4-5-20251001").
    /// These override the `[roles]` section from config.toml.
    #[serde(default)]
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

    /// Resolve a role name to a model ID. Returns `None` if the role has no assignment.
    pub fn resolve_role<'a>(&'a self, role: &str) -> Option<&'a str> {
        self.roles.get(role).map(|s| s.as_str())
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
