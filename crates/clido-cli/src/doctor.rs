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
        let config_path = crate::agent_setup::global_config_path();
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
