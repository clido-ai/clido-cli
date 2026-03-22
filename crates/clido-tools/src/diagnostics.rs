//! Diagnostics tool: run compiler/linter checks and return structured diagnostics.
//!
//! Supports Rust (cargo check), Python (py_compile), JS/TS (tsc), and Go (go vet).
//! Does NOT implement the LSP protocol — shells out to the compiler/linter directly.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::{Tool, ToolOutput};

/// Run compiler/linter diagnostics for a file or directory.
pub struct DiagnosticsTool;

impl DiagnosticsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DiagnosticsTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Auto-detect language from a file path's extension.
fn detect_lang_from_path(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Some("rust"),
        Some("py") => Some("python"),
        Some("js") | Some("jsx") => Some("js"),
        Some("ts") | Some("tsx") => Some("ts"),
        Some("go") => Some("go"),
        _ => None,
    }
}

/// Auto-detect language from workspace marker files in the given directory.
fn detect_lang_from_workspace(dir: &Path) -> Option<&'static str> {
    if dir.join("Cargo.toml").exists() {
        return Some("rust");
    }
    if dir.join("tsconfig.json").exists() {
        return Some("ts");
    }
    if dir.join("package.json").exists() {
        return Some("js");
    }
    if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        return Some("python");
    }
    if dir.join("go.mod").exists() {
        return Some("go");
    }
    None
}

/// Format a list of diagnostic lines as a clean string.
fn format_diagnostics(lines: &[String]) -> String {
    if lines.is_empty() {
        "No diagnostics found.".to_string()
    } else {
        lines.join("\n")
    }
}

/// Run `cargo check --message-format json` and parse the NDJSON output.
async fn run_rust(path: &Path) -> Result<Vec<String>, String> {
    // Find the workspace root (the dir containing Cargo.toml) closest to path.
    let workspace = find_ancestor_with(path, "Cargo.toml").unwrap_or_else(|| path.to_path_buf());

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("cargo")
            .arg("check")
            .arg("--message-format")
            .arg("json")
            .current_dir(&workspace)
            .output(),
    )
    .await
    .map_err(|_| "cargo check timed out after 60s".to_string())?
    .map_err(|e| format!("Failed to run cargo check: {}", e))?;

    // cargo check outputs both stdout (JSON) and stderr (human-readable).
    // Parse the JSON lines from stdout.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut diags: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if msg.get("reason").and_then(|v| v.as_str()) != Some("compiler-message") {
            continue;
        }

        let Some(message) = msg.get("message") else {
            continue;
        };

        let level = message
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Skip notes and help — they are supplementary to an error/warning.
        if level == "note" || level == "help" {
            continue;
        }

        let text = message
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let code = message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_str())
            .map(|s| format!("[{}] ", s))
            .unwrap_or_default();

        // Get the primary span (first span with is_primary = true, or first span).
        let spans = message
            .get("spans")
            .and_then(|s| s.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]);

        let primary_span = spans
            .iter()
            .find(|s| {
                s.get("is_primary")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .or_else(|| spans.first());

        if let Some(span) = primary_span {
            let file = span
                .get("file_name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let line_num = span.get("line_start").and_then(|v| v.as_u64()).unwrap_or(0);
            let col = span
                .get("column_start")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            diags.push(format!(
                "{}:{}:{}: {}: {}{}",
                file, line_num, col, level, code, text
            ));
        } else {
            diags.push(format!("?: {}: {}{}", level, code, text));
        }
    }

    Ok(diags)
}

/// Run `python3 -m py_compile <file>` and parse stderr.
async fn run_python(path: &Path) -> Result<Vec<String>, String> {
    if !path.is_file() {
        return Ok(vec![
            "python: path must be a file for py_compile".to_string()
        ]);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("python3")
            .arg("-m")
            .arg("py_compile")
            .arg(path)
            .output(),
    )
    .await
    .map_err(|_| "python3 -m py_compile timed out after 60s".to_string())?
    .map_err(|e| format!("Failed to run python3: {}", e))?;

    if output.status.success() {
        return Ok(vec![]);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    // stderr format: "  File "foo.py", line N\n    ...\nSyntaxError: msg"
    // Return the raw stderr as a diagnostic line.
    let diags: Vec<String> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(diags)
}

/// Run `npx tsc --noEmit` if tsconfig.json exists.
async fn run_ts(path: &Path) -> Result<Vec<String>, String> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let tsconfig = find_ancestor_with(&dir, "tsconfig.json")
        .or_else(|| find_ancestor_with(path, "tsconfig.json"))
        .unwrap_or_else(|| dir.clone());

    if !tsconfig.join("tsconfig.json").exists() {
        return Ok(vec![
            "ts: no tsconfig.json found, skipping TypeScript check.".to_string(),
        ]);
    }

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("npx")
            .args(["tsc", "--noEmit"])
            .current_dir(&tsconfig)
            .output(),
    )
    .await
    .map_err(|_| "tsc timed out after 60s".to_string())?
    .map_err(|e| format!("Failed to run npx tsc: {}", e))?;

    if output.status.success() {
        return Ok(vec![]);
    }

    // tsc --noEmit outputs: path/file.ts(line,col): error TSxxxx: message
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);

    let diags: Vec<String> = combined
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && (t.contains("error TS") || t.contains("warning TS"))
        })
        .map(|l| l.to_string())
        .collect();

    Ok(diags)
}

/// Run `go vet ./...` and parse output.
async fn run_go(path: &Path) -> Result<Vec<String>, String> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let workspace = find_ancestor_with(&dir, "go.mod").unwrap_or(dir);

    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("go")
            .args(["vet", "./..."])
            .current_dir(&workspace)
            .output(),
    )
    .await
    .map_err(|_| "go vet timed out after 60s".to_string())?
    .map_err(|e| format!("Failed to run go vet: {}", e))?;

    if output.status.success() {
        return Ok(vec![]);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let diags: Vec<String> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(diags)
}

/// Walk up the directory tree to find the nearest ancestor containing `marker`.
fn find_ancestor_with(start: &Path, marker: &str) -> Option<PathBuf> {
    let dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    let mut current = dir.as_path();
    loop {
        if current.join(marker).exists() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

#[async_trait]
impl Tool for DiagnosticsTool {
    fn name(&self) -> &str {
        "Diagnostics"
    }

    fn description(&self) -> &str {
        "Run compiler or linter checks on a file or directory and return diagnostics. \
         Supported languages: rust, python, js, ts, go. Language is auto-detected from \
         file extension or workspace marker files if not specified."
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File or directory to check. Defaults to current directory."
                },
                "lang": {
                    "type": "string",
                    "enum": ["rust", "python", "js", "ts", "go"],
                    "description": "Language to use. Auto-detected from extension or workspace if omitted."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let path_str = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = PathBuf::from(path_str);

        // Resolve language: explicit > extension > workspace marker.
        let lang = if let Some(l) = input.get("lang").and_then(|v| v.as_str()) {
            l.to_string()
        } else {
            let from_ext = detect_lang_from_path(&path);
            let from_ws = detect_lang_from_workspace(&path);
            match from_ext.or(from_ws) {
                Some(l) => l.to_string(),
                None => {
                    return ToolOutput::ok(
                        "No diagnostic tool available for this language.".to_string(),
                    );
                }
            }
        };

        let result = match lang.as_str() {
            "rust" => run_rust(&path).await,
            "python" => run_python(&path).await,
            "js" | "ts" => run_ts(&path).await,
            "go" => run_go(&path).await,
            _ => {
                return ToolOutput::ok(
                    "No diagnostic tool available for this language.".to_string(),
                );
            }
        };

        match result {
            Err(e) => ToolOutput::err(format!("Diagnostics error: {}", e)),
            Ok(diags) => ToolOutput::ok(format_diagnostics(&diags)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_diagnostics_rust_clean() {
        // Create a minimal temp Rust project with no errors.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        // Write Cargo.toml
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test-clean\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Write a valid main.rs
        std::fs::write(src.join("main.rs"), "fn main() {}\n").unwrap();

        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap(),
                "lang": "rust"
            }))
            .await;

        assert!(
            !out.is_error,
            "tool should not return is_error for a clean project"
        );
        assert_eq!(
            out.content.trim(),
            "No diagnostics found.",
            "clean Rust project should produce no diagnostics: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn test_diagnostics_unknown_lang() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.xyz");
        std::fs::write(&file, "hello").unwrap();

        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap()
            }))
            .await;

        assert!(!out.is_error);
        assert!(
            out.content.contains("No diagnostic tool available"),
            "unexpected output: {}",
            out.content
        );
    }
}
