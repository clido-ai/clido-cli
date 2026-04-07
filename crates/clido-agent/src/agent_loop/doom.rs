//! Doom-loop detection: repeated identical failures or repeated same-args errors.

use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

#[derive(Clone)]
struct DoomEntry {
    tool: String,
    fingerprint: String,
    args_hash: u64,
}

/// Sliding-window tracker for stuck tool loops.
pub(crate) struct DoomTracker {
    q: VecDeque<DoomEntry>,
    window: usize,
}

impl DoomTracker {
    pub(crate) fn new(window: usize) -> Self {
        Self {
            q: VecDeque::new(),
            window: window.max(3),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.q.clear();
    }

    /// Returns `(tool, error_snippet)` when doom should abort the turn.
    pub(crate) fn record_failure(
        &mut self,
        tool: &str,
        error_msg: &str,
        input: &Value,
        consecutive_k: usize,
        same_args_min: usize,
    ) -> Option<(String, String)> {
        let fp = normalize_error(error_msg);
        let args_hash = hash_args(input);
        self.q.push_back(DoomEntry {
            tool: tool.to_string(),
            fingerprint: fp.clone(),
            args_hash,
        });
        while self.q.len() > self.window {
            self.q.pop_front();
        }

        if consecutive_k > 0 && self.q.len() >= consecutive_k {
            let last_k: Vec<&DoomEntry> = self.q.iter().rev().take(consecutive_k).collect();
            if last_k.iter().all(|e| {
                e.tool == last_k[0].tool
                    && e.fingerprint == last_k[0].fingerprint
                    && !e.fingerprint.is_empty()
            }) {
                return Some((
                    tool.to_string(),
                    error_msg.chars().take(200).collect::<String>(),
                ));
            }
        }

        let cnt = self
            .q
            .iter()
            .filter(|e| e.tool == tool && e.args_hash == args_hash)
            .count();
        if cnt >= same_args_min {
            return Some((
                tool.to_string(),
                error_msg.chars().take(200).collect::<String>(),
            ));
        }
        None
    }
}

fn hash_args(v: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(v).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn normalize_error(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clear_resets_failure_window() {
        let mut t = DoomTracker::new(10);
        let v = json!({});
        assert!(t.record_failure("Read", "e1", &v, 3, 99).is_none());
        assert!(t.record_failure("Read", "e1", &v, 3, 99).is_none());
        t.clear();
        assert!(t.record_failure("Read", "e1", &v, 3, 99).is_none());
    }

    #[test]
    fn consecutive_identical_errors_trigger() {
        let mut t = DoomTracker::new(10);
        let v = json!({"a": 1});
        assert!(t.record_failure("Read", "same err", &v, 3, 99).is_none());
        assert!(t.record_failure("Read", "same err", &v, 3, 99).is_none());
        let r = t.record_failure("Read", "same err", &v, 3, 99);
        assert!(r.is_some());
    }

    #[test]
    fn same_args_count_triggers_before_window_full() {
        let mut t = DoomTracker::new(20);
        let v = json!({"x": "y"});
        assert!(t.record_failure("Edit", "oops", &v, 0, 2).is_none());
        let r = t.record_failure("Edit", "different msg", &v, 0, 2);
        assert!(r.is_some());
    }
}
