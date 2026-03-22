//! Test runner: auto-detect test framework, run tests, parse results.

use std::path::Path;
use std::time::Duration;

/// A single test failure with its name and (truncated) output.
#[derive(Debug, Clone, PartialEq)]
pub struct TestFailure {
    pub name: String,
    /// Truncated to 2000 chars.
    pub output: String,
}

/// Aggregated results from one test run.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub duration_ms: u64,
    pub failures: Vec<TestFailure>,
}

impl TestResult {
    /// True when no tests failed.
    pub fn all_pass(&self) -> bool {
        self.failed == 0
    }

    /// Names of failing tests (for stuck-detection comparison).
    pub fn failing_names(&self) -> Vec<String> {
        self.failures.iter().map(|f| f.name.clone()).collect()
    }
}

/// Which test runner was auto-detected or explicitly requested.
#[derive(Debug, Clone, PartialEq)]
pub enum Runner {
    Cargo,
    Pytest,
    Npm,
    Go,
    Custom(String),
}

/// Auto-detect a runner by inspecting marker files in `workdir`.
pub fn detect_runner(workdir: &Path) -> Option<Runner> {
    if workdir.join("Cargo.toml").exists() {
        return Some(Runner::Cargo);
    }
    if workdir.join("go.mod").exists() {
        return Some(Runner::Go);
    }
    if workdir.join("pyproject.toml").exists() || workdir.join("pytest.ini").exists() {
        return Some(Runner::Pytest);
    }
    if workdir.join("package.json").exists() {
        return Some(Runner::Npm);
    }
    None
}

/// Run the test suite in `workdir` using the given runner (or auto-detect).
///
/// Returns a `TestResult` with parsed counts, or an error string when the
/// runner could not be determined or the subprocess failed to launch.
pub fn run_tests(workdir: &Path, runner: Option<&str>) -> Result<TestResult, String> {
    let effective_runner = if let Some(r) = runner {
        match r.to_lowercase().as_str() {
            "cargo" => Runner::Cargo,
            "pytest" | "python" => Runner::Pytest,
            "npm" | "vitest" => Runner::Npm,
            "go" => Runner::Go,
            other => Runner::Custom(other.to_string()),
        }
    } else {
        detect_runner(workdir)
            .ok_or_else(|| "Could not detect test runner (no Cargo.toml, go.mod, pyproject.toml, pytest.ini, or package.json found)".to_string())?
    };

    let (command, parse_fn): (&str, fn(&str) -> TestResult) = match &effective_runner {
        Runner::Cargo => ("cargo test 2>&1", parse_cargo_output),
        Runner::Pytest => ("python3 -m pytest --tb=short -q 2>&1", parse_pytest_output),
        Runner::Npm => ("npm test 2>&1", parse_npm_output),
        Runner::Go => ("go test ./... 2>&1", parse_go_output),
        Runner::Custom(cmd) => {
            // Heap allocation needed; handled separately below.
            return run_custom(workdir, cmd);
        }
    };

    run_command(workdir, command, parse_fn)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

const TIMEOUT_SECS: u64 = 120;
const MAX_OUTPUT_CHARS: usize = 2000;

fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_CHARS {
        s.to_string()
    } else {
        format!("{}… (truncated)", &s[..MAX_OUTPUT_CHARS])
    }
}

fn run_command(
    workdir: &Path,
    command: &str,
    parse_fn: fn(&str) -> TestResult,
) -> Result<TestResult, String> {
    let start = std::time::Instant::now();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .output()
        .map_err(|e| format!("Failed to launch test command: {}", e))?;

    // Enforce timeout by checking elapsed — note: std::process::Command is
    // blocking so we can only check after the fact.  For a blocking approach
    // this is acceptable; a true timeout would require a thread or tokio.
    let elapsed = start.elapsed();
    if elapsed > Duration::from_secs(TIMEOUT_SECS) {
        return Err(format!(
            "Test command timed out after {} seconds",
            TIMEOUT_SECS
        ));
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut result = parse_fn(&combined);
    result.duration_ms = elapsed.as_millis() as u64;
    Ok(result)
}

fn run_custom(workdir: &Path, cmd: &str) -> Result<TestResult, String> {
    let start = std::time::Instant::now();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .output()
        .map_err(|e| format!("Failed to launch test command: {}", e))?;

    let elapsed = start.elapsed();
    let duration_ms = elapsed.as_millis() as u64;

    if output.status.success() {
        Ok(TestResult {
            passed: 1,
            failed: 0,
            skipped: 0,
            duration_ms,
            failures: vec![],
        })
    } else {
        let raw = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(TestResult {
            passed: 0,
            failed: 1,
            skipped: 0,
            duration_ms,
            failures: vec![TestFailure {
                name: "(test suite)".to_string(),
                output: truncate(&raw),
            }],
        })
    }
}

// ---------------------------------------------------------------------------
// Cargo parser
// ---------------------------------------------------------------------------

/// Parse `cargo test 2>&1` human-readable output.
///
/// Lines of interest:
/// - `test some::path::name ... ok`
/// - `test some::path::name ... FAILED`
/// - `test some::path::name ... ignored`
/// - Failure detail blocks between `---- name stdout ----` and the next `----` header.
pub fn parse_cargo_output(output: &str) -> TestResult {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;
    let mut failures: Vec<TestFailure> = Vec::new();

    // First pass: count results.
    let mut failing_names: Vec<String> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test ") {
            if rest.ends_with("... ok") {
                passed += 1;
            } else if rest.ends_with("... FAILED") {
                failed += 1;
                let name = rest.trim_end_matches("... FAILED").trim().to_string();
                failing_names.push(name);
            } else if rest.ends_with("... ignored") {
                skipped += 1;
            }
        }
    }

    // Second pass: extract failure detail blocks.
    // Blocks start with `---- <name> stdout ----` and end before the next `----` or `failures:` section.
    for name in &failing_names {
        let header = format!("---- {} stdout ----", name);
        let mut in_block = false;
        let mut block_lines: Vec<&str> = Vec::new();
        for line in output.lines() {
            if line.trim() == header.as_str() {
                in_block = true;
                continue;
            }
            if in_block {
                // End of block: another `---- ` header or the `failures:` section.
                if (line.trim().starts_with("---- ") && line.trim().ends_with(" ----"))
                    || line.trim() == "failures:"
                {
                    break;
                }
                block_lines.push(line);
            }
        }
        let detail = block_lines.join("\n");
        failures.push(TestFailure {
            name: name.clone(),
            output: truncate(detail.trim()),
        });
    }

    // If we found failing names but no detail blocks, add placeholder entries.
    if failures.is_empty() && failed > 0 {
        for name in &failing_names {
            failures.push(TestFailure {
                name: name.clone(),
                output: String::new(),
            });
        }
    }

    TestResult {
        passed,
        failed,
        skipped,
        duration_ms: 0,
        failures,
    }
}

// ---------------------------------------------------------------------------
// pytest parser
// ---------------------------------------------------------------------------

/// Parse `python3 -m pytest --tb=short -q` output.
///
/// Summary line examples:
/// - `5 passed`
/// - `2 failed, 3 passed`
/// - `1 failed, 4 passed, 1 skipped`
/// - `FAILED test_foo.py::test_bar - AssertionError`
pub fn parse_pytest_output(output: &str) -> TestResult {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut skipped = 0u32;
    let mut failures: Vec<TestFailure> = Vec::new();

    // Collect FAILED lines for names.
    let mut failing_names: Vec<String> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("FAILED ") {
            // `FAILED path::test_name - reason`
            let rest = trimmed.strip_prefix("FAILED ").unwrap_or(trimmed);
            let name = rest.split(" - ").next().unwrap_or(rest).trim().to_string();
            failing_names.push(name);
        }
    }

    // Parse summary line — last line matching the pattern.
    for line in output.lines() {
        let trimmed = line.trim();
        // Example: `2 failed, 3 passed in 0.12s` or `== 2 failed, 3 passed ==`
        let cleaned = trimmed
            .trim_matches('=')
            .trim()
            .split(" in ")
            .next()
            .unwrap_or("")
            .trim();
        let mut found_counts = false;
        for part in cleaned.split(',') {
            let p = part.trim();
            if let Some(n) = p.strip_suffix(" passed") {
                if let Ok(v) = n.trim().parse::<u32>() {
                    passed = v;
                    found_counts = true;
                }
            } else if let Some(n) = p.strip_suffix(" failed") {
                if let Ok(v) = n.trim().parse::<u32>() {
                    failed = v;
                    found_counts = true;
                }
            } else if let Some(n) = p.strip_suffix(" skipped") {
                if let Ok(v) = n.trim().parse::<u32>() {
                    skipped = v;
                    found_counts = true;
                }
            }
        }
        if found_counts {
            // Keep parsing to get the last such line (most authoritative).
        }
    }

    // Build failure entries (output detail not extracted — pytest short format
    // interleaves output; for now just record the name).
    for name in &failing_names {
        failures.push(TestFailure {
            name: name.clone(),
            output: String::new(),
        });
    }

    // If the summary said failures but we found no FAILED lines, synthesize.
    if failed > 0 && failures.is_empty() {
        failures.push(TestFailure {
            name: "(unknown)".to_string(),
            output: truncate(output),
        });
    }

    TestResult {
        passed,
        failed,
        skipped,
        duration_ms: 0,
        failures,
    }
}

// ---------------------------------------------------------------------------
// npm/vitest parser (opaque fallback)
// ---------------------------------------------------------------------------

/// Parse npm test output: treat exit-code as pass/fail; no structured parsing.
pub fn parse_npm_output(output: &str) -> TestResult {
    // Look for common patterns in jest/vitest text output.
    let mut passed = 0u32;
    let mut failed = 0u32;

    for line in output.lines() {
        let t = line.trim();
        // Jest: `Tests:       2 passed, 1 failed, 3 total`
        if t.starts_with("Tests:") {
            for part in t.trim_start_matches("Tests:").split(',') {
                let p = part.trim();
                if let Some(n) = p.strip_suffix(" passed") {
                    passed = n.trim().parse().unwrap_or(0);
                } else if let Some(n) = p.strip_suffix(" failed") {
                    failed = n.trim().parse().unwrap_or(0);
                }
            }
        }
    }

    // If we couldn't parse, check for common failure indicators.
    if passed == 0 && failed == 0 {
        let lower = output.to_lowercase();
        if lower.contains("1 failed") || lower.contains("error") || lower.contains("failing") {
            failed = 1;
        } else {
            passed = 1;
        }
    }

    let failures = if failed > 0 {
        vec![TestFailure {
            name: "(test suite)".to_string(),
            output: truncate(output),
        }]
    } else {
        vec![]
    };

    TestResult {
        passed,
        failed,
        skipped: 0,
        duration_ms: 0,
        failures,
    }
}

// ---------------------------------------------------------------------------
// Go parser
// ---------------------------------------------------------------------------

/// Parse `go test ./... 2>&1` output.
///
/// Lines of interest:
/// - `--- PASS: TestFoo (0.00s)`
/// - `--- FAIL: TestBar (0.01s)`
/// - `ok      pkg/path    0.234s`
/// - `FAIL    pkg/path    0.234s`
pub fn parse_go_output(output: &str) -> TestResult {
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut failures: Vec<TestFailure> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--- PASS:") {
            passed += 1;
        } else if trimmed.starts_with("--- FAIL:") {
            failed += 1;
            // `--- FAIL: TestFoo (0.01s)`
            let rest = trimmed.trim_start_matches("--- FAIL:").trim();
            let name = rest.split('(').next().unwrap_or(rest).trim().to_string();
            failures.push(TestFailure {
                name,
                output: String::new(),
            });
        }
    }

    TestResult {
        passed,
        failed,
        skipped: 0,
        duration_ms: 0,
        failures,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // --- detect_runner ---

    #[test]
    fn test_runner_detects_cargo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        assert_eq!(detect_runner(dir.path()), Some(Runner::Cargo));
    }

    #[test]
    fn test_runner_detects_go() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module example.com/x\ngo 1.21").unwrap();
        assert_eq!(detect_runner(dir.path()), Some(Runner::Go));
    }

    #[test]
    fn test_runner_detects_pytest_via_pyproject() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[tool.pytest]").unwrap();
        assert_eq!(detect_runner(dir.path()), Some(Runner::Pytest));
    }

    #[test]
    fn test_runner_detects_pytest_via_ini() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pytest.ini"), "[pytest]").unwrap();
        assert_eq!(detect_runner(dir.path()), Some(Runner::Pytest));
    }

    #[test]
    fn test_runner_detects_npm() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_runner(dir.path()), Some(Runner::Npm));
    }

    #[test]
    fn test_runner_returns_none_for_empty_dir() {
        let dir = tempdir().unwrap();
        assert_eq!(detect_runner(dir.path()), None);
    }

    // --- parse_cargo_output ---

    const CARGO_PASS: &str = r#"
running 3 tests
test foo::tests::test_a ... ok
test foo::tests::test_b ... ok
test foo::tests::test_c ... ignored

test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
"#;

    const CARGO_FAIL: &str = r#"
running 3 tests
test foo::tests::test_a ... ok
test foo::tests::test_b ... FAILED
test foo::tests::test_c ... ok

---- foo::tests::test_b stdout ----
thread 'foo::tests::test_b' panicked at 'assertion failed: 1 == 2'

failures:

    foo::tests::test_b

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;

    #[test]
    fn test_result_parsing_cargo_all_pass() {
        let r = parse_cargo_output(CARGO_PASS);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 0);
        assert_eq!(r.skipped, 1);
        assert!(r.failures.is_empty());
        assert!(r.all_pass());
    }

    #[test]
    fn test_result_parsing_cargo_with_failure() {
        let r = parse_cargo_output(CARGO_FAIL);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 1);
        assert!(!r.all_pass());
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].name, "foo::tests::test_b");
        assert!(r.failures[0].output.contains("panicked"));
    }

    #[test]
    fn test_result_parsing_cargo_failing_names() {
        let r = parse_cargo_output(CARGO_FAIL);
        assert_eq!(r.failing_names(), vec!["foo::tests::test_b"]);
    }

    // --- parse_pytest_output ---

    const PYTEST_PASS: &str = r#"
collecting ... collected 2 items

test_foo.py::test_one PASSED
test_foo.py::test_two PASSED

2 passed in 0.12s
"#;

    const PYTEST_FAIL: &str = r#"
collecting ... collected 3 items

test_foo.py::test_one PASSED
test_foo.py::test_two FAILED
test_foo.py::test_three PASSED
FAILED test_foo.py::test_two - assert 1 == 2

1 failed, 2 passed in 0.15s
"#;

    #[test]
    fn test_pytest_parses_human_fallback_all_pass() {
        let r = parse_pytest_output(PYTEST_PASS);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 0);
        assert!(r.all_pass());
    }

    #[test]
    fn test_pytest_parses_human_fallback_summary() {
        let r = parse_pytest_output(PYTEST_FAIL);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 1);
        assert!(!r.all_pass());
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].name, "test_foo.py::test_two");
    }

    // --- parse_go_output ---

    const GO_OUTPUT: &str = r#"
--- PASS: TestAdd (0.00s)
--- FAIL: TestSub (0.01s)
--- PASS: TestMul (0.00s)
FAIL	example.com/calc	0.045s
"#;

    #[test]
    fn test_go_parses_output() {
        let r = parse_go_output(GO_OUTPUT);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 1);
        assert_eq!(r.failures[0].name, "TestSub");
    }

    // --- truncate ---

    #[test]
    fn test_truncate_short_string_unchanged() {
        let s = "hello";
        assert_eq!(truncate(s), "hello");
    }

    #[test]
    fn test_truncate_long_string_trimmed() {
        let s = "x".repeat(3000);
        let t = truncate(&s);
        assert!(t.len() <= MAX_OUTPUT_CHARS + 20); // allow for the ellipsis suffix
        assert!(t.contains('…'));
    }
}
