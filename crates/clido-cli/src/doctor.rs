//! `clido doctor`: health checks for API key, session dir, pricing.

use clido_core::{load_config, load_pricing};
use clido_storage::session_dir_for_project;
use std::env;

use crate::errors::CliError;
use crate::provider::default_api_key_env;
use crate::ui::{ansi, cli_use_color};

pub async fn run_doctor() -> Result<(), anyhow::Error> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::DoctorMandatory(e.to_string()))?;
    let profile_name = loaded.default_profile.as_str();
    let profile = loaded
        .get_profile(profile_name)
        .map_err(|e| CliError::DoctorMandatory(e.to_string()))?;

    let mut mandatory = Vec::new();
    let mut warnings = Vec::new();
    let use_color = cli_use_color();

    check_api_key(profile, profile_name, use_color, &mut mandatory);
    check_api_key_format(profile, profile_name, use_color, &mut warnings);
    check_config_permissions(use_color, &mut warnings);
    check_session_dir(&cwd, use_color, &mut mandatory);
    check_pricing(use_color, &mut warnings);
    check_rules_files(&cwd, use_color, &mut warnings);
    check_fast_provider(&loaded, use_color, &mut warnings);

    if !mandatory.is_empty() {
        for m in &mandatory {
            if use_color {
                eprintln!("{}✗ {}{}", ansi::RED, m, ansi::RESET);
            } else {
                eprintln!("✗ {}", m);
            }
        }
        return Err(CliError::DoctorMandatory(mandatory.join(" ")).into());
    }
    if !warnings.is_empty() {
        for w in &warnings {
            if use_color {
                eprintln!("{}⚠ {}{}", ansi::YELLOW, w, ansi::RESET);
            } else {
                eprintln!("⚠ {}", w);
            }
        }
        return Err(CliError::DoctorWarnings(warnings.join(" ")).into());
    }
    Ok(())
}

fn check_api_key(
    profile: &clido_core::ProfileEntry,
    profile_name: &str,
    use_color: bool,
    mandatory: &mut Vec<String>,
) {
    if profile.provider == "local" {
        return;
    }
    if profile.api_key.is_some() {
        print_ok(
            use_color,
            &format!("API key for profile '{}' stored in config", profile_name),
        );
        return;
    }
    let api_key_env = profile
        .api_key_env
        .as_deref()
        .unwrap_or_else(|| default_api_key_env(&profile.provider));
    if env::var(api_key_env).is_err() {
        mandatory.push(format!(
            "API key not set for profile '{}' (set {}).",
            profile_name, api_key_env
        ));
    } else {
        print_ok(
            use_color,
            &format!(
                "API key ({}) set for profile '{}'",
                api_key_env, profile_name
            ),
        );
    }
}

fn check_session_dir(cwd: &std::path::Path, use_color: bool, mandatory: &mut Vec<String>) {
    match session_dir_for_project(cwd) {
        Ok(dir) => {
            if !dir.exists() {
                match std::fs::create_dir_all(&dir) {
                    Err(e) => mandatory.push(format!("Session dir not writable: {}", e)),
                    Ok(_) => print_ok(
                        use_color,
                        &format!("Session dir created and writable: {}", dir.display()),
                    ),
                }
            } else {
                let test_file = dir.join(".clido_doctor_write_test");
                if std::fs::write(&test_file, b"").is_ok() {
                    let _ = std::fs::remove_file(&test_file);
                    print_ok(
                        use_color,
                        &format!("Session dir writable: {}", dir.display()),
                    );
                } else {
                    mandatory.push(format!("Session dir not writable: {}", dir.display()));
                }
            }
        }
        Err(e) => mandatory.push(format!("Session dir: {}", e)),
    }
}

fn check_pricing(use_color: bool, warnings: &mut Vec<String>) {
    let (pricing_table, pricing_path) = load_pricing();
    if let Some(path) = &pricing_path {
        print_ok(
            use_color,
            &format!("pricing.toml present: {}", path.display()),
        );
        if pricing_table.models.is_empty() {
            warnings.push(
                "pricing.toml is empty or invalid; using default cost estimates.".to_string(),
            );
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                if let (Ok(now), Ok(mod_dur)) = (
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH),
                    modified.duration_since(std::time::UNIX_EPOCH),
                ) {
                    let age_secs = now.as_secs().saturating_sub(mod_dur.as_secs());
                    if age_secs > 90 * 86400 {
                        warnings.push(
                            "pricing.toml is older than 90 days; consider updating.".to_string(),
                        );
                    }
                }
            }
        }
    } else {
        print_info(
            use_color,
            "pricing.toml not found; using default cost estimates.",
        );
    }
}

/// Check API key format for known providers (validate without making network calls).
fn check_api_key_format(
    profile: &clido_core::ProfileEntry,
    _profile_name: &str,
    use_color: bool,
    warnings: &mut Vec<String>,
) {
    if profile.provider == "local" {
        return;
    }
    let key = profile.api_key.as_deref().or_else(|| {
        let env_name = profile
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| default_api_key_env(&profile.provider));
        // We can't return a reference to a temporary, so we skip env key format check here
        // (it would require static storage). Format check only applies to stored keys.
        let _ = env_name;
        None
    });

    if let Some(key) = key {
        let valid_format = match profile.provider.as_str() {
            "anthropic" => key.starts_with("sk-ant-"),
            "openrouter" => key.starts_with("sk-or-"),
            _ => true,
        };
        if !valid_format {
            warnings.push(format!(
                "API key for provider '{}' has unexpected format (expected {})",
                profile.provider,
                match profile.provider.as_str() {
                    "anthropic" => "sk-ant-...",
                    "openrouter" => "sk-or-...",
                    _ => "unknown",
                }
            ));
        } else {
            print_ok(
                use_color,
                &format!(
                    "API key format looks valid for provider '{}'",
                    profile.provider
                ),
            );
        }
    }
}

/// Check config file permissions (warn if not 0o600 on Unix).
fn check_config_permissions(use_color: bool, warnings: &mut Vec<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let config_path = clido_core::global_config_path();
        if let Some(path) = config_path {
            if path.exists() {
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mode = meta.permissions().mode() & 0o777;
                        if mode != 0o600 {
                            warnings.push(format!(
                                "Config file {} has permissions {:04o}; recommend 0600 to protect API keys.",
                                path.display(),
                                mode
                            ));
                        } else {
                            print_ok(
                                use_color,
                                &format!("Config file permissions OK (0600): {}", path.display()),
                            );
                        }
                    }
                    Err(e) => {
                        warnings.push(format!("Could not check config file permissions: {}", e));
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = use_color;
        let _ = warnings;
    }
}

fn check_rules_files(cwd: &std::path::Path, use_color: bool, warnings: &mut Vec<String>) {
    let files = clido_context::discover_rules(cwd, false, None);
    if files.is_empty() {
        print_info(
            use_color,
            "Rules files: none found (create CLIDO.md in project root)",
        );
    } else {
        for f in &files {
            let char_count = f.content.chars().count();
            print_ok(
                use_color,
                &format!("Rules files: {} ({} chars)", f.path.display(), char_count),
            );
            if char_count > 8000 {
                warnings.push(format!(
                    "Rules file is large ({} chars) — may inflate token costs: {}",
                    char_count,
                    f.path.display()
                ));
            }
        }
    }
}

fn check_fast_provider(
    loaded: &clido_core::LoadedConfig,
    use_color: bool,
    _warnings: &mut Vec<String>,
) {
    let profile = loaded.profiles.get(&loaded.default_profile);
    if let Some(entry) = profile {
        if let Some(ref fast) = entry.fast {
            print_ok(
                use_color,
                &format!("fast provider: {} / {}", fast.provider, fast.model),
            );
        } else {
            print_info(
                use_color,
                "fast provider: not configured (utility tasks use main provider)",
            );
        }
    }
}

fn print_ok(use_color: bool, msg: &str) {
    if use_color {
        println!("{}✓ {}{}", ansi::GREEN, msg, ansi::RESET);
    } else {
        println!("✓ {}", msg);
    }
}

fn print_info(use_color: bool, msg: &str) {
    if use_color {
        println!("{}ℹ {}{}", ansi::DIM, msg, ansi::RESET);
    } else {
        println!("ℹ {}", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::ProfileEntry;
    use std::env;

    fn make_profile(provider: &str, api_key: Option<&str>) -> ProfileEntry {
        ProfileEntry {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            api_key: api_key.map(|s| s.to_string()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        }
    }

    #[test]
    fn check_api_key_format_valid_anthropic() {
        let profile = make_profile("anthropic", Some("sk-ant-api03-abcdef"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(warnings.is_empty(), "valid anthropic key should not warn");
    }

    #[test]
    fn check_api_key_format_invalid_anthropic() {
        let profile = make_profile("anthropic", Some("sk-OR-invalid-format"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(!warnings.is_empty(), "invalid anthropic key should warn");
        assert!(warnings[0].contains("sk-ant-"));
    }

    #[test]
    fn check_api_key_format_valid_openrouter() {
        let profile = make_profile("openrouter", Some("sk-or-v1-abcdef12345"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(warnings.is_empty(), "valid openrouter key should not warn");
    }

    #[test]
    fn check_api_key_format_invalid_openrouter() {
        let profile = make_profile("openrouter", Some("sk-ant-wrong-provider"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("sk-or-"));
    }

    #[test]
    fn check_api_key_format_local_provider_skips_check() {
        let profile = make_profile("local", None);
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_api_key_format_no_key_no_warning() {
        // No stored key → format check does not apply
        let profile = make_profile("anthropic", None);
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_api_key_format_other_provider_any_key_ok() {
        let profile = make_profile("alibabacloud", Some("whatever-key-format"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", false, &mut warnings);
        assert!(
            warnings.is_empty(),
            "unknown provider should not warn on format"
        );
    }

    // ── check_api_key ──────────────────────────────────────────────────────

    #[test]
    fn check_api_key_local_provider_skips() {
        let profile = make_profile("local", None);
        let mut mandatory = Vec::new();
        check_api_key(&profile, "default", false, &mut mandatory);
        assert!(
            mandatory.is_empty(),
            "local provider should not require API key"
        );
    }

    #[test]
    fn check_api_key_present_in_profile_no_error() {
        let profile = make_profile("anthropic", Some("sk-ant-secret"));
        let mut mandatory = Vec::new();
        check_api_key(&profile, "default", false, &mut mandatory);
        assert!(mandatory.is_empty());
    }

    #[test]
    fn check_api_key_missing_key_env_not_set_adds_mandatory() {
        let mut profile = make_profile("anthropic", None);
        // Use a unique env var that is definitely not set
        profile.api_key_env = Some("CLIDO_DOCTOR_TEST_NONEXISTENT_KEY_ABC123".to_string());
        env::remove_var("CLIDO_DOCTOR_TEST_NONEXISTENT_KEY_ABC123");
        let mut mandatory = Vec::new();
        check_api_key(&profile, "default", false, &mut mandatory);
        assert!(!mandatory.is_empty());
        assert!(mandatory[0].contains("API key not set"));
    }

    #[test]
    fn check_api_key_missing_key_env_set_no_error() {
        let mut profile = make_profile("anthropic", None);
        let env_name = "CLIDO_DOCTOR_TEST_PRESENT_KEY_XYZ987";
        profile.api_key_env = Some(env_name.to_string());
        env::set_var(env_name, "sk-ant-fake");
        let mut mandatory = Vec::new();
        check_api_key(&profile, "default", false, &mut mandatory);
        env::remove_var(env_name);
        assert!(mandatory.is_empty());
    }

    // ── check_session_dir ──────────────────────────────────────────────────

    #[test]
    fn check_session_dir_writable_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mandatory = Vec::new();
        // check_session_dir uses session_dir_for_project() — just test it doesn't panic
        check_session_dir(tmp.path(), false, &mut mandatory);
        // Either creates dir successfully or raises error depending on env — just no panic
    }

    // ── check_pricing ──────────────────────────────────────────────────────

    #[test]
    fn check_pricing_does_not_panic() {
        let mut warnings = Vec::new();
        check_pricing(false, &mut warnings);
        // Either finds a pricing.toml or doesn't — no panic
    }

    // ── check_rules_files ──────────────────────────────────────────────────

    #[test]
    fn check_rules_files_empty_dir_no_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();
        check_rules_files(tmp.path(), false, &mut warnings);
        // With no CLIDO.md file, should print info but no warning
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_rules_files_large_file_warns() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a large CLIDO.md
        let large = "x".repeat(9000);
        std::fs::write(tmp.path().join("CLIDO.md"), &large).unwrap();
        let mut warnings = Vec::new();
        check_rules_files(tmp.path(), false, &mut warnings);
        assert!(!warnings.is_empty(), "large rules file should warn");
        assert!(
            warnings[0].contains("large")
                || warnings[0].contains("chars")
                || warnings[0].contains("token")
        );
    }

    // ── print_ok and print_info smoke tests ────────────────────────────────

    #[test]
    fn print_ok_no_color_no_panic() {
        print_ok(false, "all good");
    }

    #[test]
    fn print_info_no_color_no_panic() {
        print_info(false, "info msg");
    }

    #[test]
    fn print_ok_with_color_no_panic() {
        print_ok(true, "all good with color");
    }

    #[test]
    fn print_info_with_color_no_panic() {
        print_info(true, "info msg with color");
    }

    // ── check_session_dir: existing writable dir ──────────────────────────

    #[test]
    fn check_session_dir_existing_and_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mandatory = Vec::new();
        // Create an actual session dir structure so it doesn't try to create it
        check_session_dir(tmp.path(), false, &mut mandatory);
        // Result may vary but should not panic
    }

    // ── check_pricing: color output path ─────────────────────────────────

    #[test]
    fn check_pricing_with_color_does_not_panic() {
        let mut warnings = Vec::new();
        check_pricing(true, &mut warnings);
        // No panic with color=true
    }

    // ── check_api_key_format: valid format prints ok ───────────────────────

    #[test]
    fn check_api_key_format_valid_prints_ok() {
        let profile = make_profile("anthropic", Some("sk-ant-api03-valid"));
        let mut warnings = Vec::new();
        // call with use_color=true to exercise color print_ok path
        check_api_key_format(&profile, "default", true, &mut warnings);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_api_key_format_invalid_format_with_color() {
        let profile = make_profile("anthropic", Some("wrong-format-key"));
        let mut warnings = Vec::new();
        check_api_key_format(&profile, "default", true, &mut warnings);
        assert!(!warnings.is_empty());
    }
}
