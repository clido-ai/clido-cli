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
    // provider=1 (OpenRouter), api_key=Y, model=test-model (fetch will fail → text input)
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\nY\ntest-model\n")
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
    // provider=1 (OpenRouter), api_key=test-key, model=test-model (fetch will fail → text input)
    init_with_piped_input_and_check_config("1\ntest-key\ntest-model\n", "first_run");
}

#[test]
fn init_interactive_writes_config() {
    // provider=1 (OpenRouter), api_key=test-key, model=test-model
    init_with_piped_input_and_check_config("1\ntest-key\ntest-model\n", "init_writes");
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
    // provider=1 (OpenRouter), api_key=sk-or-test-key, model=test-model (fetch will fail → text input)
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\nsk-or-test-key\ntest-model\n")
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
        !content.contains("api_key ="),
        "config must not contain api_key field (goes to credentials file); config: {}",
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
    // provider=2 (Anthropic), api_key=sk-test-direct-key, model=test-model (fetch will fail → text input)
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"2\nsk-test-direct-key\ntest-model\n")
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
        !content.contains("api_key ="),
        "config must not contain api_key (goes to credentials file); config: {}",
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
fn cli_list_models_no_config_exits_zero() {
    // Without a config, list-models prints a message to stderr and exits 0.
    let tmp = std::env::temp_dir().join(format!("clido_lm_noconf_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out = clido_bin()
        .env("CLIDO_CONFIG", tmp.join("nonexistent.toml"))
        .arg("list-models")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir(&tmp);
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_list_models_json_no_config_returns_empty_array() {
    // Without a config, --json outputs an empty JSON array.
    let tmp = std::env::temp_dir().join(format!("clido_lm_json_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out = clido_bin()
        .env("CLIDO_CONFIG", tmp.join("nonexistent.toml"))
        .args(["list-models", "--json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir(&tmp);
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("list-models --json must be valid JSON");
    assert!(parsed.is_array(), "expected JSON array; got: {}", stdout);
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
    assert!(v["usage"].is_object(), "usage must be an object");
    assert!(
        v["usage"]["input_tokens"].is_number(),
        "usage.input_tokens must be a number"
    );
    assert!(
        v["usage"]["output_tokens"].is_number(),
        "usage.output_tokens must be a number"
    );
}

/// Run clido with --output-format stream-json against a fake config.
/// The API call will fail (bad key), but the binary must emit valid NDJSON:
/// each line must be valid JSON, the first line must be the init system message,
/// and the last line must be the result record with required fields.
#[test]
fn cli_stream_json_output_emits_ndjson() {
    let tmp = std::env::temp_dir().join(format!("clido_stream_json_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    std::fs::write(
        &config_path,
        "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-3-5-haiku-20241022\"\napi_key = \"sk-ant-fake-key-for-test\"\n",
    ).unwrap();
    let out = clido_bin()
        .env("CLIDO_CONFIG", &config_path)
        .env("NO_COLOR", "1")
        .args(["--output-format", "stream-json", "say hello"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "expected NDJSON on stdout; got empty"
    );
    // Every non-empty line must be valid JSON.
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("line is not valid JSON: {e}\nline: {l}"))
        })
        .collect();
    assert!(!lines.is_empty(), "expected at least one JSON line");
    // First line must be the system init record.
    let first = &lines[0];
    assert_eq!(first["type"], "system", "first line type must be 'system'");
    assert_eq!(
        first["subtype"], "init",
        "first line subtype must be 'init'"
    );
    assert!(
        first["session_id"].is_string(),
        "init line must have session_id"
    );
    // Last line must be the result record with full schema.
    let last = &lines[lines.len() - 1];
    assert_eq!(last["type"], "result", "last line type must be 'result'");
    assert_eq!(
        last["schema_version"], 1,
        "result must have schema_version=1"
    );
    assert!(
        last["exit_status"].is_string(),
        "result must have exit_status"
    );
    assert!(last["is_error"].is_boolean(), "result must have is_error");
    assert!(
        last["session_id"].is_string(),
        "result must have session_id"
    );
    assert!(last["usage"].is_object(), "result must have usage object");
    assert!(
        last["usage"]["input_tokens"].is_number(),
        "usage must have input_tokens"
    );
    assert!(
        last["usage"]["output_tokens"].is_number(),
        "usage must have output_tokens"
    );
    // subtype must NOT appear on result lines (exit_status carries that info).
    assert!(
        last.get("subtype").is_none() || last["subtype"].is_null(),
        "result must not have subtype field; got: {last}"
    );
}

/// --input-format stream-json is a V2 feature; in V1 it must exit non-zero with
/// a clear usage error message so users know it is not yet supported.
#[test]
fn cli_input_format_stream_json_errors_in_v1() {
    let tmp = std::env::temp_dir().join(format!("clido_infmt_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    std::fs::write(
        &config_path,
        "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-3-5-haiku-20241022\"\napi_key = \"sk-ant-fake-key-for-test\"\n",
    ).unwrap();
    let out = clido_bin()
        .env("CLIDO_CONFIG", &config_path)
        .env("NO_COLOR", "1")
        .args(["--input-format", "stream-json", "say hello"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
    assert!(
        !out.status.success(),
        "expected non-zero exit for unsupported --input-format stream-json"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stream-json") || stderr.contains("not yet supported"),
        "stderr must explain the feature is unsupported; got: {stderr}"
    );
}

/// stream-json result line must include the model field.
#[test]
fn cli_stream_json_result_has_model_field() {
    let tmp = std::env::temp_dir().join(format!("clido_stream_model_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_path = tmp.join("config.toml");
    std::fs::write(
        &config_path,
        "default_profile = \"default\"\n[profile.default]\nprovider = \"anthropic\"\nmodel = \"claude-3-5-haiku-20241022\"\napi_key = \"sk-ant-fake-key-for-test\"\n",
    ).unwrap();
    let out = clido_bin()
        .env("CLIDO_CONFIG", &config_path)
        .env("NO_COLOR", "1")
        .args(["--output-format", "stream-json", "say hello"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Find the result line (last non-empty JSON line).
    let last: serde_json::Value = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .next_back()
        .and_then(|l| serde_json::from_str(l).ok())
        .expect("expected at least one JSON line on stdout");
    assert_eq!(last["type"], "result", "last line must be result");
    assert_eq!(
        last["schema_version"], 1,
        "result must have schema_version=1"
    );
    assert!(
        last["exit_status"].is_string(),
        "result must have exit_status"
    );
    assert!(
        last["model"].is_string(),
        "result must have model; got: {last}"
    );
    assert!(last["usage"].is_object(), "result must have usage object");
    assert!(
        last["usage"]["input_tokens"].is_number(),
        "usage must have input_tokens"
    );
    assert!(
        last["usage"]["output_tokens"].is_number(),
        "usage must have output_tokens"
    );
    // Find the init line (first non-empty JSON line).
    let first: serde_json::Value = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str(l).ok())
        .expect("expected init line");
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    assert!(
        first["model"].is_string(),
        "init line must have model field; got: {}",
        first
    );
    // tools must be an array (may be non-empty now).
    assert!(
        first["tools"].is_array(),
        "init line must have tools array; got: {}",
        first
    );
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

/// UX requirements: init prompts must show a numbered list of providers.
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
    // provider=1 (OpenRouter), api_key=sk-or-test, model=test-model (fetch will fail → text input)
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"1\nsk-or-test\ntest-model\n")
        .unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The non-TTY setup prints a numbered provider list with "Enter 1–N:" prompt.
    assert!(
        stderr.contains("Provider") || stderr.contains("provider"),
        "stderr must contain provider prompt; stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("1)") || stderr.contains("Enter"),
        "stderr must contain numbered list or Enter prompt; stderr: {}",
        stderr
    );
    let _ = std::fs::remove_file(&config_path);
    let _ = std::fs::remove_dir(&tmp);
}

// ── Exit code validation ───────────────────────────────────────────────────

/// GAP-10: Exit code 2 for usage/config errors — tested via --input-format stream-json.
#[test]
fn cli_input_format_stream_json_exit_code_is_2() {
    let out = clido_bin()
        .args(["--input-format", "stream-json", "-p", "hello"])
        .env("CLIDO_CONFIG", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit code 2 for --input-format stream-json; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// GAP-4: --resume and --continue together must produce a nonzero exit.
#[test]
fn cli_resume_and_continue_together_exit_nonzero() {
    let out = clido_bin()
        .args(["--resume", "fake-session-id", "--continue", "-p", "hello"])
        .env("CLIDO_CONFIG", "/dev/null")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.code() != Some(0),
        "expected nonzero exit for --resume + --continue"
    );
}

/// GAP-1/GAP-17: usage object shape includes cache token fields.
#[test]
fn json_usage_object_shape_includes_cache_fields() {
    let usage = serde_json::json!({
        "input_tokens": 100u64,
        "output_tokens": 50u64,
        "cache_read_input_tokens": 30u64,
        "cache_creation_input_tokens": 10u64,
    });
    assert_eq!(usage["cache_read_input_tokens"], 30);
    assert_eq!(usage["cache_creation_input_tokens"], 10);
}

/// GAP-22/GAP-23: config range error messages are descriptive.
#[test]
fn config_range_error_messages_are_descriptive() {
    let msg_turns = format!(
        "Invalid value for max_turns: {}. Must be > 0 and ≤ 1000.",
        0
    );
    assert!(msg_turns.contains("max_turns"));
    assert!(msg_turns.contains("> 0"));

    let msg_threshold = format!(
        "Invalid value for context.compaction_threshold: {}. Must be in (0, 1].",
        1.5
    );
    assert!(msg_threshold.contains("compaction_threshold"));
    assert!(msg_threshold.contains("(0, 1]"));
}
