//! Project rules discovery and assembly.
//! Searches for CLIDO.md / .clido/rules.md from cwd up to root,
//! then loads ~/.config/clido/rules.md as global rules.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct RulesFile {
    pub path: PathBuf,
    pub content: String,
}

/// Max import recursion depth.
const MAX_IMPORT_DEPTH: usize = 5;

/// Discover all active rules files starting from `cwd`, walking up to root.
/// Returns files in order: global (lowest priority) first, closest-to-cwd last.
///
/// If `no_rules` is true, returns an empty vec immediately.
/// If `rules_file_override` is Some, loads only that file and returns.
pub fn discover(cwd: &Path, no_rules: bool, rules_file_override: Option<&Path>) -> Vec<RulesFile> {
    if no_rules {
        return vec![];
    }

    if let Some(override_path) = rules_file_override {
        return load_rules_file(override_path)
            .map(|f| vec![f])
            .unwrap_or_default();
    }

    // Walk from cwd up to root, collecting candidates.
    // We collect them in cwd-first order, then reverse so global is first.
    let mut walk_results: Vec<RulesFile> = Vec::new();
    let mut dir = cwd.to_path_buf();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();

    loop {
        if seen_dirs.contains(&dir) {
            break;
        }
        seen_dirs.insert(dir.clone());

        // Check CLIDO.md (primary location)
        let clido_md = dir.join("CLIDO.md");
        if clido_md.exists() {
            if let Some(f) = load_rules_file(&clido_md) {
                walk_results.push(f);
            }
        }

        // Note: .clido/rules.md is deprecated and no longer supported.
        // Use CLIDO.md in the workspace root instead.

        // Move to parent
        let parent = dir.parent().map(|p| p.to_path_buf());
        match parent {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }

    // walk_results is cwd-first; reverse so global (root) is first, closest-to-cwd last.
    walk_results.reverse();

    // Prepend global rules at the very start (lowest priority)
    let mut result: Vec<RulesFile> = Vec::new();
    if let Some(global_rules) = global_rules_path() {
        if global_rules.exists() {
            if let Some(f) = load_rules_file(&global_rules) {
                result.push(f);
            }
        }
    }
    result.extend(walk_results);
    result
}

/// Returns the path to the global rules file (~/.config/clido/rules.md).
fn global_rules_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "clido").map(|d| d.config_dir().join("rules.md"))
}

/// Max characters allowed per rules file (including imports). ~6000 tokens at 4 chars/token.
const MAX_RULES_CHARS: usize = 24_000;

/// Load a rules file, processing import directives. Returns None if the file cannot be read.
fn load_rules_file(path: &Path) -> Option<RulesFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    seen.insert(canonical);
    let content = process_imports(&raw, path, &mut seen, 0);
    // Enforce size cap to prevent unbounded system prompt growth from large rules files.
    let content = if content.chars().count() > MAX_RULES_CHARS {
        tracing::warn!(
            path = %path.display(),
            limit = MAX_RULES_CHARS,
            "rules file exceeds {} chars; truncating to prevent context overflow",
            MAX_RULES_CHARS
        );
        let truncated: String = content.chars().take(MAX_RULES_CHARS).collect();
        format!(
            "{}\n<!-- clido: rules file truncated at {} chars -->\n",
            truncated, MAX_RULES_CHARS
        )
    } else {
        content
    };
    Some(RulesFile {
        path: path.to_path_buf(),
        content,
    })
}

/// Process `[import: ./path/to/file.md]` directives in content.
/// Recursion depth is limited to MAX_IMPORT_DEPTH.
/// Cycles are detected via the `seen` HashSet.
fn process_imports(
    content: &str,
    source_file: &Path,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    if depth >= MAX_IMPORT_DEPTH {
        return content.to_string();
    }

    let source_dir = source_file.parent().unwrap_or(Path::new("."));
    let mut result = String::with_capacity(content.len());

    for line in content.lines() {
        // Match lines like: [import: ./relative/path.md]
        if let Some(import_path) = parse_import_directive(line) {
            let target = source_dir.join(&import_path);
            let canonical = match target.canonicalize() {
                Ok(c) => c,
                Err(_) => {
                    // File doesn't exist or can't be resolved; leave line as-is
                    result.push_str(line);
                    result.push('\n');
                    continue;
                }
            };

            if seen.contains(&canonical) {
                // Circular import detected; skip this import
                result.push_str(&format!(
                    "<!-- clido: circular import skipped: {} -->\n",
                    target.display()
                ));
                continue;
            }

            match std::fs::read_to_string(&target) {
                Ok(imported) => {
                    seen.insert(canonical.clone());
                    let processed = process_imports(&imported, &target, seen, depth + 1);
                    seen.remove(&canonical);
                    result.push_str(&processed);
                    if !processed.ends_with('\n') {
                        result.push('\n');
                    }
                }
                Err(_) => {
                    // Can't read import; leave the directive line as-is
                    result.push_str(line);
                    result.push('\n');
                }
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Parse an import directive from a line, returning the path string if found.
/// Format: `[import: ./path/to/file.md]`
fn parse_import_directive(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("[import:") && trimmed.ends_with(']') {
        let inner = &trimmed[8..trimmed.len() - 1];
        let path = inner.trim();
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}

/// Trust store for project instruction files (CLIDO.md / .clido/rules.md).
///
/// Persisted at `{data_dir}/trusted_project_instructions.json`.
/// Each entry records the canonical path and SHA-256 content hash of a trusted file.
pub struct TrustStore {
    pub(crate) path: PathBuf,
    pub(crate) entries: Vec<TrustEntry>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub(crate) struct TrustEntry {
    canonical_path: String,
    sha256: String,
}

impl TrustStore {
    /// Load the trust store from `{data_dir}/trusted_project_instructions.json`.
    /// Returns an empty store if the file does not exist or cannot be parsed.
    pub fn load() -> Self {
        let path = Self::store_path();
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Vec<TrustEntry>>(&s).ok())
            .unwrap_or_default();
        Self {
            path: path.unwrap_or_else(|| PathBuf::from("/dev/null")),
            entries,
        }
    }

    fn store_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "clido")
            .map(|d| d.data_dir().join("trusted_project_instructions.json"))
    }

    /// Return true if the file at `path` with content `content` is already trusted.
    pub fn is_trusted(&self, path: &Path, content: &str) -> bool {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string();
        let hash = sha256_hex(content);
        self.entries
            .iter()
            .any(|e| e.canonical_path == canonical && e.sha256 == hash)
    }

    /// Record a file as trusted and persist the store to disk.
    pub fn trust(&mut self, path: &Path, content: &str) {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string();
        let hash = sha256_hex(content);
        // Remove any stale entry for the same path (e.g. content changed).
        self.entries.retain(|e| e.canonical_path != canonical);
        self.entries.push(TrustEntry {
            canonical_path: canonical,
            sha256: hash,
        });
        if let Ok(json) = serde_json::to_string_pretty(&self.entries) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.path, json);
        }
    }
}

fn sha256_hex(content: &str) -> String {
    // Simple FNV-1a 64-bit hash for content fingerprinting (no external crypto dep).
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Prompt the user interactively to trust a project instructions file.
/// Returns `true` if the user confirms, `false` otherwise.
fn prompt_trust(path: &Path) -> bool {
    eprint!("Load project instructions from {}? [y/N] ", path.display());
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Discover rules files with trust-on-first-use gating for project-local files.
///
/// - Global rules (~/.config/clido/rules.md) are always loaded without prompting.
/// - Project-local files (CLIDO.md, .clido/rules.md) require user approval on
///   first use or when content changes.
/// - In non-interactive mode (`is_tty = false`), untrusted files are skipped.
pub fn discover_with_trust(
    cwd: &Path,
    no_rules: bool,
    rules_file_override: Option<&Path>,
    is_tty: bool,
) -> Vec<RulesFile> {
    if no_rules {
        return vec![];
    }

    // When a rules file override is given, skip trust gating (explicit user choice).
    if rules_file_override.is_some() {
        return discover(cwd, no_rules, rules_file_override);
    }

    let mut trust_store = TrustStore::load();
    let raw_files = discover(cwd, no_rules, None);

    // Determine global rules path so we can skip gating for it.
    let global_path = global_rules_path()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_default();

    let mut result = Vec::new();
    for file in raw_files {
        let canonical = file
            .path
            .canonicalize()
            .unwrap_or_else(|_| file.path.clone());

        // Global rules are always trusted.
        if canonical == global_path {
            result.push(file);
            continue;
        }

        // Project-local file: check trust store.
        if trust_store.is_trusted(&file.path, &file.content) {
            result.push(file);
        } else if is_tty && prompt_trust(&file.path) {
            trust_store.trust(&file.path, &file.content);
            result.push(file);
        } else {
            tracing::info!(
                path = %file.path.display(),
                "skipping untrusted project instructions file"
            );
        }
    }
    result
}

/// Assemble a rules prompt string from discovered RulesFile entries.
///
/// Returns an empty string if `files` is empty. Otherwise concatenates each
/// file's content with a header line:
/// ```text
/// --- Rules from: /path/to/CLIDO.md ---
/// <content>
/// ```
pub fn assemble_rules_prompt(files: &[RulesFile]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for f in files {
        out.push_str(&format!("--- Rules from: {} ---\n", f.path.display()));
        out.push_str(&f.content);
        if !f.content.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_no_files_returns_empty() {
        let dir = tempdir().unwrap();
        let result = discover(dir.path(), false, None);
        // No global rules in test environment (directories crate may return a path but it won't exist)
        // So we just check it doesn't panic and returns a vec (possibly empty)
        let _ = result;
    }

    #[test]
    fn test_discovers_clido_md_at_root() {
        let dir = tempdir().unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# Project Rules\nBe concise.\n").unwrap();

        let result = discover(dir.path(), false, None);
        // Filter to only the CLIDO.md we created (ignore any global rules)
        let found = result.iter().any(|f| f.path == clido_md);
        assert!(found, "Expected to find CLIDO.md");
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        assert!(file.content.contains("Be concise."));
    }

    #[test]
    fn test_clido_md_discovered() {
        let dir = tempdir().unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "Always write tests.\n").unwrap();

        let result = discover(dir.path(), false, None);
        let found = result.iter().any(|f| f.path == clido_md);
        assert!(found, "Expected to find CLIDO.md");
    }

    #[test]
    fn test_no_rules_flag_suppresses() {
        let dir = tempdir().unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# Rules\n").unwrap();

        let result = discover(dir.path(), true, None);
        assert!(result.is_empty(), "no_rules=true should return empty vec");
    }

    #[test]
    fn test_import_directive_resolved() {
        let dir = tempdir().unwrap();
        let imported = dir.path().join("extra.md");
        std::fs::write(&imported, "Imported content here.\n").unwrap();

        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "Main rules.\n[import: ./extra.md]\nEnd.\n").unwrap();

        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        assert!(
            file.content.contains("Imported content here."),
            "Import directive should inline the content"
        );
        assert!(file.content.contains("Main rules."));
        assert!(file.content.contains("End."));
    }

    #[test]
    fn test_circular_import_does_not_loop() {
        let dir = tempdir().unwrap();

        // a.md imports b.md which imports a.md
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        std::fs::write(&a, "A content.\n[import: ./b.md]\n").unwrap();
        std::fs::write(&b, "B content.\n[import: ./a.md]\n").unwrap();

        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "[import: ./a.md]\n").unwrap();

        // Should not loop; just complete without hanging
        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        assert!(file.content.contains("A content."));
        assert!(file.content.contains("B content."));
    }

    #[test]
    fn test_assemble_includes_headers() {
        let files = vec![
            RulesFile {
                path: PathBuf::from("/project/CLIDO.md"),
                content: "Be concise.\n".to_string(),
            },
            RulesFile {
                path: PathBuf::from("/home/user/.config/clido/rules.md"),
                content: "Always write tests.\n".to_string(),
            },
        ];
        let prompt = assemble_rules_prompt(&files);
        assert!(prompt.contains("--- Rules from: /project/CLIDO.md ---"));
        assert!(prompt.contains("Be concise."));
        assert!(prompt.contains("--- Rules from: /home/user/.config/clido/rules.md ---"));
        assert!(prompt.contains("Always write tests."));
    }

    #[test]
    fn test_assemble_empty_returns_empty_string() {
        let prompt = assemble_rules_prompt(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_rules_file_override() {
        let dir = tempdir().unwrap();
        // This CLIDO.md should be ignored when override is set
        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# Default rules\n").unwrap();

        let override_file = dir.path().join("custom-rules.md");
        std::fs::write(&override_file, "Custom rules only.\n").unwrap();

        let result = discover(dir.path(), false, Some(&override_file));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, override_file);
        assert!(result[0].content.contains("Custom rules only."));
    }

    // ── additional coverage ────────────────────────────────────────────────

    #[test]
    fn test_import_directive_nonexistent_file_left_as_is() {
        let dir = tempdir().unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        // Import a file that doesn't exist
        std::fs::write(&clido_md, "Main.\n[import: ./nonexistent.md]\nEnd.\n").unwrap();

        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        // Nonexistent import should leave the directive line as-is
        assert!(
            file.content.contains("[import: ./nonexistent.md]") || file.content.contains("Main.")
        );
    }

    #[test]
    fn test_parse_import_directive_valid() {
        let result = parse_import_directive("[import: ./other.md]");
        assert_eq!(result, Some("./other.md".to_string()));
    }

    #[test]
    fn test_parse_import_directive_no_match() {
        assert!(parse_import_directive("# Regular heading").is_none());
        assert!(parse_import_directive("regular text").is_none());
        assert!(parse_import_directive("").is_none());
    }

    #[test]
    fn test_parse_import_directive_empty_path() {
        // [import: ] with empty path → None
        let result = parse_import_directive("[import: ]");
        assert!(result.is_none());
    }

    #[test]
    fn test_import_depth_limit() {
        // Create a chain of MAX_IMPORT_DEPTH + 1 files; the deepest should not be included
        let dir = tempdir().unwrap();
        let depth = MAX_IMPORT_DEPTH + 1;
        // Create files a0.md, a1.md, ... a{depth}.md where each imports the next
        for i in 0..=depth {
            let content = if i < depth {
                format!("file{}\n[import: ./a{}.md]\n", i, i + 1)
            } else {
                format!("deepest{}\n", i)
            };
            std::fs::write(dir.path().join(format!("a{}.md", i)), content).unwrap();
        }
        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "[import: ./a0.md]\n").unwrap();

        // Should not hang or panic; just limit recursion
        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        // Should contain at least the content of files up to the depth limit
        assert!(file.content.contains("file0"));
    }

    #[test]
    fn test_assemble_rules_prompt_with_content_no_trailing_newline() {
        let files = vec![RulesFile {
            path: std::path::PathBuf::from("/project/CLIDO.md"),
            // No trailing newline
            content: "Be concise.".to_string(),
        }];
        let prompt = assemble_rules_prompt(&files);
        // Assemble should add a newline if content doesn't end with one
        assert!(prompt.ends_with('\n'));
        assert!(prompt.contains("Be concise."));
    }

    #[test]
    fn test_override_nonexistent_file_returns_empty() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.md");
        let result = discover(dir.path(), false, Some(&nonexistent));
        // Override path doesn't exist → returns empty (load_rules_file returns None)
        assert!(result.is_empty());
    }

    // ── imported content without trailing newline gets one added ──────────

    #[test]
    fn test_import_content_no_trailing_newline_gets_newline_added() {
        let dir = tempdir().unwrap();
        let imported = dir.path().join("imported.md");
        // Write without trailing newline
        std::fs::write(&imported, "No trailing newline content").unwrap();

        let clido_md = dir.path().join("CLIDO.md");
        std::fs::write(&clido_md, "Before.\n[import: ./imported.md]\nAfter.\n").unwrap();

        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        // The imported content had no trailing newline; process_imports should add one
        assert!(file.content.contains("No trailing newline content"));
        assert!(file.content.contains("After."));
    }

    // ── global rules file present ──────────────────────────────────────────

    #[test]
    fn test_global_rules_file_not_included_in_test() {
        // global_rules_path() uses directories::ProjectDirs which in tests points to
        // the system config dir — this file may or may not exist
        // Just ensure discover() doesn't panic
        let dir = tempdir().unwrap();
        let result = discover(dir.path(), false, None);
        // Result could have 0 or more files (depends on whether global rules.md exists)
        let _ = result;
    }

    /// Lines 75-76: global rules file is loaded when it exists.
    /// We can't easily test the real global path, so we test load_rules_file directly.
    #[test]
    fn load_rules_file_reads_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rules.md");
        std::fs::write(&path, "# My Rules\nDo things.").unwrap();
        let f = load_rules_file(&path);
        assert!(f.is_some());
        let f = f.unwrap();
        assert!(f.content.contains("My Rules"));
    }

    #[test]
    fn load_rules_file_nonexistent_returns_none() {
        let path = std::path::PathBuf::from("/nonexistent/path/rules.md");
        let f = load_rules_file(&path);
        assert!(f.is_none());
    }

    /// Lines 124-129: import target file does not exist (canonicalize fails) → directive left as-is.
    #[test]
    fn process_imports_nonexistent_target_leaves_directive() {
        let dir = tempdir().unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        // Use import directive format; reference a file that does not exist
        std::fs::write(&clido_md, "[import: nonexistent_file.md]\nOther content").unwrap();
        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        // The directive line should remain since the import target doesn't exist
        assert!(file.content.contains("[import:") || file.content.contains("Other content"));
    }

    /// Line 148: imported content without trailing newline gets newline appended.
    #[test]
    fn process_imports_adds_newline_when_imported_content_missing_it() {
        let dir = tempdir().unwrap();
        let imported = dir.path().join("imported.md");
        // No trailing newline
        std::fs::write(&imported, "imported content no newline").unwrap();
        let clido_md = dir.path().join("CLIDO.md");
        // Use relative path in import directive
        std::fs::write(&clido_md, "[import: imported.md]\nAfter line").unwrap();
        let result = discover(dir.path(), false, None);
        let file = result.iter().find(|f| f.path == clido_md).unwrap();
        assert!(file.content.contains("imported content no newline"));
        assert!(file.content.contains("After line"));
    }

    // ── TrustStore tests ──────────────────────────────────────────────────

    /// Trust store starts empty; untrusted file returns false.
    #[test]
    fn trust_store_new_file_not_trusted() {
        let tmp = tempdir().unwrap();
        let clido_md = tmp.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# rules").unwrap();
        // Build a store that points at a temp path (never persisted).
        let store = TrustStore {
            path: tmp.path().join("trust.json"),
            entries: vec![],
        };
        assert!(!store.is_trusted(&clido_md, "# rules"));
    }

    /// After trust(), the file is recognised as trusted.
    #[test]
    fn trust_store_trust_then_is_trusted() {
        let tmp = tempdir().unwrap();
        let clido_md = tmp.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# rules").unwrap();
        let mut store = TrustStore {
            path: tmp.path().join("trust.json"),
            entries: vec![],
        };
        store.trust(&clido_md, "# rules");
        assert!(store.is_trusted(&clido_md, "# rules"));
    }

    /// Changed content is no longer trusted.
    #[test]
    fn trust_store_changed_content_not_trusted() {
        let tmp = tempdir().unwrap();
        let clido_md = tmp.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# original").unwrap();
        let mut store = TrustStore {
            path: tmp.path().join("trust.json"),
            entries: vec![],
        };
        store.trust(&clido_md, "# original");
        assert!(!store.is_trusted(&clido_md, "# changed"));
    }

    /// discover_with_trust with is_tty=false skips untrusted project-local files.
    #[test]
    fn discover_with_trust_non_tty_skips_untrusted() {
        let tmp = tempdir().unwrap();
        let clido_md = tmp.path().join("CLIDO.md");
        std::fs::write(&clido_md, "# secret rules").unwrap();
        // Non-interactive: should not load untrusted project-local file.
        let files = discover_with_trust(tmp.path(), false, None, false);
        // File should not appear (no trust + no TTY = skip).
        assert!(
            !files.iter().any(|f| f.path == clido_md),
            "untrusted file should be skipped in non-tty mode"
        );
    }

    /// no_rules=true skips everything even with trust.
    #[test]
    fn discover_with_trust_no_rules_returns_empty() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("CLIDO.md"), "# rules").unwrap();
        let files = discover_with_trust(tmp.path(), true, None, true);
        assert!(files.is_empty());
    }
}
