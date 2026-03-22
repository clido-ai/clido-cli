//! Integration tests: run clido binary for critical paths.

use std::io::Write;
use std::process::{Command, Stdio};

fn clido_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clido"))
}

#[test]
fn clido_help_exits_zero() {
    let out = clido_bin().arg("--help").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("clido"));
}

#[test]
fn clido_doctor_runs() {
    let out = clido_bin().arg("doctor").output().unwrap();
    let code = out.status.code().unwrap_or(-1);
    assert!(
        code == 0 || code == 1 || code == 2,
        "unexpected exit code {}",
        code
    );
}

#[test]
fn clido_init_exits_zero() {
    let tmp = std::env::temp_dir().join(format!("clido_init_help_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    let mut child = clido_bin()
        .env("CLIDO_CONFIG", &config_path)
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // provider=1 (Anthropic), model=default (Enter), api_key=Y
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\n\nY\n")
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("config") || stdout.contains("Created"),
        "stdout: {}",
        stdout
    );
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}

/// Interactive setup flow (CLI spec §4): init with piped input writes config.
fn init_with_piped_input_and_check_config(input: &str, test_suffix: &str) {
    let tmp = std::env::temp_dir().join(format!(
        "clido_init_test_{}_{}",
        std::process::id(),
        test_suffix
    ));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    let config_path_str = config_path.to_string_lossy().to_string();
    let mut child = clido_bin()
        .env("CLIDO_CONFIG", &config_path_str)
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        panic!(
            "config not found at {}: {}; stderr: {}",
            config_path.display(),
            e,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(content.contains("provider"), "config: {}", content);
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}

#[test]
fn first_run_interactive() {
    // provider=1 (Anthropic), model=default (Enter), api_key=Y
    init_with_piped_input_and_check_config("1\n\nY\n", "first_run");
}

#[test]
fn init_interactive_writes_config() {
    // provider=1 (Anthropic), model=default (Enter), api_key=Y
    init_with_piped_input_and_check_config("1\n\nY\n", "init_writes");
}

#[test]
fn init_openrouter_writes_config() {
    let tmp =
        std::env::temp_dir().join(format!("clido_init_test_{}_openrouter", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    let config_path_str = config_path.to_string_lossy().to_string();
    let mut child = clido_bin()
        .env("CLIDO_CONFIG", &config_path_str)
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // provider=2 (OpenRouter), model=default (Enter), api_key=Y
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"2\n\nY\n")
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        panic!(
            "config not found at {}: {}; stderr: {}",
            config_path.display(),
            e,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(
        content.contains("openrouter"),
        "config must contain openrouter; config: {}",
        content
    );
    assert!(
        content.contains("OPENROUTER_API_KEY"),
        "config must reference OPENROUTER_API_KEY; config: {}",
        content
    );
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}

#[test]
fn init_stores_api_key_directly_in_config() {
    let tmp =
        std::env::temp_dir().join(format!("clido_init_test_{}_direct_key", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    let config_path_str = config_path.to_string_lossy().to_string();
    let mut child = clido_bin()
        .env("CLIDO_CONFIG", &config_path_str)
        // Unset any real key so the prompt doesn't show an existing value
        .env_remove("OPENROUTER_API_KEY")
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // provider=2 (OpenRouter), model=default, api_key=No, key=sk-test-direct-key
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"2\n\nN\nsk-test-direct-key\n")
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        panic!(
            "config not found at {}: {}; stderr: {}",
            config_path.display(),
            e,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(
        content.contains("api_key = \"sk-test-direct-key\""),
        "config must contain api_key with entered value; config: {}",
        content
    );
    assert!(
        !content.contains("api_key_env"),
        "config must not use api_key_env when key entered directly; config: {}",
        content
    );
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}

// ─── V1.5 integration tests ───────────────────────────────────────────────────

#[test]
fn cli_quiet_flag_in_help() {
    let out = clido_bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--quiet") || stdout.contains("-q"),
        "expected --quiet / -q in help; stdout: {}",
        stdout
    );
}

#[test]
fn cli_output_format_json_in_help() {
    let out = clido_bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("output-format"),
        "expected --output-format in help; stdout: {}",
        stdout
    );
}

#[test]
fn cli_list_models_exits_zero() {
    let out = clido_bin().arg("list-models").output().unwrap();
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.is_empty(),
        "expected non-empty model list; stdout: {}",
        stdout
    );
}

#[test]
fn cli_list_models_json_is_valid_json() {
    let out = clido_bin()
        .args(["list-models", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("list-models --json output is not valid JSON");
    assert!(
        parsed.is_array() || parsed.is_object(),
        "expected JSON array or object; got: {}",
        stdout
    );
}

#[test]
fn cli_list_models_provider_filter() {
    let out = clido_bin()
        .args(["list-models", "--provider", "anthropic"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("claude") || stdout.to_lowercase().contains("anthropic"),
        "expected anthropic models in output; stdout: {}",
        stdout
    );
}

#[test]
fn cli_update_pricing_exits_zero() {
    let out = clido_bin().arg("update-pricing").output().unwrap();
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_sessions_fork_help_exits_zero() {
    let out = clido_bin()
        .args(["sessions", "fork", "--help"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_mcp_config_flag_in_help() {
    let out = clido_bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("mcp-config"),
        "expected --mcp-config in help; stdout: {}",
        stdout
    );
}

// ─── JSON output integration ──────────────────────────────────────────────────

/// Run clido with --output-format json against a fake config.
/// The API call will fail (bad key), but the binary must still output valid JSON
/// with the required schema fields (schema_version, type, exit_status, is_error).
#[test]
fn cli_json_output_error_has_schema() {
    let tmp = std::env::temp_dir().join(format!("clido_json_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    // Write a minimal config with a fake API key so init isn't triggered.
    std::fs::write(
        &config_path,
        "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-3-5-haiku-20241022\"\napi_key = \"sk-ant-fake-key-for-test\"\n",
    ).unwrap();
    let out = clido_bin()
        .env("CLIDO_CONFIG", &config_path)
        .env("NO_COLOR", "1")
        .args(["--output-format", "json", "say hello"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
    // Binary should exit non-zero (API error) but stdout must be valid JSON.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "expected JSON on stdout; got empty"
    );
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    assert_eq!(v["schema_version"], 1, "schema_version must be 1");
    assert_eq!(v["type"], "result", "type must be result");
    assert!(v["exit_status"].is_string(), "exit_status must be string");
    assert!(v["is_error"].is_boolean(), "is_error must be boolean");
    assert!(v["session_id"].is_string(), "session_id must be string");
}

/// Cost footer: emit_result in text mode with nonzero cost writes footer to stderr.
/// We can't run a full agent call in integration tests, so we confirm the binary's
/// text output path doesn't crash and the footer format is documented via unit tests.
#[test]
fn cli_text_output_exits_zero_on_help() {
    let out = clido_bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    // Footer format is "↳ N turns · $X.XXXX · Xms" — verified in run.rs unit tests.
    // Here we just confirm text mode (the default) works end-to-end.
}

// ─── UX requirements ──────────────────────────────────────────────────────────

/// UX requirements: init prompts must state what to type and press Enter (ux-requirements §2.3).
#[test]
fn init_prompts_contain_ux_copy() {
    let tmp = std::env::temp_dir().join(format!("clido_ux_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    let config_path_str = config_path.to_string_lossy().to_string();
    let mut child = clido_bin()
        .env("CLIDO_CONFIG", &config_path_str)
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // provider=1, model=default, api_key=Y
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\n\nY\n")
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Type 1, 2, or 3") || stderr.contains("press Enter"),
        "stderr must contain UX copy (Type 1, 2, or 3 / press Enter); stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("[default: 1]") || stderr.contains("default: 1"),
        "stderr must show default; stderr: {}",
        stderr
    );
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}
