//! Integration tests: run clido binary for critical paths.

use std::process::Command;

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
    let out = clido_bin().arg("init").output().unwrap();
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
}
