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
