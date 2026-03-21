//! Repository file and symbol index for improved code navigation.

use std::path::Path;

use anyhow::Context as AnyhowContext;
use regex::Regex;
use rusqlite::{Connection, params};
use walkdir::WalkDir;

/// A file entry in the index.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size_bytes: u64,
    pub ext: String,
}

/// A symbol entry in the index.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    pub path: String,
    pub name: String,
    /// One of: "fn", "async_fn", "struct", "enum", "impl", "trait", "const", "mod", "type"
    pub kind: String,
    pub line: u32,
}

/// SQLite-backed repository index for files and symbols.
pub struct RepoIndex {
    db: Connection,
}

impl RepoIndex {
    /// Open (or create) the index at the given path.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let db = Connection::open(db_path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL DEFAULT 0,
                 ext TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS symbols (
                 path TEXT NOT NULL,
                 name TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 line INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
             CREATE INDEX IF NOT EXISTS symbols_path ON symbols(path);
             CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
                 name, path,
                 content=symbols,
                 content_rowid=rowid
             );",
        )?;
        Ok(Self { db })
    }

    /// Walk `root`, index all files matching `extensions`, extract symbols.
    /// Returns the number of files indexed.
    pub fn build(&mut self, root: &Path, extensions: &[&str]) -> anyhow::Result<usize> {
        // Clear existing data
        self.db.execute_batch("DELETE FROM symbols; DELETE FROM files;")?;

        let symbol_re = build_symbol_regex();
        let mut count = 0usize;

        let tx = self.db.transaction()?;
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();
            if !extensions.is_empty() && !extensions.contains(&ext.as_str()) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            tx.execute(
                "INSERT OR REPLACE INTO files (path, size, ext) VALUES (?1, ?2, ?3)",
                params![rel_path, size as i64, ext],
            )?;

            // Extract symbols from file content
            if let Ok(contents) = std::fs::read_to_string(path) {
                let mut rowid_buf: Vec<(String, String, String, u32)> = Vec::new();
                for (line_idx, line) in contents.lines().enumerate() {
                    let line_no = (line_idx + 1) as u32;
                    for cap in symbol_re.captures_iter(line) {
                        let kind = cap.name("kind").map(|m| m.as_str()).unwrap_or("fn");
                        let name = cap
                            .name("name")
                            .map(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !name.is_empty() {
                            rowid_buf.push((
                                rel_path.clone(),
                                name,
                                kind.to_string(),
                                line_no,
                            ));
                        }
                    }
                }
                for (p, name, kind, line) in rowid_buf {
                    tx.execute(
                        "INSERT INTO symbols (path, name, kind, line) VALUES (?1, ?2, ?3, ?4)",
                        params![p, name, kind, line],
                    )?;
                }
            }
            count += 1;
        }
        // Rebuild FTS
        tx.execute_batch("INSERT INTO symbols_fts(symbols_fts) VALUES ('rebuild');")?;
        tx.commit()?;
        Ok(count)
    }

    /// Search files by path pattern (SQL LIKE).
    pub fn search_files(&self, pattern: &str) -> anyhow::Result<Vec<FileEntry>> {
        let like_pat = format!("%{}%", pattern);
        let mut stmt = self
            .db
            .prepare("SELECT path, size, ext FROM files WHERE path LIKE ?1 LIMIT 100")?;
        let rows = stmt.query_map(params![like_pat], |row| {
            Ok(FileEntry {
                path: row.get(0)?,
                size_bytes: row.get::<_, i64>(1)? as u64,
                ext: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("search_files query failed")
    }

    /// Search symbols by name (FTS5).
    pub fn search_symbols(&self, name: &str) -> anyhow::Result<Vec<SymbolEntry>> {
        // Try FTS first, fall back to LIKE
        let mut stmt = self.db.prepare(
            "SELECT s.path, s.name, s.kind, s.line
             FROM symbols_fts f
             JOIN symbols s ON s.rowid = f.rowid
             WHERE symbols_fts MATCH ?1
             LIMIT 50",
        )?;
        let rows = stmt.query_map(params![name], |row| {
            Ok(SymbolEntry {
                path: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: row.get::<_, i64>(3)? as u32,
            })
        })?;
        let results: Vec<SymbolEntry> = rows.collect::<Result<Vec<_>, _>>()?;
        if !results.is_empty() {
            return Ok(results);
        }
        // Fallback: LIKE
        let like_pat = format!("%{}%", name);
        let mut stmt2 = self
            .db
            .prepare("SELECT path, name, kind, line FROM symbols WHERE name LIKE ?1 LIMIT 50")?;
        let rows2 = stmt2.query_map(params![like_pat], |row| {
            Ok(SymbolEntry {
                path: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: row.get::<_, i64>(3)? as u32,
            })
        })?;
        rows2
            .collect::<Result<Vec<_>, _>>()
            .context("search_symbols fallback query failed")
    }

    /// Get all symbols for a specific file path.
    pub fn file_symbols(&self, path: &str) -> anyhow::Result<Vec<SymbolEntry>> {
        let mut stmt = self
            .db
            .prepare("SELECT path, name, kind, line FROM symbols WHERE path = ?1 ORDER BY line")?;
        let rows = stmt.query_map(params![path], |row| {
            Ok(SymbolEntry {
                path: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: row.get::<_, i64>(3)? as u32,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("file_symbols query failed")
    }

    /// Return (file_count, symbol_count).
    pub fn stats(&self) -> anyhow::Result<(usize, usize)> {
        let fc: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        let sc: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
        Ok((fc as usize, sc as usize))
    }
}

/// Build a combined regex for Rust/Python/JS symbol extraction.
fn build_symbol_regex() -> Regex {
    // Rust: fn, async fn, struct, enum, trait, impl, const, mod, type
    // Python: def, class
    // JS/TS: function, class, const/let/var = (arrow functions simplified)
    Regex::new(
        r"(?x)
        (?:pub\s+)?(?:async\s+)?
        (?P<kind>fn|struct|enum|trait|impl|const|mod|type|def|class|function)
        \s+
        (?P<name>[A-Za-z_][A-Za-z0-9_]*)
        ",
    )
    .expect("symbol regex is valid")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{tempdir, NamedTempFile};

    fn open_test_index() -> RepoIndex {
        let f = NamedTempFile::new().unwrap();
        RepoIndex::open(f.path()).unwrap()
    }

    #[test]
    fn empty_index_stats() {
        let idx = open_test_index();
        let (fc, sc) = idx.stats().unwrap();
        assert_eq!(fc, 0);
        assert_eq!(sc, 0);
    }

    #[test]
    fn build_indexes_rust_files() {
        let dir = tempdir().unwrap();
        // Create a simple Rust file
        fs::write(
            dir.path().join("main.rs"),
            "pub fn hello() {}\npub struct Foo {}\n",
        )
        .unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();
        let count = idx.build(dir.path(), &["rs"]).unwrap();
        assert_eq!(count, 1);

        let (fc, sc) = idx.stats().unwrap();
        assert_eq!(fc, 1);
        assert!(sc >= 2, "Expected at least 2 symbols, got {}", sc);
    }

    #[test]
    fn search_files_by_pattern() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("agent_loop.rs"), "fn run() {}").unwrap();
        fs::write(dir.path().join("lib.rs"), "fn init() {}").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();
        idx.build(dir.path(), &["rs"]).unwrap();

        let results = idx.search_files("agent").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.contains("agent_loop"));
    }

    #[test]
    fn search_symbols_finds_fn() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("foo.rs"),
            "pub fn my_function() {}\npub struct MyStruct {}\n",
        )
        .unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();
        idx.build(dir.path(), &["rs"]).unwrap();

        let results = idx.search_symbols("my_function").unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "my_function");
        assert_eq!(results[0].kind, "fn");
    }

    #[test]
    fn file_symbols_returns_correct_symbols() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("bar.rs"),
            "pub fn foo() {}\npub fn bar() {}\npub struct Baz {}\n",
        )
        .unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();
        idx.build(dir.path(), &["rs"]).unwrap();

        let symbols = idx.file_symbols("bar.rs").unwrap();
        assert!(symbols.len() >= 3);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
        assert!(names.contains(&"Baz"));
    }

    #[test]
    fn build_with_extensions_filter() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("code.rs"), "fn rust_fn() {}").unwrap();
        fs::write(dir.path().join("script.py"), "def python_fn(): pass").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();
        let count = idx.build(dir.path(), &["rs"]).unwrap();
        assert_eq!(count, 1); // Only Rust file
    }
}
