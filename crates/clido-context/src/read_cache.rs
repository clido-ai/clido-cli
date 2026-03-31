//! Bounded LRU cache for file reads.
//!
//! Avoids redundant disk I/O when the agent reads the same file multiple times
//! within a session. Uses least-recently-used eviction once the cache is full.
//!
//! The cache key is `(path, content_hash)` so stale entries are not returned
//! after a file has changed on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Maximum number of entries kept in the cache.
const MAX_ENTRIES: usize = 64;

/// Thread-safe LRU read cache shared across tool instances in a session.
#[derive(Clone, Default)]
pub struct ReadCache {
    inner: Arc<Mutex<CacheInner>>,
}

/// Internally uses a generation counter: each access bumps the entry's
/// generation, and eviction removes the entry with the lowest generation.
/// This avoids a linked-list and gives O(1) get/insert with O(n) eviction
/// (acceptable for n=64).
#[derive(Default)]
struct CacheInner {
    /// Maps (path, content_hash) → (content, generation).
    map: HashMap<(PathBuf, String), (String, u64)>,
    /// Monotonically increasing generation counter.
    gen: u64,
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
    /// Touching an entry marks it as most-recently-used.
    pub fn get(&self, path: &Path, content_hash: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let key = (path.to_path_buf(), content_hash.to_string());
        inner.gen += 1;
        let gen = inner.gen;
        if let Some(entry) = inner.map.get_mut(&key) {
            entry.1 = gen;
            Some(entry.0.clone())
        } else {
            None
        }
    }

    /// Insert a file's content into the cache.
    ///
    /// If the cache is full the least-recently-used entry is evicted first.
    pub fn insert(
        &self,
        path: PathBuf,
        content_hash: impl Into<String>,
        content: impl Into<String>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner.gen += 1;
        let gen = inner.gen;
        let content = content.into();
        let key = (path, content_hash.into());
        if let std::collections::hash_map::Entry::Occupied(mut e) = inner.map.entry(key.clone()) {
            e.get_mut().0 = content;
            e.get_mut().1 = gen;
            return;
        }
        // Evict LRU entry if at capacity.
        if inner.map.len() >= MAX_ENTRIES {
            if let Some(lru_key) = inner
                .map
                .iter()
                .min_by_key(|(_, (_, g))| *g)
                .map(|(k, _)| k.clone())
            {
                inner.map.remove(&lru_key);
            }
        }
        inner.map.insert(key, (content, gen));
    }

    /// Remove all entries for a given path (e.g. after a write).
    pub fn invalidate(&self, path: &PathBuf) {
        let mut inner = self.inner.lock().unwrap();
        inner.map.retain(|(p, _), _| p != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let cache = ReadCache::new();
        let path = PathBuf::from("/tmp/foo.rs");
        cache.insert(
            path.clone(),
            "abc123".to_string(),
            "content here".to_string(),
        );
        assert_eq!(cache.get(&path, "abc123"), Some("content here".to_string()));
        // Different hash → miss.
        assert_eq!(cache.get(&path, "deadbeef"), None);
    }

    #[test]
    fn evicts_lru_when_full() {
        let cache = ReadCache::new();
        // Insert MAX_ENTRIES entries.
        for i in 0..MAX_ENTRIES {
            let p = PathBuf::from(format!("/tmp/file{}.rs", i));
            cache.insert(p, format!("hash{}", i), format!("content{}", i));
        }
        // Access file0 so it becomes most-recently-used.
        let first = PathBuf::from("/tmp/file0.rs");
        assert!(cache.get(&first, "hash0").is_some());
        // Insert one more — should evict file1 (the LRU), not file0.
        let extra = PathBuf::from("/tmp/extra.rs");
        cache.insert(extra.clone(), "hx".to_string(), "cx".to_string());
        // file0 was recently accessed → still present.
        assert!(cache.get(&first, "hash0").is_some());
        // file1 was the LRU → evicted.
        let second = PathBuf::from("/tmp/file1.rs");
        assert_eq!(cache.get(&second, "hash1"), None);
        // extra is present.
        assert!(cache.get(&extra, "hx").is_some());
    }

    /// Re-inserting same key refreshes the value without eviction.
    #[test]
    fn insert_existing_key_refreshes_value() {
        let cache = ReadCache::new();
        let path = PathBuf::from("/tmp/refresh.rs");
        cache.insert(path.clone(), "hash1".to_string(), "old content".to_string());
        cache.insert(path.clone(), "hash1".to_string(), "new content".to_string());
        assert_eq!(cache.get(&path, "hash1"), Some("new content".to_string()));
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

    #[test]
    fn get_returns_none_for_never_inserted_key() {
        let cache = ReadCache::new();
        let path = PathBuf::from("/tmp/nonexistent.rs");
        assert_eq!(cache.get(&path, "anyhash"), None);
    }

    #[test]
    fn lru_eviction_removes_first_inserted() {
        let cache = ReadCache::new();
        // Insert exactly MAX_ENTRIES + 1 items without touching any of them
        // via get, so the first inserted (generation=1) is the LRU.
        for i in 0..=MAX_ENTRIES {
            let p = PathBuf::from(format!("/f/{}.rs", i));
            cache.insert(p, format!("h{}", i), format!("c{}", i));
        }
        // The very first entry should have been evicted.
        let first = PathBuf::from("/f/0.rs");
        assert_eq!(cache.get(&first, "h0"), None);
        // The last entry should still be present.
        let last = PathBuf::from(format!("/f/{}.rs", MAX_ENTRIES));
        assert_eq!(
            cache.get(&last, &format!("h{}", MAX_ENTRIES)),
            Some(format!("c{}", MAX_ENTRIES))
        );
    }

    #[test]
    fn invalidate_does_not_affect_other_paths() {
        let cache = ReadCache::new();
        let p1 = PathBuf::from("/a.rs");
        let p2 = PathBuf::from("/b.rs");
        cache.insert(p1.clone(), "h1", "c1");
        cache.insert(p2.clone(), "h2", "c2");
        cache.invalidate(&p1);
        assert_eq!(cache.get(&p1, "h1"), None);
        assert_eq!(cache.get(&p2, "h2"), Some("c2".to_string()));
    }
}
