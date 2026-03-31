//! `clido profile` subcommands: list, create, switch, edit, delete.

use clido_core::{
    delete_profile_from_config, global_config_path, load_config, switch_active_profile,
};

use crate::cli::ProfileCmd;
use crate::errors::CliError;
use crate::ui::{ansi, cli_use_color};

pub async fn run_profile(cmd: &ProfileCmd) -> Result<(), anyhow::Error> {
    match cmd {
        ProfileCmd::List => run_profiles_list(),
        ProfileCmd::Create { name } => run_profile_create(name.clone()).await,
        ProfileCmd::Switch { name } => run_profile_switch(name),
        ProfileCmd::Edit { name } => run_profile_edit(name.clone()).await,
        ProfileCmd::Delete { name, yes } => run_profile_delete(name, *yes),
    }
}

// ── List ──────────────────────────────────────────────────────────────────────

fn run_profiles_list() -> Result<(), anyhow::Error> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::Config(e.to_string()))?;
    let use_color = cli_use_color();

    let mut names: Vec<&String> = loaded.profiles.keys().collect();
    names.sort();

    println!();
    for name in &names {
        let entry = &loaded.profiles[*name];
        let is_active = *name == &loaded.default_profile;
        let marker = if is_active { "▶" } else { " " };
        let model_display = format!("{} / {}", entry.provider, entry.model);

        if use_color {
            if is_active {
                println!(
                    "{} {} {}{}{}  {}",
                    marker,
                    ansi::CYAN,
                    name,
                    ansi::RESET,
                    ansi::DARK_GRAY,
                    model_display,
                );
                print!("{}", ansi::RESET);
            } else {
                println!(
                    "{}  {}{}  {}{}",
                    marker,
                    ansi::WHITE,
                    name,
                    ansi::DARK_GRAY,
                    model_display,
                );
                print!("{}", ansi::RESET);
            }
        } else {
            println!("{} {}  {}", marker, name, model_display);
        }

        // Show fast provider if configured
        if let Some(ref f) = entry.fast {
            println!("     fast      {} / {}", f.provider, f.model);
        }
    }
    println!();

    if use_color {
        println!(
            "{}Active: {}{}{}",
            ansi::DARK_GRAY,
            ansi::CYAN,
            loaded.default_profile,
            ansi::RESET
        );
    } else {
        println!("Active: {}", loaded.default_profile);
    }
    println!();
    Ok(())
}

// ── Switch ────────────────────────────────────────────────────────────────────

fn run_profile_switch(name: &str) -> Result<(), anyhow::Error> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::Config(e.to_string()))?;

    if !loaded.profiles.contains_key(name) {
        let mut names: Vec<&String> = loaded.profiles.keys().collect();
        names.sort();
        return Err(CliError::Usage(format!(
            "Profile '{}' not found. Available: {}",
            name,
            names
                .iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .into());
    }

    if name == loaded.default_profile {
        println!("Profile '{}' is already active.", name);
        return Ok(());
    }

    let config_path = global_config_path()
        .ok_or_else(|| CliError::Usage("Cannot determine config path.".into()))?;

    switch_active_profile(&config_path, name).map_err(|e| CliError::Config(e.to_string()))?;

    let use_color = cli_use_color();
    if use_color {
        println!(
            "{}Switched to profile '{}'.{}",
            ansi::GREEN,
            name,
            ansi::RESET
        );
    } else {
        println!("Switched to profile '{}'.", name);
    }
    Ok(())
}

// ── Delete ────────────────────────────────────────────────────────────────────

fn run_profile_delete(name: &str, yes: bool) -> Result<(), anyhow::Error> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::Config(e.to_string()))?;

    if !loaded.profiles.contains_key(name) {
        return Err(CliError::Usage(format!("Profile '{}' not found.", name)).into());
    }

    if name == loaded.default_profile {
        return Err(CliError::Usage(format!(
            "Cannot delete the active profile '{}'. Switch to another profile first.",
            name
        ))
        .into());
    }

    if !yes {
        use std::io::Write;
        print!("Delete profile '{}'? [y/N] ", name);
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let config_path = global_config_path()
        .ok_or_else(|| CliError::Usage("Cannot determine config path.".into()))?;

    delete_profile_from_config(&config_path, name).map_err(|e| CliError::Config(e.to_string()))?;

    let use_color = cli_use_color();
    if use_color {
        println!("{}Deleted profile '{}'.{}", ansi::GREEN, name, ansi::RESET);
    } else {
        println!("Deleted profile '{}'.", name);
    }
    Ok(())
}

// ── Create ────────────────────────────────────────────────────────────────────

pub async fn run_profile_create(name: Option<String>) -> Result<(), anyhow::Error> {
    crate::setup::run_create_profile(name).await
}

// ── Edit ──────────────────────────────────────────────────────────────────────

pub async fn run_profile_edit(name: String) -> Result<(), anyhow::Error> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(&cwd).map_err(|e| CliError::Config(e.to_string()))?;

    let entry = loaded
        .profiles
        .get(&name)
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("Profile '{}' not found.", name)))?;

    crate::setup::run_edit_profile(name, entry).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_cmd_list_variant_is_list() {
        let cmd = ProfileCmd::List;
        assert!(matches!(cmd, ProfileCmd::List));
    }

    #[test]
    fn profile_cmd_switch_has_name() {
        let cmd = ProfileCmd::Switch {
            name: "work".to_string(),
        };
        if let ProfileCmd::Switch { name } = cmd {
            assert_eq!(name, "work");
        } else {
            panic!("expected Switch");
        }
    }

    #[test]
    fn profile_cmd_delete_yes_flag() {
        let cmd = ProfileCmd::Delete {
            name: "old".to_string(),
            yes: true,
        };
        if let ProfileCmd::Delete { name, yes } = cmd {
            assert_eq!(name, "old");
            assert!(yes);
        } else {
            panic!("expected Delete");
        }
    }

    #[test]
    fn profile_cmd_create_no_name() {
        let cmd = ProfileCmd::Create { name: None };
        if let ProfileCmd::Create { name } = cmd {
            assert!(name.is_none());
        } else {
            panic!("expected Create");
        }
    }
}
