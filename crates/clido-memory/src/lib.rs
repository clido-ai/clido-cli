//! Long-term memory: SQLite + FTS5 for keyword search, with optional embedding support.

pub mod shared;

use std::path::Path;

use rusqlite::{params, Connection};
use uuid::Uuid;

pub use shared::{CacheStats, FileContent, SearchResults, SharedMemory};

/// A stored memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    /// Set by search — relevance score from FTS5 BM25.
    pub relevance_score: Option<f64>,
}

/// SQLite-backed long-term memory store with FTS5 keyword search.
pub struct MemoryStore {
    db: Connection,
}

impl MemoryStore {
    /// Open (or create) the memory store at the given path.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let db = Connection::open(db_path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS memories (
                 id TEXT PRIMARY KEY,
                 content TEXT NOT NULL,
                 tags TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                 content, tags,
                 content=memories, content_rowid=rowid
             );
             CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                 INSERT INTO memories_fts(rowid, content, tags) VALUES (new.rowid, new.content, new.tags);
             END;
             CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                 INSERT INTO memories_fts(memories_fts, rowid, content, tags) VALUES ('delete', old.rowid, old.content, old.tags);
             END;
             CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                 INSERT INTO memories_fts(memories_fts, rowid, content, tags) VALUES ('delete', old.rowid, old.content, old.tags);
                 INSERT INTO memories_fts(rowid, content, tags) VALUES (new.rowid, new.content, new.tags);
             END;",
        )?;
        Ok(Self { db })
    }

    /// Insert a new memory. Returns the generated ID.
    pub fn insert(&mut self, content: &str, tags: &[&str]) -> anyhow::Result<String> {
        let id = Uuid::new_v4().to_string();
        let tags_json = serde_json::to_string(tags)?;
        let created_at = chrono::Utc::now().to_rfc3339();
        self.db.execute(
            "INSERT INTO memories (id, content, tags, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, content, tags_json, created_at],
        )?;
        Ok(id)
    }

    /// Keyword search using FTS5 BM25 ranking.
    pub fn search_keyword(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let mut stmt = self.db.prepare(
            "SELECT m.id, m.content, m.tags, m.created_at, bm25(memories_fts) as score
             FROM memories_fts
             JOIN memories m ON m.rowid = memories_fts.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            let tags_json: String = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                tags_json,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, content, tags_json, created_at, score) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            entries.push(MemoryEntry {
                id,
                content,
                tags,
                created_at,
                relevance_score: Some(score),
            });
        }
        Ok(entries)
    }

    /// Hybrid search — currently delegates to keyword search.
    /// TODO: integrate vector similarity when embedding engine is plugged in.
    pub fn search_hybrid(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        self.search_keyword(query, limit)
    }

    /// Delete a memory by ID.
    pub fn delete(&mut self, id: &str) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Prune old memories, keeping only the most recent `keep_recent`.
    /// Returns the number of deleted entries.
    pub fn prune_old(&mut self, keep_recent: usize) -> anyhow::Result<usize> {
        let deleted = self.db.execute(
            "DELETE FROM memories WHERE id NOT IN (
                 SELECT id FROM memories ORDER BY created_at DESC LIMIT ?1
             )",
            params![keep_recent as i64],
        )?;
        Ok(deleted)
    }

    /// List memories (most recent first).
    pub fn list(&self, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let mut stmt = self.db.prepare(
            "SELECT id, content, tags, created_at FROM memories ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let tags_json: String = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                tags_json,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, content, tags_json, created_at) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            entries.push(MemoryEntry {
                id,
                content,
                tags,
                created_at,
                relevance_score: None,
            });
        }
        Ok(entries)
    }

    /// Delete all memories.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.db.execute_batch(
            "DELETE FROM memories;
             INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');",
        )?;
        Ok(())
    }

    /// Count total memories.
    pub fn count(&self) -> anyhow::Result<usize> {
        let n: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(n as usize)
    }
}

// ---------------------------------------------------------------------------
// Embedding engine trait + TF-IDF stub
// ---------------------------------------------------------------------------

/// Trait for computing text embeddings.
pub trait EmbeddingEngine: Send + Sync {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn dimension(&self) -> usize;
}

/// Simple hash-based embedding stub (not semantic, but satisfies the trait).
/// TODO: swap TfIdfEmbeddingEngine with fastembed::TextEmbedding for production quality.
pub struct TfIdfEmbeddingEngine {
    dimension: usize,
}

impl TfIdfEmbeddingEngine {
    pub fn new() -> Self {
        Self { dimension: 128 }
    }
}

impl Default for TfIdfEmbeddingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingEngine for TfIdfEmbeddingEngine {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut result = Vec::new();
        for text in texts {
            let mut vec = vec![0.0f32; self.dimension];
            for word in text.split_whitespace() {
                let h = word
                    .bytes()
                    .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
                let idx = (h as usize) % self.dimension;
                vec[idx] += 1.0;
            }
            // L2 normalize
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut vec {
                    *x /= norm;
                }
            }
            result.push(vec);
        }
        Ok(result)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn open_test_store() -> MemoryStore {
        let f = NamedTempFile::new().unwrap();
        MemoryStore::open(f.path()).unwrap()
    }

    #[test]
    fn insert_and_count() {
        let mut store = open_test_store();
        assert_eq!(store.count().unwrap(), 0);
        store.insert("hello world", &["test"]).unwrap();
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn list_returns_inserted() {
        let mut store = open_test_store();
        let id = store.insert("my memory", &["tag1", "tag2"]).unwrap();
        let entries = store.list(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert!(entries[0].tags.contains(&"tag1".to_string()));
    }

    #[test]
    fn search_keyword_finds_match() {
        let mut store = open_test_store();
        store.insert("rust programming language", &[]).unwrap();
        store.insert("python scripting", &[]).unwrap();

        let results = store.search_keyword("rust", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("rust"));
    }

    #[test]
    fn search_keyword_no_match() {
        let mut store = open_test_store();
        store.insert("unrelated content here", &[]).unwrap();
        let results = store.search_keyword("quantum", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn delete_removes_entry() {
        let mut store = open_test_store();
        let id = store.insert("to be deleted", &[]).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        store.delete(&id).unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn prune_keeps_recent() {
        let mut store = open_test_store();
        for i in 0..5 {
            store.insert(&format!("memory {}", i), &[]).unwrap();
        }
        assert_eq!(store.count().unwrap(), 5);
        let deleted = store.prune_old(3).unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(store.count().unwrap(), 3);
    }

    #[test]
    fn reset_clears_all() {
        let mut store = open_test_store();
        store.insert("a", &[]).unwrap();
        store.insert("b", &[]).unwrap();
        store.reset().unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn search_hybrid_delegates_to_keyword() {
        let mut store = open_test_store();
        store.insert("the quick brown fox", &[]).unwrap();
        let r = store.search_hybrid("quick", 10).unwrap();
        assert!(!r.is_empty());
    }

    #[test]
    fn embedding_engine_produces_correct_dimension() {
        let engine = TfIdfEmbeddingEngine::new();
        let embeddings = engine.embed(&["hello world", "foo bar"]).unwrap();
        assert_eq!(embeddings.len(), 2);
        for emb in &embeddings {
            assert_eq!(emb.len(), 128);
        }
    }

    /// Lines 196-197: TfIdfEmbeddingEngine::default() calls new().
    #[test]
    fn tf_idf_default_is_same_as_new() {
        let engine = TfIdfEmbeddingEngine::default();
        assert_eq!(engine.dimension, 128);
    }

    /// Lines 225-226: dimension() returns the field value.
    #[test]
    fn tf_idf_dimension_returns_128() {
        let engine = TfIdfEmbeddingEngine::new();
        assert_eq!(engine.dimension(), 128);
    }

    #[test]
    fn irrelevant_memories_do_not_surface() {
        let mut store = open_test_store();
        store.insert("rust borrow checker lifetime", &[]).unwrap();
        store.insert("cooking pasta carbonara recipe", &[]).unwrap();
        store
            .insert("machine learning neural networks", &[])
            .unwrap();

        // Search for rust-related content
        let results = store.search_keyword("borrow", 10).unwrap();
        // Should only return the rust entry
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rust"));
        // Cooking and ML entries should NOT appear
        for r in &results {
            assert!(!r.content.contains("pasta"));
            assert!(!r.content.contains("neural"));
        }
    }
}
