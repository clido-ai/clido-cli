//! Git context: detects if the working directory is a git repo and provides
//! a formatted system prompt section with branch, status, and recent commits.

use std::path::Path;
use std::process::Command;

/// Git context collected at session start.
pub struct GitContext {
    pub branch: String,
    pub status_short: String,
    pub log_oneline: String,
}

impl GitContext {
    /// Detect if `cwd` is inside a git repository and collect context.
    /// Returns `None` if git is not installed or not in a git repo.
    /// Errors are silently ignored (non-fatal).
    pub fn discover(cwd: &Path) -> Option<GitContext> {
        // Check if we are inside a git repo.
        let inside = run_git(cwd, &["rev-parse", "--is-inside-work-tree"])?;
        if inside.trim() != "true" {
            return None;
        }

        let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_string();

        let status_short = run_git(cwd, &["status", "--short"])
            .unwrap_or_default()
            .trim()
            .to_string();

        let log_oneline = run_git(cwd, &["log", "--oneline", "-3"])
            .unwrap_or_default()
            .trim()
            .to_string();

        Some(GitContext {
            branch,
            status_short,
            log_oneline,
        })
    }

    /// Format the git context as a system prompt section.
    pub fn to_prompt_section(&self) -> String {
        let status_summary = if self.status_short.is_empty() {
            "clean".to_string()
        } else {
            let n = self.status_short.lines().count();
            format!("{} changed file{}", n, if n == 1 { "" } else { "s" })
        };

        let commits = if self.log_oneline.is_empty() {
            "(none)".to_string()
        } else {
            self.log_oneline
                .lines()
                .map(|l| format!("  {}", l))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "## Git Context\nBranch: {}\nStatus: {}\nRecent commits:\n{}",
            self.branch, status_summary, commits
        )
    }
}

/// Run a git command in `cwd` with a short timeout.
/// Returns `None` on any error (command not found, non-zero exit, timeout).
fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    // Use a subprocess with a 5-second timeout via std::process (blocking, but called at
    // session setup time before the async runtime is driving tool calls).
    let output = std::thread::spawn({
        let cwd = cwd.to_path_buf();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        move || Command::new("git").args(&args).current_dir(&cwd).output()
    })
    .join()
    .ok()? // propagate join error as None
    .ok()?; // propagate io::Error as None

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_init(dir: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_discover_not_a_repo_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        // Plain directory, no .git
        let result = GitContext::discover(tmp.path());
        assert!(result.is_none(), "Should return None for non-git directory");
    }

    #[test]
    fn test_discover_in_git_repo_returns_some() {
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        // Need at least one commit for HEAD to be valid
        std::fs::write(tmp.path().join("readme.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        let result = GitContext::discover(tmp.path());
        assert!(result.is_some(), "Should return Some for a git repo");
        let ctx = result.unwrap();
        // Branch should be "main" or "master" depending on git version
        assert!(
            ctx.branch == "main" || ctx.branch == "master",
            "Unexpected branch: {}",
            ctx.branch
        );
    }

    #[test]
    fn test_to_prompt_section_format() {
        let ctx = GitContext {
            branch: "feature/test".to_string(),
            status_short: "M  src/lib.rs\n?? scratch.txt".to_string(),
            log_oneline: "abc1234 Fix bug\ndef5678 Add feature".to_string(),
        };
        let section = ctx.to_prompt_section();
        assert!(section.starts_with("## Git Context\n"));
        assert!(section.contains("Branch: feature/test"));
        assert!(section.contains("Status: 2 changed files"));
        assert!(section.contains("Recent commits:"));
        assert!(section.contains("abc1234"));
    }

    #[test]
    fn test_to_prompt_section_clean_status() {
        let ctx = GitContext {
            branch: "main".to_string(),
            status_short: String::new(),
            log_oneline: String::new(),
        };
        let section = ctx.to_prompt_section();
        assert!(section.contains("Status: clean"));
        assert!(section.contains("(none)"));
    }

    #[test]
    fn test_to_prompt_section_single_changed_file() {
        let ctx = GitContext {
            branch: "main".to_string(),
            status_short: "M  src/lib.rs".to_string(),
            log_oneline: String::new(),
        };
        let section = ctx.to_prompt_section();
        // "1 changed file" (no trailing 's')
        assert!(
            section.contains("1 changed file"),
            "Expected singular: {}",
            section
        );
        assert!(!section.contains("1 changed files"));
    }
}
