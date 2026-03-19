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
    child.stdin.as_mut().unwrap().write_all(b"1\nY\n").unwrap();
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

/// Interactive setup flow (CLI spec §4): init with piped input writes config. Same flow as first-run.
fn init_with_piped_input_and_check_config(provider_choice: &str, test_suffix: &str) {
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
        .write_all(provider_choice.as_bytes())
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
    init_with_piped_input_and_check_config("1\nY\n", "first_run");
}

#[test]
fn init_interactive_writes_config() {
    init_with_piped_input_and_check_config("1\nY\n", "init_writes");
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
    child.stdin.as_mut().unwrap().write_all(b"1\nY\n").unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Type 1 or 2") || stderr.contains("press Enter"),
        "stderr must contain UX copy (Type 1 or 2 / press Enter); stderr: {}",
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
