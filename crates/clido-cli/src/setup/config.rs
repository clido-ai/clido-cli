//! TOML config building and credentials file helpers.

use clido_providers::registry::PROVIDER_REGISTRY;

use super::types::SetupState;

pub(super) fn build_full_config_toml(s: &SetupState) -> String {
    build_toml_impl(s, None)
}

/// Alias used by tests.
#[cfg(test)]
pub(super) fn build_toml(s: &SetupState) -> String {
    build_full_config_toml(s)
}

/// Internal TOML builder.
/// - `profile_name = None` → full config (first-run / /init).
/// - `profile_name = Some(name)` → only the `[profile.<name>]` block (profile wizard).
fn build_toml_impl(s: &SetupState, profile_name: Option<&str>) -> String {
    let provider = PROVIDER_REGISTRY[s.provider].id;

    if let Some(pname) = profile_name {
        // ── Profile mode: generate only [profile.<name>] and sub-agent blocks ──
        let main_key_line = if s.is_local() {
            let base_url = if s.credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else {
            String::new()
        };
        let mut out = if s.is_local() {
            let base_url = if s.credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.credential.as_str()
            };
            format!(
                "[profile.{}]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n",
                pname, s.model, base_url
            )
        } else {
            format!(
                "[profile.{}]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, provider, s.model, main_key_line
            )
        };
        if s.configure_worker && !s.worker_model.is_empty() {
            let worker_prov = PROVIDER_REGISTRY[s.worker_provider].id;
            let is_local_worker = PROVIDER_REGISTRY[s.worker_provider].is_local;
            let worker_key_line = if is_local_worker {
                let base_url = if s.worker_credential.is_empty() {
                    "http://localhost:11434"
                } else {
                    s.worker_credential.as_str()
                };
                format!("base_url = \"{}\"\n", base_url)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n[profile.{}.worker]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, worker_prov, s.worker_model, worker_key_line
            ));
        }
        if s.configure_reviewer && !s.reviewer_model.is_empty() {
            let reviewer_prov = PROVIDER_REGISTRY[s.reviewer_provider].id;
            let is_local_reviewer = PROVIDER_REGISTRY[s.reviewer_provider].is_local;
            let reviewer_key_line = if is_local_reviewer {
                let base_url = if s.reviewer_credential.is_empty() {
                    "http://localhost:11434"
                } else {
                    s.reviewer_credential.as_str()
                };
                format!("base_url = \"{}\"\n", base_url)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n[profile.{}.reviewer]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, reviewer_prov, s.reviewer_model, reviewer_key_line
            ));
        }
        return out;
    }

    // ── Full config mode (first-run / /init) ──────────────────────────────────
    let roles_toml = if s.roles.is_empty() {
        String::new()
    } else {
        let mut t = "\n[roles]\n".to_string();
        for (name, model) in &s.roles {
            t.push_str(&format!("{} = \"{}\"\n", name, model));
        }
        t
    };

    let profile_toml = if s.is_local() {
        let base_url = if s.credential.is_empty() {
            "http://localhost:11434"
        } else {
            s.credential.as_str()
        };
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n{}",
            s.model, base_url, roles_toml
        )
    } else {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"{}\"\nmodel = \"{}\"\n# API keys are stored in the credentials file (same directory as this file).\n{}",
            provider, s.model, roles_toml
        )
    };

    // Build [agents] section
    let main_key_line = if s.is_local() {
        let base_url = if s.credential.is_empty() {
            "http://localhost:11434"
        } else {
            s.credential.as_str()
        };
        format!("base_url = \"{}\"\n", base_url)
    } else {
        String::new()
    };

    let mut agents_toml = format!(
        "\n[agents.main]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
        provider, s.model, main_key_line
    );

    if s.configure_worker && !s.worker_model.is_empty() {
        let worker_prov = PROVIDER_REGISTRY[s.worker_provider].id;
        let is_local_worker = PROVIDER_REGISTRY[s.worker_provider].is_local;
        let worker_key_line = if is_local_worker {
            let base_url = if s.worker_credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.worker_credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else {
            String::new()
        };
        agents_toml.push_str(&format!(
            "\n[agents.worker]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
            worker_prov, s.worker_model, worker_key_line
        ));
    }

    if s.configure_reviewer && !s.reviewer_model.is_empty() {
        let reviewer_prov = PROVIDER_REGISTRY[s.reviewer_provider].id;
        let is_local_reviewer = PROVIDER_REGISTRY[s.reviewer_provider].is_local;
        let reviewer_key_line = if is_local_reviewer {
            let base_url = if s.reviewer_credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.reviewer_credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else {
            String::new()
        };
        agents_toml.push_str(&format!(
            "\n[agents.reviewer]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
            reviewer_prov, s.reviewer_model, reviewer_key_line
        ));
    }

    format!("{}{}", profile_toml, agents_toml)
}

// ── Credentials file helpers ──────────────────────────────────────────────────

/// Path to the credentials file alongside the given config file.
pub(super) fn credentials_path(config_path: &std::path::Path) -> std::path::PathBuf {
    config_path
        .parent()
        .unwrap_or(config_path)
        .join("credentials")
}

/// Write API keys to `path` in TOML `[keys]` format with chmod 600.
/// Entries with empty keys are skipped. Duplicate provider IDs: last wins.
pub(super) fn write_credentials_file(
    path: &std::path::Path,
    entries: &[(String, String)],
) -> std::io::Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut content = String::from(
        "# This file contains sensitive API keys. Keep it private (chmod 600).\n\
         # Do not share or commit this file.\n\
         \n\
         [keys]\n",
    );
    for (provider_id, key) in entries {
        if key.is_empty() || !seen.insert(provider_id.clone()) {
            continue;
        }
        content.push_str(&format!("{} = \"{}\"\n", provider_id, key));
    }
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
    Ok(())
}

/// Collect all non-empty, non-local API key credentials from a setup state.
/// Returns `(provider_id, key)` pairs for main, worker, and reviewer agents.
pub(super) fn collect_credentials_from_state(s: &SetupState) -> Vec<(String, String)> {
    let mut creds: Vec<(String, String)> = Vec::new();
    if !s.is_local() && !s.credential.is_empty() {
        creds.push((
            PROVIDER_REGISTRY[s.provider].id.to_string(),
            s.credential.clone(),
        ));
    }
    if s.configure_worker && !s.worker_model.is_empty() {
        let is_local_worker = PROVIDER_REGISTRY[s.worker_provider].is_local;
        if !is_local_worker && !s.worker_credential.is_empty() {
            creds.push((
                PROVIDER_REGISTRY[s.worker_provider].id.to_string(),
                s.worker_credential.clone(),
            ));
        }
    }
    if s.configure_reviewer && !s.reviewer_model.is_empty() {
        let is_local_reviewer = PROVIDER_REGISTRY[s.reviewer_provider].is_local;
        if !is_local_reviewer && !s.reviewer_credential.is_empty() {
            creds.push((
                PROVIDER_REGISTRY[s.reviewer_provider].id.to_string(),
                s.reviewer_credential.clone(),
            ));
        }
    }
    creds
}

/// Build a `ProfileEntry` from the wizard state.
pub(super) fn state_to_profile_entry(s: &SetupState) -> clido_core::ProfileEntry {
    let provider = PROVIDER_REGISTRY[s.provider].id;
    let (api_key, base_url) = if s.is_local() {
        let url = if s.credential.is_empty() {
            Some("http://localhost:11434".to_string())
        } else {
            Some(s.credential.clone())
        };
        (None, url)
    } else {
        (Some(s.credential.clone()).filter(|k| !k.is_empty()), None)
    };

    // Build fast provider config if the user opted to configure one.
    let fast = if s.configure_worker && !s.worker_model.is_empty() {
        let fast_prov = PROVIDER_REGISTRY[s.worker_provider].id;
        let is_local = PROVIDER_REGISTRY[s.worker_provider].is_local;
        let (f_key, f_url) = if is_local {
            let url = if s.worker_credential.is_empty() {
                Some("http://localhost:11434".to_string())
            } else {
                Some(s.worker_credential.clone())
            };
            (None, url)
        } else {
            (
                Some(s.worker_credential.clone()).filter(|k| !k.is_empty()),
                None,
            )
        };
        Some(clido_core::FastProviderConfig {
            provider: fast_prov.to_string(),
            model: s.worker_model.clone(),
            api_key: f_key,
            api_key_env: None,
            base_url: f_url,
            user_agent: None,
        })
    } else {
        None
    };

    clido_core::ProfileEntry {
        provider: provider.to_string(),
        model: s.model.clone(),
        api_key,
        api_key_env: None,
        base_url,
        user_agent: None,
        fast,
    }
}
