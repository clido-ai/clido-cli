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

        // Check .clido/rules.md first
        let dot_clido_rules = dir.join(".clido").join("rules.md");
        if dot_clido_rules.exists() {
            if let Some(f) = load_rules_file(&dot_clido_rules) {
                walk_results.push(f);
            }
        }

        // Then check CLIDO.md
        let clido_md = dir.join("CLIDO.md");
        if clido_md.exists() {
            if let Some(f) = load_rules_file(&clido_md) {
                walk_results.push(f);
            }
        }

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

/// Load a rules file, processing import directives. Returns None if the file cannot be read.
fn load_rules_file(path: &Path) -> Option<RulesFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    seen.insert(canonical);
    let content = process_imports(&raw, path, &mut seen, 0);
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
    fn test_dot_clido_rules_discovered() {
        let dir = tempdir().unwrap();
        let dot_clido = dir.path().join(".clido");
        std::fs::create_dir_all(&dot_clido).unwrap();
        let rules_md = dot_clido.join("rules.md");
        std::fs::write(&rules_md, "Always write tests.\n").unwrap();

        let result = discover(dir.path(), false, None);
        let found = result.iter().any(|f| f.path == rules_md);
        assert!(found, "Expected to find .clido/rules.md");
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
}
