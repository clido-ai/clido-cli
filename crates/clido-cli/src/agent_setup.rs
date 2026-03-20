//! Shared agent setup: config loading, provider, registry, permissions.
//! Used by both the single-shot runner and the REPL to avoid duplication.

use clido_agent::AskUser;
use clido_core::{
    agent_config_from_loaded, load_config, load_pricing, AgentConfig, LoadedConfig, PermissionMode,
    PricingTable,
};
use clido_tools::{default_registry_with_blocked, ToolRegistry};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Arc;

use crate::cli::Cli;
use crate::errors::CliError;
use crate::provider::{make_provider, StdinAskUser};

pub struct AgentSetup {
    pub provider: Arc<dyn clido_providers::ModelProvider>,
    pub registry: ToolRegistry,
    pub config: AgentConfig,
    pub ask_user: Option<Arc<dyn AskUser>>,
    pub pricing_table: PricingTable,
}

impl AgentSetup {
    pub fn build(cli: &Cli, workspace_root: &Path) -> Result<Self, anyhow::Error> {
        let loaded = load_config(workspace_root).map_err(|e| CliError::Usage(e.to_string()))?;
        let (pricing_table, _) = load_pricing();
        let profile_name = cli
            .profile
            .as_deref()
            .unwrap_or(loaded.default_profile.as_str());
        let profile = loaded
            .get_profile(profile_name)
            .map_err(|e| CliError::Usage(e.to_string()))?;
        LoadedConfig::validate_provider(&profile.provider)
            .map_err(|e| CliError::Usage(e.to_string()))?;

        let provider = make_provider(
            profile_name,
            profile,
            cli.provider.as_deref(),
            cli.model.as_deref(),
        )
        .map_err(CliError::Usage)?;

        let registry = build_registry(cli, &loaded, workspace_root)?;

        let permission_mode = parse_permission_mode(cli.permission_mode.as_deref());

        let system_prompt = assemble_system_prompt(cli)?;

        let mut config = agent_config_from_loaded(
            &loaded,
            profile_name,
            Some(cli.max_turns),
            cli.max_budget_usd,
            cli.model.clone(),
            Some(system_prompt),
            Some(permission_mode),
        )
        .map_err(|e| CliError::Usage(e.to_string()))?;

        if config.max_context_tokens.is_none() {
            if let Some(entry) = pricing_table.models.get(&config.model) {
                if let Some(cw) = entry.context_window {
                    config.max_context_tokens = Some(cw);
                }
            }
        }

        let ask_user: Option<Arc<dyn AskUser>> =
            if permission_mode == PermissionMode::Default && io::stdin().is_terminal() {
                Some(Arc::new(StdinAskUser))
            } else {
                None
            };

        Ok(AgentSetup {
            provider,
            registry,
            config,
            ask_user,
            pricing_table,
        })
    }
}

/// Compute the clido config file path (mirrors setup.rs logic).
pub fn global_config_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        return Some(std::path::PathBuf::from(p));
    }
    directories::ProjectDirs::from("", "", "clido").map(|d| d.config_dir().join("config.toml"))
}

fn build_registry(
    cli: &Cli,
    loaded: &clido_core::LoadedConfig,
    workspace_root: &Path,
) -> Result<ToolRegistry, anyhow::Error> {
    let allowed = cli
        .allowed_tools
        .clone()
        .or_else(|| cli.tools.clone())
        .or_else(|| {
            if loaded.tools.allowed.is_empty() {
                None
            } else {
                Some(loaded.tools.allowed.join(","))
            }
        })
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    let disallowed = cli
        .disallowed_tools
        .clone()
        .or_else(|| {
            if loaded.tools.disallowed.is_empty() {
                None
            } else {
                Some(loaded.tools.disallowed.join(","))
            }
        })
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    // Block the config file from all tool access so its contents never leave the local system.
    let blocked = global_config_path().into_iter().collect::<Vec<_>>();
    let registry = default_registry_with_blocked(workspace_root.to_path_buf(), blocked)
        .with_filters(allowed, disallowed);
    if registry.schemas().is_empty() {
        return Err(CliError::Usage(
            "No tools left after --allowed-tools/--disallowed-tools/--tools. Check your filters."
                .into(),
        )
        .into());
    }
    Ok(registry)
}

pub fn parse_permission_mode(s: Option<&str>) -> PermissionMode {
    match s {
        Some("plan") | Some("plan-only") => PermissionMode::PlanOnly,
        Some("accept-all") => PermissionMode::AcceptAll,
        _ => PermissionMode::Default,
    }
}

fn assemble_system_prompt(cli: &Cli) -> Result<String, anyhow::Error> {
    let base = if let Some(ref path) = cli.system_prompt_file {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("Failed to read system prompt file: {}", e)))?
    } else if let Some(ref s) = cli.system_prompt {
        s.clone()
    } else {
        "You are clido, an AI coding agent. \
         You help with software development tasks: reading, writing, editing, and running code. \
         Always refer to yourself as clido — never as Claude, GPT, Gemini, or any other model name. \
         Be concise and direct."
            .to_string()
    };
    Ok(if let Some(ref append) = cli.append_system_prompt {
        format!("{}\n{}", base, append)
    } else {
        base
    })
}
