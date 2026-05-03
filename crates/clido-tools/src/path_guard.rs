//! Path canonicalization, workspace-root check, and blocked-path enforcement.

use std::path::{Path, PathBuf};

/// Stable message when a path is outside the workspace and not allow-listed.
/// The agent matches on this for interactive `/allow-path` recovery — keep in sync with callers.
pub const ACCESS_DENIED_OUTSIDE_WORKSPACE: &str = "Access denied: path outside working directory.";

/// Path-access guard: restricts operations to workspace_root and rejects blocked paths.
/// Also allows access to explicitly permitted external paths outside the workspace.
#[derive(Clone)]
pub struct PathGuard {
    root: PathBuf,
    blocked: Vec<PathBuf>,
    allowed_external: Vec<PathBuf>,
    allowed_dirs: Vec<PathBuf>,
}

impl PathGuard {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            root: workspace_root,
            blocked: Vec::new(),
            allowed_external: Vec::new(),
            allowed_dirs: Vec::new(),
        }
    }

    pub fn with_blocked(mut self, paths: Vec<PathBuf>) -> Self {
        // Canonicalize each blocked path so we can compare robustly.
        self.blocked = paths
            .into_iter()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
        self
    }

    /// Add allowed external paths (outside workspace_root) that tools may access.
    pub fn with_allowed_external(mut self, paths: Vec<PathBuf>) -> Self {
        // Canonicalize each allowed path so we can compare robustly.
        self.allowed_external = paths
            .into_iter()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
        self
    }

    /// Add allowed external directories — any file under these dirs is accessible.
    pub fn with_allowed_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.allowed_dirs = dirs
            .into_iter()
            .map(|d| std::fs::canonicalize(&d).unwrap_or(d))
            .collect();
        self
    }

    /// Set allowed external paths dynamically (for runtime permission changes).
    pub fn set_allowed_external(&mut self, paths: Vec<PathBuf>) {
        self.allowed_external = paths
            .into_iter()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
    }

    /// Set allowed external directories dynamically.
    pub fn set_allowed_dirs(&mut self, dirs: Vec<PathBuf>) {
        self.allowed_dirs = dirs
            .into_iter()
            .map(|d| std::fs::canonicalize(&d).unwrap_or(d))
            .collect();
    }

    /// Get current allowed directories (for display/serialization).
    pub fn allowed_dirs(&self) -> &[PathBuf] {
        &self.allowed_dirs
    }

    /// Check if a canonical path is within any allowed external path or directory.
    fn is_in_allowed_external(&self, canonical: &Path) -> bool {
        self.allowed_external
            .iter()
            .any(|allowed| canonical == allowed || canonical.starts_with(allowed))
            || self
                .allowed_dirs
                .iter()
                .any(|dir| canonical == dir || canonical.starts_with(dir))
    }

    /// Canonicalize path and ensure it is under workspace_root (or explicitly allowed
    /// external path) and not blocked.
    pub fn resolve_and_check(&self, path: &str) -> Result<PathBuf, String> {
        let root_canon = std::fs::canonicalize(&self.root)
            .map_err(|e| format!("canonicalize workspace root: {e}"))?;
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root_canon.join(path)
        };
        let normalized = normalize_path(&joined);
        let canonical = std::fs::canonicalize(&normalized)
            .map_err(|e| format!("canonicalize {}: {e}", normalized.display()))?;

        // Check workspace root first
        if canonical.starts_with(&root_canon) {
            if self.is_blocked(&canonical) {
                return Err("Access denied: this file is protected.".to_string());
            }
            return Ok(canonical);
        }

        // Check allowed external paths
        if self.is_in_allowed_external(&canonical) {
            if self.is_blocked(&canonical) {
                return Err("Access denied: this file is protected.".to_string());
            }
            return Ok(canonical);
        }

        Err(ACCESS_DENIED_OUTSIDE_WORKSPACE.to_string())
    }

    /// Resolve path for write: file may not exist yet.
    /// Also allows paths under allowed external directories.
    pub fn resolve_for_write(&self, path: &str) -> Result<PathBuf, String> {
        let root_canon = std::fs::canonicalize(&self.root)
            .map_err(|e| format!("canonicalize workspace root: {e}"))?;
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root_canon.join(path)
        };

        // For existing files, canonicalize and check
        if joined.exists() {
            let canonical = std::fs::canonicalize(&joined)
                .map_err(|e| format!("canonicalize {}: {e}", joined.display()))?;

            // Check workspace root
            if canonical.starts_with(&root_canon) {
                if self.is_blocked(&canonical) {
                    return Err("Access denied: this file is protected.".to_string());
                }
                return Ok(canonical);
            }

            // Check allowed external paths
            if self.is_in_allowed_external(&canonical) {
                if self.is_blocked(&canonical) {
                    return Err("Access denied: this file is protected.".to_string());
                }
                return Ok(canonical);
            }

            return Err(ACCESS_DENIED_OUTSIDE_WORKSPACE.to_string());
        }

        // For new files, check parent directory
        if let Some(parent) = joined.parent() {
            let canon_parent = match std::fs::canonicalize(parent) {
                Ok(p) => p,
                Err(_) => {
                    // Parent doesn't exist - check if it would be in workspace
                    if parent == root_canon || parent.starts_with(&root_canon) {
                        let normalized = normalize_path(&joined);
                        if self.is_blocked_raw(&normalized) {
                            return Err("Access denied: this file is protected.".to_string());
                        }
                        return Ok(joined);
                    }
                    // Check if parent would be in allowed external paths
                    if self.is_in_allowed_external(parent) {
                        let normalized = normalize_path(&joined);
                        if self.is_blocked_raw(&normalized) {
                            return Err("Access denied: this file is protected.".to_string());
                        }
                        return Ok(joined);
                    }
                    return Err(ACCESS_DENIED_OUTSIDE_WORKSPACE.to_string());
                }
            };

            // Check workspace root
            if canon_parent.starts_with(&root_canon) {
                if let Some(name) = joined.file_name() {
                    let target = canon_parent.join(name);
                    if self.is_blocked(&target) {
                        return Err("Access denied: this file is protected.".to_string());
                    }
                    return Ok(target);
                }
                return Ok(joined);
            }

            // Check allowed external paths
            if self.is_in_allowed_external(&canon_parent) {
                if let Some(name) = joined.file_name() {
                    let target = canon_parent.join(name);
                    if self.is_blocked(&target) {
                        return Err("Access denied: this file is protected.".to_string());
                    }
                    return Ok(target);
                }
                return Ok(joined);
            }

            return Err(ACCESS_DENIED_OUTSIDE_WORKSPACE.to_string());
        }

        Ok(joined)
    }

    /// True if a (possibly non-canonical) path matches any blocked entry.
    pub fn is_blocked(&self, path: &Path) -> bool {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.blocked.contains(&canon)
    }

    fn is_blocked_raw(&self, path: &Path) -> bool {
        self.blocked.iter().any(|b| path == *b)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.root
    }
}

fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => out.push(c),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(s) => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_under_root() {
        let tmp = std::env::temp_dir().join("clido_path_guard_test_1");
        let _ = std::fs::create_dir_all(&tmp);
        let f = tmp.join("a").join("b.txt");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "x").unwrap();
        let res = PathGuard::new(tmp.clone()).resolve_and_check("a/b.txt");
        assert!(res.is_ok());
        assert!(res.unwrap().ends_with("b.txt"));
    }

    #[test]
    fn path_outside_root_denied() {
        let workspace = std::env::temp_dir().join("clido_path_guard_test_2");
        std::fs::create_dir_all(&workspace).unwrap();
        // Absolute path outside the workspace (stable regardless of how many `..` segments exist).
        let outside = std::env::temp_dir().join("clido_path_guard_outside_file.txt");
        std::fs::write(&outside, b"x").unwrap();
        let outside_s = outside.to_string_lossy();
        let res = PathGuard::new(workspace).resolve_and_check(outside_s.as_ref());
        assert!(res.is_err(), "expected err, got {res:?}");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("Access denied"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn blocked_path_denied() {
        let tmp = std::env::temp_dir().join("clido_path_guard_test_blocked");
        std::fs::create_dir_all(&tmp).unwrap();
        let secret = tmp.join("config.toml");
        std::fs::write(&secret, "secret").unwrap();
        let guard = PathGuard::new(tmp.clone()).with_blocked(vec![secret.clone()]);
        let res = guard.resolve_and_check("config.toml");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("protected"));
    }

    #[test]
    fn resolve_for_write_existing_file() {
        let tmp = std::env::temp_dir().join("clido_pfwrite_1");
        std::fs::create_dir_all(&tmp).unwrap();
        let f = tmp.join("out.txt");
        std::fs::write(&f, "content").unwrap();
        let guard = PathGuard::new(tmp.clone());
        let res = guard.resolve_for_write("out.txt");
        assert!(res.is_ok(), "err: {:?}", res);
    }

    #[test]
    fn resolve_for_write_new_file_in_root() {
        let tmp = std::env::temp_dir().join("clido_pfwrite_2");
        std::fs::create_dir_all(&tmp).unwrap();
        let guard = PathGuard::new(tmp.clone());
        let res = guard.resolve_for_write("new_file.txt");
        assert!(res.is_ok(), "err: {:?}", res);
    }

    #[test]
    fn resolve_for_write_blocked_existing_file() {
        let tmp = std::env::temp_dir().join("clido_pfwrite_blocked");
        std::fs::create_dir_all(&tmp).unwrap();
        let secret = tmp.join("secret.toml");
        std::fs::write(&secret, "secret").unwrap();
        let guard = PathGuard::new(tmp.clone()).with_blocked(vec![secret.clone()]);
        let res = guard.resolve_for_write("secret.toml");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("protected"));
    }

    #[test]
    fn resolve_for_write_outside_root_denied() {
        let tmp = std::env::temp_dir().join("clido_pfwrite_outside");
        std::fs::create_dir_all(&tmp).unwrap();
        let guard = PathGuard::new(tmp.clone());
        // Use an absolute path that definitely exists and is outside any tempdir.
        let res = guard.resolve_for_write("/etc/hosts");
        assert!(
            res.is_err(),
            "expected Err for /etc/hosts outside root, got Ok"
        );
        assert!(res.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn workspace_root_accessor() {
        let tmp = std::env::temp_dir();
        let guard = PathGuard::new(tmp.clone());
        assert_eq!(guard.workspace_root(), tmp.as_path());
    }

    #[test]
    fn normalize_dotdot() {
        let p = super::normalize_path(std::path::Path::new("/a/b/../c"));
        assert_eq!(p, std::path::PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_curdot() {
        let p = super::normalize_path(std::path::Path::new("/a/./b/./c"));
        assert_eq!(p, std::path::PathBuf::from("/a/b/c"));
    }

    /// Line 33: absolute path that IS under root hits `PathBuf::from(path)` branch.
    #[test]
    fn absolute_path_under_root_resolve_and_check() {
        let tmp = tempfile::tempdir().unwrap();
        // Use the canonical root so macOS /var -> /private/var symlink doesn't cause issues.
        let root_canon = std::fs::canonicalize(tmp.path()).unwrap();
        let f = root_canon.join("abs_file.txt");
        std::fs::write(&f, "data").unwrap();
        let guard = PathGuard::new(root_canon.clone());
        // Pass the absolute canonical path string
        let path_str = f.to_str().unwrap();
        let res = guard.resolve_and_check(path_str);
        assert!(
            res.is_ok(),
            "expected Ok for absolute path under root, got {:?}",
            res
        );
    }

    /// Line 43: symlink inside root that points outside → second bounds check triggers.
    #[cfg(unix)]
    #[test]
    fn symlink_escaping_root_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("escape_link");
        // Create symlink pointing outside root (to /tmp itself or /etc)
        std::os::unix::fs::symlink("/etc", &link).unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        let res = guard.resolve_and_check("escape_link");
        assert!(res.is_err(), "expected Err for symlink escape, got Ok");
        assert!(res.unwrap_err().contains("Access denied"));
    }

    /// Lines 73-80: new file whose parent doesn't exist yet but is under root.
    #[test]
    fn resolve_for_write_new_file_in_nonexistent_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        // subdir does not exist; joined path won't exist; canonicalize(parent) will fail
        let res = guard.resolve_for_write("nonexistent_subdir/newfile.txt");
        // Should succeed since the parent path starts_with root_canon
        assert!(
            res.is_ok(),
            "expected Ok for new file in nonexistent subdir, got {:?}",
            res
        );
    }

    /// Line 76-77: new file in nonexistent subdir that is blocked via raw path.
    #[test]
    fn resolve_for_write_new_file_in_nonexistent_subdir_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        // Use canonical root to avoid macOS symlink issues.
        let root_canon = std::fs::canonicalize(tmp.path()).unwrap();
        // Block the normalized path that would be produced (root_canon/ghost_dir/blocked.txt)
        let normalized_blocked = root_canon.join("ghost_dir").join("blocked.txt");
        // with_blocked tries to canonicalize — since it doesn't exist, falls back to path as-is
        let guard = PathGuard::new(root_canon.clone()).with_blocked(vec![normalized_blocked]);
        let res = guard.resolve_for_write("ghost_dir/blocked.txt");
        assert!(
            res.is_err(),
            "expected Err for blocked file in nonexistent subdir, got Ok"
        );
        assert!(res.unwrap_err().contains("protected"));
    }

    /// Line 81: parent of new file is outside root and doesn't exist → Err.
    #[test]
    fn resolve_for_write_new_file_parent_outside_root_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        // Absolute path to a file whose parent doesn't exist and is outside root
        let res = guard.resolve_for_write("/nonexistent_outside_root_xyz/file.txt");
        assert!(res.is_err(), "expected Err for file outside root, got Ok");
        assert!(res.unwrap_err().contains("Access denied"));
    }

    /// Lines 84-85: canon_parent exists but is outside root.
    #[test]
    fn resolve_for_write_absolute_path_parent_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = PathGuard::new(tmp.path().to_path_buf());
        // /tmp exists and canonicalizes, but it's not under tmp subdir
        let outside_root = std::env::temp_dir().join("some_file_outside.txt");
        let path_str = outside_root.to_str().unwrap();
        let res = guard.resolve_for_write(path_str);
        assert!(res.is_err(), "expected Err for parent outside root, got Ok");
        assert!(res.unwrap_err().contains("Access denied"));
    }

    /// Line 95: resolve_for_write returns Ok(joined) when parent() is None.
    /// This path is taken when the path has no parent (e.g., a bare filename
    /// with no directory component after join and the parent is None).
    /// In practice, root.join("foo") always has a parent, but a path like "."
    /// or "/" might not. Let's test normalize_path with a CurDir component (line 118).
    #[test]
    fn normalize_path_with_cur_dir_component() {
        // "./foo" has a CurDir component followed by Normal
        let p = std::path::Path::new("./foo/bar");
        let normalized = normalize_path(p);
        // CurDir (.) is ignored, so result is "foo/bar"
        let normalized_str = normalized.to_string_lossy();
        assert!(normalized_str.ends_with("foo/bar") || normalized_str == "foo/bar");
    }

    /// Line 95: resolve_for_write fallthrough when joined.file_name() is None.
    /// This can happen when path is purely a root/parent component.
    /// We test the closest thing: resolve_for_write on a path that goes
    /// through the normal flow and hits line 95.
    #[test]
    fn resolve_for_write_path_with_no_file_name_component() {
        let tmp = tempfile::tempdir().unwrap();
        let root_canon = std::fs::canonicalize(tmp.path()).unwrap();
        let guard = PathGuard::new(root_canon.clone());
        // A path ending in ".." or "." might have None file_name
        // joined = root / ".." → parent exists but file_name() is None for ".."
        // This should fall through to Ok(joined)
        let res = guard.resolve_for_write("..");
        // Either denied (outside root) or an error — we just ensure no panic
        let _ = res;
    }

    /// Lines 89-90: new file in existing dir that matches a blocked path.
    #[test]
    fn resolve_for_write_new_file_blocked_in_existing_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root_canon = std::fs::canonicalize(tmp.path()).unwrap();
        let subdir = root_canon.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        // Block the canonical target path that would be produced (canon_parent/name).
        // subdir exists so canonicalize succeeds; blocked_target doesn't exist so
        // with_blocked stores it as-is, and is_blocked also falls back to as-is.
        let blocked_target = subdir.join("blocked_new.txt");
        let guard = PathGuard::new(root_canon.clone()).with_blocked(vec![blocked_target]);
        let res = guard.resolve_for_write("subdir/blocked_new.txt");
        assert!(
            res.is_err(),
            "expected Err for blocked new file target, got Ok"
        );
        assert!(res.unwrap_err().contains("protected"));
    }

    // ── resolve_and_check ──────────────────────────────────────────────

    #[test]
    fn resolve_and_check_relative_nested_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let deep = root.join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        let file = deep.join("deep.txt");
        std::fs::write(&file, "deep").unwrap();

        let guard = PathGuard::new(root.clone());
        let res = guard.resolve_and_check("a/b/c/deep.txt").unwrap();
        assert_eq!(res, file);
    }

    #[test]
    fn resolve_and_check_absolute_outside_separate_tmpdir() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let other_file = other.path().join("other.txt");
        std::fs::write(&other_file, "data").unwrap();

        let guard = PathGuard::new(workspace.path().to_path_buf());
        let res = guard.resolve_and_check(other_file.to_str().unwrap());
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn resolve_and_check_dotdot_staying_inside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let sub = root.join("dir");
        std::fs::create_dir_all(&sub).unwrap();
        let file = root.join("top.txt");
        std::fs::write(&file, "top").unwrap();

        let guard = PathGuard::new(root.clone());
        // "dir/../top.txt" traverses up but stays inside workspace
        let res = guard.resolve_and_check("dir/../top.txt").unwrap();
        assert_eq!(res, file);
    }

    #[test]
    fn resolve_and_check_dotdot_escaping_from_child_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();

        // Workspace is the child dir; "../" escapes to root
        let guard = PathGuard::new(child.clone());
        let res = guard.resolve_and_check("../");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn resolve_and_check_multiple_blocked_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f1 = root.join("secret1.key");
        let f2 = root.join("secret2.key");
        let f3 = root.join("public.txt");
        std::fs::write(&f1, "k1").unwrap();
        std::fs::write(&f2, "k2").unwrap();
        std::fs::write(&f3, "pub").unwrap();

        let guard = PathGuard::new(root.clone()).with_blocked(vec![f1.clone(), f2.clone()]);

        let r1 = guard.resolve_and_check("secret1.key");
        assert!(r1.is_err());
        assert!(r1.unwrap_err().contains("protected"));

        let r2 = guard.resolve_and_check("secret2.key");
        assert!(r2.is_err());
        assert!(r2.unwrap_err().contains("protected"));

        // Non-blocked file is fine
        let r3 = guard.resolve_and_check("public.txt").unwrap();
        assert_eq!(r3, f3);
    }

    // ── resolve_for_write ──────────────────────────────────────────────

    #[test]
    fn resolve_for_write_relative_existing_in_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let sub = root.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("lib.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let guard = PathGuard::new(root.clone());
        let res = guard.resolve_for_write("src/lib.rs").unwrap();
        assert_eq!(res, file);
    }

    #[test]
    fn resolve_for_write_new_file_existing_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let sub = root.join("docs");
        std::fs::create_dir_all(&sub).unwrap();

        let guard = PathGuard::new(root.clone());
        let res = guard.resolve_for_write("docs/new_page.md").unwrap();
        assert_eq!(res, sub.join("new_page.md"));
    }

    #[test]
    fn resolve_for_write_absolute_outside_separate_tmpdir() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let other_file = std::fs::canonicalize(other.path())
            .unwrap()
            .join("external.txt");
        std::fs::write(&other_file, "ext").unwrap();

        let guard = PathGuard::new(workspace.path().to_path_buf());
        let res = guard.resolve_for_write(other_file.to_str().unwrap());
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Access denied"));
    }

    #[test]
    fn resolve_for_write_dotdot_escaping_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let child = root.join("sandbox");
        std::fs::create_dir_all(&child).unwrap();

        let guard = PathGuard::new(child.clone());
        let res = guard.resolve_for_write("../escape.txt");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Access denied"));
    }

    // ── with_blocked / is_blocked ──────────────────────────────────────

    #[test]
    fn with_blocked_canonicalizes_existing_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let file = root.join("guarded.txt");
        std::fs::write(&file, "g").unwrap();

        // Pass non-canonical path (via tempdir original path)
        let non_canon = tmp.path().join("guarded.txt");
        let guard = PathGuard::new(root.clone()).with_blocked(vec![non_canon]);
        assert!(guard.is_blocked(&file));
    }

    #[test]
    fn is_blocked_returns_false_for_unblocked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let blocked = root.join("blocked.txt");
        let safe = root.join("safe.txt");
        std::fs::write(&blocked, "b").unwrap();
        std::fs::write(&safe, "s").unwrap();

        let guard = PathGuard::new(root.clone()).with_blocked(vec![blocked.clone()]);
        assert!(guard.is_blocked(&blocked));
        assert!(!guard.is_blocked(&safe));
    }

    #[test]
    fn with_blocked_empty_vec_blocks_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let file = root.join("anything.txt");
        std::fs::write(&file, "x").unwrap();

        let guard = PathGuard::new(root.clone()).with_blocked(vec![]);
        assert!(!guard.is_blocked(&file));
        let res = guard.resolve_and_check("anything.txt").unwrap();
        assert_eq!(res, file);
    }
}
