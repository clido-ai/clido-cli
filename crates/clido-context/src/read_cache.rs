//! Simple bounded in-memory cache for file reads.
//!
//! Avoids redundant disk I/O when the agent reads the same file multiple times
//! within a session. Uses insertion-order eviction once the cache is full
//! (oldest entry dropped when capacity is reached).
//!
//! The cache key is `(path, content_hash)` so stale entries are not returned
//! after a file has changed on disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Maximum number of entries kept in the cache.
const MAX_ENTRIES: usize = 64;

/// Thread-safe read cache shared across tool instances in a session.
#[derive(Clone, Default)]
pub struct ReadCache {
    inner: Arc<Mutex<CacheInner>>,
}

#[derive(Default)]
struct CacheInner {
    /// Maps (path, content_hash) → content.
    map: HashMap<(PathBuf, String), String>,
    /// Insertion order (oldest first) so we know which entry to evict.
    order: Vec<(PathBuf, String)>,
}

impl ReadCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a cached file by path and content hash.
    ///
    /// Returns `Some(content)` only when the cached entry's hash matches
    /// the supplied `content_hash` (i.e. the file hasn't changed).
    pub fn get(&self, path: &PathBuf, content_hash: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.map.get(&(path.clone(), content_hash.to_string())).cloned()
    }

    /// Insert a file's content into the cache.
    ///
    /// If the cache is full the oldest entry is evicted first.
    pub fn insert(&self, path: PathBuf, content_hash: String, content: String) {
        let mut inner = self.inner.lock().unwrap();
        let key = (path.clone(), content_hash.clone());
        if inner.map.contains_key(&key) {
            // Already present — just refresh (no eviction needed).
            inner.map.insert(key, content);
            return;
        }
        // Evict oldest entry if at capacity.
        if inner.order.len() >= MAX_ENTRIES {
            let oldest = inner.order.remove(0);
            inner.map.remove(&oldest);
        }
        inner.order.push(key.clone());
        inner.map.insert(key, content);
    }

    /// Remove all entries for a given path (e.g. after a write).
    pub fn invalidate(&self, path: &PathBuf) {
        let mut inner = self.inner.lock().unwrap();
        let keys_to_remove: Vec<_> = inner
            .order
            .iter()
            .filter(|(p, _)| p == path)
            .cloned()
            .collect();
        for key in &keys_to_remove {
            inner.map.remove(key);
        }
        inner.order.retain(|(p, _)| p != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let cache = ReadCache::new();
        let path = PathBuf::from("/tmp/foo.rs");
        cache.insert(path.clone(), "abc123".to_string(), "content here".to_string());
        assert_eq!(cache.get(&path, "abc123"), Some("content here".to_string()));
        // Different hash → miss.
        assert_eq!(cache.get(&path, "deadbeef"), None);
    }

    #[test]
    fn evicts_when_full() {
        let cache = ReadCache::new();
        // Insert MAX_ENTRIES + 1 distinct paths.
        for i in 0..=MAX_ENTRIES {
            let p = PathBuf::from(format!("/tmp/file{}.rs", i));
            cache.insert(p, format!("hash{}", i), format!("content{}", i));
        }
        // The very first entry should be evicted.
        let first = PathBuf::from("/tmp/file0.rs");
        assert_eq!(cache.get(&first, "hash0"), None);
        // The last entry should still be present.
        let last = PathBuf::from(format!("/tmp/file{}.rs", MAX_ENTRIES));
        assert!(cache.get(&last, &format!("hash{}", MAX_ENTRIES)).is_some());
    }

    #[test]
    fn invalidate_removes_all_hashes_for_path() {
        let cache = ReadCache::new();
        let p = PathBuf::from("/tmp/bar.rs");
        cache.insert(p.clone(), "h1".to_string(), "v1".to_string());
        cache.insert(p.clone(), "h2".to_string(), "v2".to_string());
        cache.invalidate(&p);
        assert_eq!(cache.get(&p, "h1"), None);
        assert_eq!(cache.get(&p, "h2"), None);
    }
}
