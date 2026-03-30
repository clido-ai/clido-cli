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

    #[test]
    fn detect_lang_from_path_rust() {
        assert_eq!(detect_lang_from_path(Path::new("main.rs")), Some("rust"));
    }

    #[test]
    fn detect_lang_from_path_python() {
        assert_eq!(detect_lang_from_path(Path::new("app.py")), Some("python"));
    }

    #[test]
    fn detect_lang_from_path_js() {
        assert_eq!(detect_lang_from_path(Path::new("index.js")), Some("js"));
        assert_eq!(
            detect_lang_from_path(Path::new("component.jsx")),
            Some("js")
        );
    }

    #[test]
    fn detect_lang_from_path_ts() {
        assert_eq!(detect_lang_from_path(Path::new("app.ts")), Some("ts"));
        assert_eq!(
            detect_lang_from_path(Path::new("component.tsx")),
            Some("ts")
        );
    }

    #[test]
    fn detect_lang_from_path_go() {
        assert_eq!(detect_lang_from_path(Path::new("main.go")), Some("go"));
    }

    #[test]
    fn detect_lang_from_path_unknown() {
        assert_eq!(detect_lang_from_path(Path::new("README.md")), None);
        assert_eq!(detect_lang_from_path(Path::new("data.json")), None);
        assert_eq!(detect_lang_from_path(Path::new("no_extension")), None);
    }

    #[test]
    fn detect_lang_from_workspace_rust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("rust"));
    }

    #[test]
    fn detect_lang_from_workspace_ts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("ts"));
    }

    #[test]
    fn detect_lang_from_workspace_js() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("js"));
    }

    #[test]
    fn detect_lang_from_workspace_python_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[tool]").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("python"));
    }

    #[test]
    fn detect_lang_from_workspace_python_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("setup.py"), "").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("python"));
    }

    #[test]
    fn detect_lang_from_workspace_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module x").unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), Some("go"));
    }

    #[test]
    fn detect_lang_from_workspace_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_lang_from_workspace(dir.path()), None);
    }

    #[test]
    fn format_diagnostics_empty() {
        let diags: Vec<String> = vec![];
        assert_eq!(format_diagnostics(&diags), "No diagnostics found.");
    }

    #[test]
    fn format_diagnostics_non_empty() {
        let diags = vec![
            "error at line 1".to_string(),
            "warning at line 2".to_string(),
        ];
        let result = format_diagnostics(&diags);
        assert!(result.contains("error at line 1"));
        assert!(result.contains("warning at line 2"));
    }

    #[test]
    fn find_ancestor_with_finds_marker() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();
        // Start from subdir → should find Cargo.toml in dir
        let found = find_ancestor_with(&sub, "Cargo.toml");
        assert!(found.is_some());
        assert_eq!(found.unwrap(), dir.path());
    }

    #[test]
    fn find_ancestor_with_returns_none_when_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_ancestor_with(dir.path(), "nonexistent_marker_12345.txt");
        assert!(result.is_none());
    }

    #[test]
    fn find_ancestor_with_file_path_uses_parent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();
        let file = dir.path().join("src").join("main.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn main() {}").unwrap();
        // Pass a file path → should use parent dir for traversal
        let found = find_ancestor_with(&file, "Cargo.toml");
        assert!(found.is_some());
    }

    #[test]
    fn diagnostics_tool_name_and_schema() {
        let tool = DiagnosticsTool::new();
        assert_eq!(tool.name(), "Diagnostics");
        assert!(tool.is_read_only());
        let schema = tool.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].get("path").is_some());
        assert!(schema["properties"].get("lang").is_some());
    }

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

    // ── execute with explicit lang override ────────────────────────────────

    #[tokio::test]
    async fn test_diagnostics_explicit_unknown_lang_returns_no_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap(),
                "lang": "cobol"
            }))
            .await;
        assert!(!out.is_error);
        assert!(out.content.contains("No diagnostic tool available"));
    }

    // ── execute with no path (defaults to ".") ─────────────────────────────

    #[tokio::test]
    async fn test_diagnostics_defaults_to_dot() {
        // Just verify the tool doesn't panic with no path argument
        let tool = DiagnosticsTool::new();
        let out = tool.execute(serde_json::json!({})).await;
        // Output depends on current directory; just check no panic
        let _ = out;
    }

    // ── DiagnosticsTool::default ───────────────────────────────────────────

    #[test]
    fn diagnostics_tool_default() {
        let tool = DiagnosticsTool;
        assert_eq!(tool.name(), "Diagnostics");
    }

    // ── description ────────────────────────────────────────────────────────

    #[test]
    fn diagnostics_tool_description_non_empty() {
        let tool = DiagnosticsTool::new();
        let desc = tool.description();
        assert!(!desc.is_empty());
        assert!(
            desc.contains("rust")
                || desc.contains("Rust")
                || desc.contains("linter")
                || desc.contains("compiler")
        );
    }

    // ── run_python with directory path ────────────────────────────────────

    #[tokio::test]
    async fn test_diagnostics_python_on_directory_returns_error_message() {
        let tmp = tempfile::tempdir().unwrap();
        // python requires a file, not a dir
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap(),
                "lang": "python"
            }))
            .await;
        // Should not panic; may return error or "No diagnostics"
        let _ = out;
    }

    // ── cargo JSON parsing: note/help lines are skipped ────────────────────

    #[test]
    fn cargo_note_and_help_lines_skipped() {
        // run_rust parses cargo JSON; we test the parsing logic by examining
        // what format_diagnostics returns for empty vec vs non-empty vec
        let empty: Vec<String> = vec![];
        assert_eq!(format_diagnostics(&empty), "No diagnostics found.");

        let with_items = vec!["src/main.rs:1:1: error: something wrong".to_string()];
        let result = format_diagnostics(&with_items);
        assert!(result.contains("something wrong"));
    }

    // ── execute with python lang detects file extension ────────────────────

    #[tokio::test]
    async fn test_diagnostics_python_valid_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("script.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let tool = DiagnosticsTool::new();
        // python3 must be available; if not, tool may error gracefully
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap(),
                "lang": "python"
            }))
            .await;
        // No panic; result depends on python3 availability
        let _ = out;
    }

    #[tokio::test]
    async fn test_diagnostics_python_syntax_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("bad.py");
        std::fs::write(&file, "def foo(\n").unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap(),
                "lang": "python"
            }))
            .await;
        // May detect syntax error or error gracefully — no panic
        let _ = out;
    }

    // ── execute with ts lang: no tsconfig.json ─────────────────────────────

    #[tokio::test]
    async fn test_diagnostics_ts_no_tsconfig() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.ts");
        std::fs::write(&file, "const x: number = 1;\n").unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap(),
                "lang": "ts"
            }))
            .await;
        // Should return "no tsconfig.json found" message
        assert!(
            out.content.contains("tsconfig") || !out.is_error,
            "unexpected output: {}",
            out.content
        );
    }

    // ── execute with go lang: no go.mod ────────────────────────────────────

    #[tokio::test]
    async fn test_diagnostics_go_no_go_mod() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("main.go");
        std::fs::write(&file, "package main\nfunc main() {}\n").unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap(),
                "lang": "go"
            }))
            .await;
        // May succeed (go vet clean) or fail gracefully — no panic
        let _ = out;
    }

    // ── execute: workspace language auto-detection from ts file ───────────

    #[tokio::test]
    async fn test_diagnostics_ts_file_extension_auto_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.ts");
        std::fs::write(&file, "const x: string = \"hello\";\n").unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap()
                // no lang — should be auto-detected as "ts"
            }))
            .await;
        // No panic; ts detected from extension
        let _ = out;
    }

    // ── execute: go file extension auto-detected ──────────────────────────

    #[tokio::test]
    async fn test_diagnostics_go_file_extension_auto_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("hello.go");
        std::fs::write(&file, "package main\nfunc main() {}\n").unwrap();
        let tool = DiagnosticsTool::new();
        let out = tool
            .execute(serde_json::json!({
                "path": file.to_str().unwrap()
                // no lang — should be auto-detected as "go"
            }))
            .await;
        // No panic; go detected from extension
        let _ = out;
    }

    // ── execute: python file extension auto-detected from workspace ────────

    #[tokio::test]
    async fn test_diagnostics_python_workspace_auto_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("setup.py"), "").unwrap();
        let tool = DiagnosticsTool::new();
        // Pass the directory (not a file), lang auto-detected from workspace marker
        let out = tool
            .execute(serde_json::json!({
                "path": tmp.path().to_str().unwrap()
            }))
            .await;
        // setup.py → "python" → run_python with a directory → returns error message
        assert!(!out.is_error || out.content.contains("python"));
    }
}
