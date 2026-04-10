//! Shared memory for multi-agent exploration.
//!
//! Provides a thread-safe cache for file contents and search results
//! to prevent duplicate reads across multiple sub-agents.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Content of a file cached for sharing between agents.
#[derive(Clone, Debug)]
pub struct FileContent {
    pub content: String,
    pub cached_at: std::time::Instant,
}

/// Search results cached for sharing between agents.
#[derive(Clone, Debug)]
pub struct SearchResults {
    pub results: Vec<String>,
    pub cached_at: std::time::Instant,
}

/// Shared memory for multi-agent exploration.
///
/// This struct provides thread-safe caching of file contents and search results
/// to prevent duplicate work when multiple sub-agents explore the same codebase.
#[derive(Clone, Debug)]
pub struct SharedMemory {
    /// Cache of file contents keyed by canonical path.
    file_cache: Arc<RwLock<HashMap<PathBuf, FileContent>>>,
    /// Cache of search results keyed by query string.
    search_cache: Arc<RwLock<HashMap<String, SearchResults>>>,
    /// Maximum cache size (LRU eviction when exceeded).
    max_files: usize,
    /// Cache TTL for file contents.
    file_ttl: std::time::Duration,
    /// Cache TTL for search results.
    search_ttl: std::time::Duration,
}

impl SharedMemory {
    /// Create a new shared memory instance with default settings.
    pub fn new() -> Self {
        Self::with_config(1000, std::time::Duration::from_secs(300), std::time::Duration::from_secs(60))
    }

    /// Create a new shared memory instance with custom configuration.
    ///
    /// # Arguments
    /// * `max_files` - Maximum number of files to cache (LRU eviction)
    /// * `file_ttl` - Time-to-live for cached file contents
    /// * `search_ttl` - Time-to-live for cached search results
    pub fn with_config(
        max_files: usize,
        file_ttl: std::time::Duration,
        search_ttl: std::time::Duration,
    ) -> Self {
        Self {
            file_cache: Arc::new(RwLock::new(HashMap::with_capacity(max_files))),
            search_cache: Arc::new(RwLock::new(HashMap::new())),
            max_files,
            file_ttl,
            search_ttl,
        }
    }

    /// Get a file from the cache if it exists and hasn't expired.
    ///
    /// Returns `Some(FileContent)` if found and not expired, `None` otherwise.
    pub fn get_file(&self, path: &PathBuf) -> Option<FileContent> {
        let cache = self.file_cache.read().ok()?;
        let entry = cache.get(path)?;

        // Check if expired
        if entry.cached_at.elapsed() > self.file_ttl {
            drop(cache);
            // Try to remove expired entry
            if let Ok(mut cache) = self.file_cache.write() {
                cache.remove(path);
            }
            return None;
        }

        Some(entry.clone())
    }

    /// Cache a file content for sharing with other agents.
    ///
    /// If the cache is at capacity, evicts the oldest entry (LRU).
    pub fn cache_file(&self, path: PathBuf, content: String) {
        let mut cache = match self.file_cache.write() {
            Ok(cache) => cache,
            Err(_) => return, // Poisoned lock, skip caching
        };

        // Evict oldest if at capacity
        if cache.len() >= self.max_files {
            let oldest = cache
                .iter()
                .min_by_key(|(_, v)| v.cached_at)
                .map(|(k, _)| k.clone());
            if let Some(oldest_key) = oldest {
                cache.remove(&oldest_key);
            }
        }

        cache.insert(
            path,
            FileContent {
                content,
                cached_at: std::time::Instant::now(),
            },
        );
    }

    /// Get search results from the cache if they exist and haven't expired.
    pub fn get_search(&self, query: &str) -> Option<SearchResults> {
        let cache = self.search_cache.read().ok()?;
        let entry = cache.get(query)?;

        // Check if expired
        if entry.cached_at.elapsed() > self.search_ttl {
            drop(cache);
            if let Ok(mut cache) = self.search_cache.write() {
                cache.remove(query);
            }
            return None;
        }

        Some(entry.clone())
    }

    /// Cache search results for sharing with other agents.
    pub fn cache_search(&self, query: String, results: Vec<String>) {
        let mut cache = match self.search_cache.write() {
            Ok(cache) => cache,
            Err(_) => return,
        };

        cache.insert(
            query,
            SearchResults {
                results,
                cached_at: std::time::Instant::now(),
            },
        );
    }

    /// Clear all cached data.
    pub fn clear(&self) {
        if let Ok(mut cache) = self.file_cache.write() {
            cache.clear();
        }
        if let Ok(mut cache) = self.search_cache.write() {
            cache.clear();
        }
    }

    /// Get cache statistics for debugging/monitoring.
    pub fn stats(&self) -> CacheStats {
        let file_count = self.file_cache.read().map(|c| c.len()).unwrap_or(0);
        let search_count = self.search_cache.read().map(|c| c.len()).unwrap_or(0);
        CacheStats {
            file_count,
            search_count,
            max_files: self.max_files,
        }
    }
}

impl Default for SharedMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// Cache statistics for monitoring.
#[derive(Clone, Copy, Debug)]
pub struct CacheStats {
    pub file_count: usize,
    pub search_count: usize,
    pub max_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_file_cache_basic() {
        let memory = SharedMemory::new();
        let path = PathBuf::from("/test/file.rs");

        // Initially empty
        assert!(memory.get_file(&path).is_none());

        // Cache a file
        memory.cache_file(path.clone(), "content".to_string());

        // Should be retrievable
        let cached = memory.get_file(&path).unwrap();
        assert_eq!(cached.content, "content");
    }

    #[test]
    fn test_file_cache_ttl_expiration() {
        let memory = SharedMemory::with_config(100, std::time::Duration::from_millis(10), std::time::Duration::from_secs(60));
        let path = PathBuf::from("/test/file.rs");

        memory.cache_file(path.clone(), "content".to_string());
        assert!(memory.get_file(&path).is_some());

        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Should be expired
        assert!(memory.get_file(&path).is_none());
    }

    #[test]
    fn test_file_cache_lru_eviction() {
        let memory = SharedMemory::with_config(2, std::time::Duration::from_secs(300), std::time::Duration::from_secs(60));

        let path1 = PathBuf::from("/test/file1.rs");
        let path2 = PathBuf::from("/test/file2.rs");
        let path3 = PathBuf::from("/test/file3.rs");

        memory.cache_file(path1.clone(), "content1".to_string());
        memory.cache_file(path2.clone(), "content2".to_string());
        memory.cache_file(path3.clone(), "content3".to_string());

        // One of the first two should be evicted
        let count = [memory.get_file(&path1).is_some(), memory.get_file(&path2).is_some(), memory.get_file(&path3).is_some()]
            .iter()
            .filter(|&&x| x)
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_concurrent_access() {
        let memory = SharedMemory::new();
        let path = PathBuf::from("/test/file.rs");

        // Spawn multiple threads to cache and read
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let memory = memory.clone();
                let path = path.clone();
                thread::spawn(move || {
                    memory.cache_file(path.clone(), format!("content{}", i));
                    memory.get_file(&path)
                })
            })
            .collect();

        // All should complete without panicking
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_search_cache() {
        let memory = SharedMemory::new();
        let query = "test query";

        assert!(memory.get_search(query).is_none());

        memory.cache_search(query.to_string(), vec!["result1".to_string(), "result2".to_string()]);

        let cached = memory.get_search(query).unwrap();
        assert_eq!(cached.results.len(), 2);
    }

    #[test]
    fn test_stats() {
        let memory = SharedMemory::new();
        let path = PathBuf::from("/test/file.rs");

        memory.cache_file(path, "content".to_string());
        memory.cache_search("query".to_string(), vec![]);

        let stats = memory.stats();
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.search_count, 1);
    }
}
