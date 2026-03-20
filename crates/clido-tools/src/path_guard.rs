//! Path canonicalization, workspace-root check, and blocked-path enforcement.

use std::path::{Path, PathBuf};

/// Path-access guard: restricts operations to workspace_root and rejects blocked paths.
#[derive(Clone)]
pub struct PathGuard {
    root: PathBuf,
    blocked: Vec<PathBuf>,
}

impl PathGuard {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            root: workspace_root,
            blocked: Vec::new(),
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

    /// Canonicalize path and ensure it is under workspace_root and not blocked.
    pub fn resolve_and_check(&self, path: &str) -> Result<PathBuf, String> {
        let root_canon = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root_canon.join(path)
        };
        let normalized = normalize_path(&joined);
        if !normalized.starts_with(&root_canon) {
            return Err("Access denied: path outside working directory.".to_string());
        }
        let canonical = std::fs::canonicalize(&normalized).map_err(|e| e.to_string())?;
        if !canonical.starts_with(&root_canon) {
            return Err("Access denied: path outside working directory.".to_string());
        }
        if self.is_blocked(&canonical) {
            return Err("Access denied: this file is protected.".to_string());
        }
        Ok(canonical)
    }

    /// Resolve path for write: file may not exist yet.
    pub fn resolve_for_write(&self, path: &str) -> Result<PathBuf, String> {
        let root_canon = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root_canon.join(path)
        };
        if joined.exists() {
            let canonical = std::fs::canonicalize(&joined).map_err(|e| e.to_string())?;
            if !canonical.starts_with(&root_canon) {
                return Err("Access denied: path outside working directory.".to_string());
            }
            if self.is_blocked(&canonical) {
                return Err("Access denied: this file is protected.".to_string());
            }
            return Ok(canonical);
        }
        if let Some(parent) = joined.parent() {
            let canon_parent = match std::fs::canonicalize(parent) {
                Ok(p) => p,
                Err(_) => {
                    if parent == root_canon || parent.starts_with(&root_canon) {
                        // Check normalized target too
                        let normalized = normalize_path(&joined);
                        if self.is_blocked_raw(&normalized) {
                            return Err("Access denied: this file is protected.".to_string());
                        }
                        return Ok(joined);
                    }
                    return Err("Access denied: path outside working directory.".to_string());
                }
            };
            if !canon_parent.starts_with(&root_canon) {
                return Err("Access denied: path outside working directory.".to_string());
            }
            if let Some(name) = joined.file_name() {
                let target = canon_parent.join(name);
                if self.is_blocked(&target) {
                    return Err("Access denied: this file is protected.".to_string());
                }
                return Ok(target);
            }
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
        let tmp = std::env::temp_dir().join("clido_path_guard_test_2");
        std::fs::create_dir_all(&tmp).unwrap();
        let res = PathGuard::new(tmp.clone()).resolve_and_check("../../../etc/passwd");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Access denied"));
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
}
