//! Repository file and symbol index for improved code navigation.

use std::path::Path;

use anyhow::Context as AnyhowContext;
use glob::Pattern;
use ignore::WalkBuilder;
use regex::Regex;
use rusqlite::{params, Connection};

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

/// Statistics returned after a build.
#[derive(Debug, Clone, Default)]
pub struct BuildStats {
    /// Number of files that were indexed (matched extension filter and not excluded).
    pub indexed: u64,
    /// Number of file entries skipped due to ignore rules or exclude patterns.
    pub skipped: u64,
}

/// Options for `RepoIndex::build`.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// File extensions to include (empty = all extensions).
    pub extensions: Vec<String>,
    /// Glob patterns to exclude (e.g. `["*.lock", "vendor/**"]`).
    pub exclude_patterns: Vec<String>,
    /// When `true`, `.gitignore`, global git ignore, and `.clido-ignore` are all bypassed.
    pub include_ignored: bool,
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
    ///
    /// This is the legacy convenience wrapper; prefer `build_with_options` for full control.
    pub fn build(&mut self, root: &Path, extensions: &[&str]) -> anyhow::Result<usize> {
        let opts = BuildOptions {
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let stats = self.build_with_options(root, &opts)?;
        Ok(stats.indexed as usize)
    }

    /// Walk `root` and index files according to `opts`. Returns `BuildStats`.
    pub fn build_with_options(
        &mut self,
        root: &Path,
        opts: &BuildOptions,
    ) -> anyhow::Result<BuildStats> {
        // Clear existing data
        self.db
            .execute_batch("DELETE FROM symbols; DELETE FROM files;")?;

        // Pre-compile glob exclude patterns for performance.
        let compiled_excludes: Vec<Pattern> = opts
            .exclude_patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        let symbol_re = build_symbol_regex();
        let mut stats = BuildStats::default();

        // Build the walker. Always show hidden files so dot-directories are traversed,
        // but apply gitignore rules unless `include_ignored` is set.
        let mut builder = WalkBuilder::new(root);
        builder.hidden(false);

        if opts.include_ignored {
            builder
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false);
        } else {
            builder
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .add_custom_ignore_filename(".clido-ignore");
        }

        let tx = self.db.transaction()?;
        for entry in builder.build().filter_map(|e| e.ok()) {
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            // Extension filter
            if !opts.extensions.is_empty() && !opts.extensions.iter().any(|e| e == &ext) {
                stats.skipped += 1;
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            // Exclude pattern filter
            if compiled_excludes
                .iter()
                .any(|p| p.matches(&rel_path) || p.matches(path.to_string_lossy().as_ref()))
            {
                stats.skipped += 1;
                continue;
            }

            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

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
                            rowid_buf.push((rel_path.clone(), name, kind.to_string(), line_no));
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
            stats.indexed += 1;
        }
        // Rebuild FTS
        tx.execute_batch("INSERT INTO symbols_fts(symbols_fts) VALUES ('rebuild');")?;
        tx.commit()?;
        Ok(stats)
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

    // -----------------------------------------------------------------------
    // gitignore-aware tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_skips_gitignored_files() {
        let dir = tempdir().unwrap();

        // Initialize a git repository so that .gitignore is respected by the walker.
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create a .gitignore that ignores the target/ directory
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();

        // Create a file that should be indexed
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        // Create a file inside target/ that should be skipped
        let target_dir = dir.path().join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("output.rs"), "fn generated() {}").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();

        let opts = BuildOptions {
            extensions: vec!["rs".to_string()],
            include_ignored: false,
            ..Default::default()
        };
        let stats = idx.build_with_options(dir.path(), &opts).unwrap();

        // Only main.rs should be indexed; target/output.rs should be skipped by gitignore
        assert_eq!(stats.indexed, 1, "Only main.rs should be indexed");
        let results = idx.search_files("output").unwrap();
        assert!(
            results.is_empty(),
            "target/output.rs should not be in index"
        );
    }

    #[test]
    fn test_build_respects_clido_ignore() {
        let dir = tempdir().unwrap();

        // Create a .clido-ignore that ignores *.snap files
        fs::write(dir.path().join(".clido-ignore"), "*.snap\n").unwrap();

        // Create files
        fs::write(dir.path().join("lib.rs"), "fn lib_fn() {}").unwrap();
        fs::write(dir.path().join("snapshot.snap"), "snapshot data").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();

        // Index both rs and snap extensions so the extension filter doesn't hide the snap file
        let opts = BuildOptions {
            extensions: vec!["rs".to_string(), "snap".to_string()],
            include_ignored: false,
            ..Default::default()
        };
        let stats = idx.build_with_options(dir.path(), &opts).unwrap();

        // snapshot.snap should be skipped by .clido-ignore
        assert_eq!(stats.indexed, 1, "Only lib.rs should be indexed");
        let snap_results = idx.search_files("snapshot").unwrap();
        assert!(
            snap_results.is_empty(),
            "snapshot.snap should be excluded by .clido-ignore"
        );
    }

    #[test]
    fn test_include_ignored_bypasses_gitignore() {
        let dir = tempdir().unwrap();

        // Initialize a git repository so that .gitignore would normally be respected.
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create a .gitignore that ignores target/
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();

        // Create main.rs and a target/ file
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        let target_dir = dir.path().join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("output.rs"), "fn generated() {}").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();

        let opts = BuildOptions {
            extensions: vec!["rs".to_string()],
            include_ignored: true,
            ..Default::default()
        };
        let stats = idx.build_with_options(dir.path(), &opts).unwrap();

        // Both files should be indexed since gitignore is bypassed
        assert_eq!(
            stats.indexed, 2,
            "Both files should be indexed with include_ignored=true"
        );
        let results = idx.search_files("output").unwrap();
        assert_eq!(
            results.len(),
            1,
            "target/output.rs should be in index when include_ignored=true"
        );
    }

    #[test]
    fn test_exclude_patterns_applied() {
        let dir = tempdir().unwrap();

        // Create various files
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("Cargo.lock"), "# lockfile content").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut idx = RepoIndex::open(db_file.path()).unwrap();

        let opts = BuildOptions {
            // No extension filter — index everything
            extensions: vec![],
            exclude_patterns: vec!["*.lock".to_string()],
            include_ignored: false,
        };
        let stats = idx.build_with_options(dir.path(), &opts).unwrap();

        // Cargo.lock should be excluded; main.rs and Cargo.toml should be indexed
        let lock_results = idx.search_files("Cargo.lock").unwrap();
        assert!(
            lock_results.is_empty(),
            "Cargo.lock should be excluded by exclude_patterns"
        );
        assert!(
            stats.indexed >= 2,
            "main.rs and Cargo.toml should be indexed"
        );
    }
}
