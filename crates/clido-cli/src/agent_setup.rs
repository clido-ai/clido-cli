//! Shared agent setup: config loading, provider, registry, permissions.
//! Used by both the single-shot runner and the REPL to avoid duplication.

use clido_agent::AskUser;
use clido_core::{
    agent_config_from_loaded, load_config, load_pricing, AgentConfig, LoadedConfig, PermissionMode,
    PricingTable,
};
use clido_providers::RetryProvider;
use clido_tools::{default_registry_with_todo_store, McpTool, TodoItem, ToolRegistry};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::cli::Cli;
use crate::errors::CliError;

use crate::provider::{default_api_key_env, load_credentials, make_provider, StdinAskUser};
use crate::spawn_tools::{SpawnReviewerTool, SpawnWorkerTool};

type TodoStore = std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>;

/// Detect context window size from model name.
/// Falls back to safe defaults when model is not recognized.
fn detect_context_window(model: &str) -> u32 {
    let model_lower = model.to_lowercase();
    
    // kimi (Moonshot AI) models
    if model_lower.contains("kimi") {
        if model_lower.contains("k2") || model_lower.contains("-k2-") {
            return 256_000; // kimi-k2 has 256k context
        }
        if model_lower.contains("k1.5") || model_lower.contains("-k1.5-") {
            return 128_000; // kimi-k1.5 has 128k context
        }
        // Default for other kimi models (k1, etc.)
        return 128_000;
    }
    
    // Cerebras models
    if model_lower.contains("cerebras") {
        return 128_000; // Cerebras models typically have 128k
    }
    
    // DeepSeek models
    if model_lower.contains("deepseek") {
        if model_lower.contains("coder") || model_lower.contains("chat") {
            return 128_000; // DeepSeek v2/v3 have 128k
        }
        return 64_000; // Older DeepSeek models have 64k
    }
    
    // Mistral models
    if model_lower.contains("mistral") {
        if model_lower.contains("large") || model_lower.contains("medium") {
            return 128_000;
        }
        return 32_000; // Mistral small/tiny have 32k
    }
    
    // Llama models
    if model_lower.contains("llama") {
        if model_lower.contains("3") || model_lower.contains("-3-") {
            return 128_000; // Llama 3 has 128k
        }
        return 8_192; // Llama 2 has 4k/8k
    }
    
    // Gemma models
    if model_lower.contains("gemma") {
        return 8_192; // Gemma typically has 8k
    }
    
    // Phi models
    if model_lower.contains("phi") {
        return 128_000; // Phi-3/4 have 128k
    }
    
    // Qwen models
    if model_lower.contains("qwen") {
        if model_lower.contains("2.5") || model_lower.contains("-2.5-") {
            return 128_000; // Qwen 2.5 has 128k
        }
        if model_lower.contains("2") || model_lower.contains("-2-") {
            return 32_000; // Qwen 2 has 32k
        }
        return 128_000; // Default for newer Qwen
    }
    
    // Claude models (should be in pricing table, but just in case)
    if model_lower.contains("claude") {
        if model_lower.contains("3") {
            if model_lower.contains("opus") {
                return 200_000;
            }
            return 200_000; // Claude 3 Sonnet/Haiku have 200k
        }
        return 100_000; // Claude 2 has 100k
    }
    
    // GPT-4 models (should be in pricing table, but just in case)
    if model_lower.contains("gpt-4") || model_lower.contains("gpt4") {
        if model_lower.contains("32k") {
            return 32_768;
        }
        if model_lower.contains("o1") || model_lower.contains("o3") {
            return 200_000; // O1/O3 have large context
        }
        return 128_000; // GPT-4 Turbo and later have 128k
    }
    
    // GPT-3.5 models
    if model_lower.contains("gpt-3.5") || model_lower.contains("gpt3.5") {
        if model_lower.contains("16k") {
            return 16_384;
        }
        return 4_096; // Standard GPT-3.5 has 4k
    }
    
    // Gemini models (should be in pricing table, but just in case)
    if model_lower.contains("gemini") {
        if model_lower.contains("1.5") || model_lower.contains("-1.5-") {
            if model_lower.contains("flash") || model_lower.contains("pro") {
                return 1_000_000; // Gemini 1.5 has 1M context!
            }
        }
        return 32_768; // Older Gemini has 32k
    }
    
    // Groq hosted models often have limited context
    if model_lower.contains("groq") || model_lower.starts_with("llama-3") && model_lower.contains("groq") {
        return 8_192; // Groq often limits to 8k
    }
    
    // Safe default for unknown models - 128k is a common modern standard
    // This is safer than 200k which causes issues with many providers
    128_000
}

pub struct AgentSetup {
    pub provider: Arc<dyn clido_providers::ModelProvider>,
    /// Provider ID string (e.g. "kimi-code", "openrouter") — used for subscription checks.
    pub provider_name: String,
    pub registry: ToolRegistry,
    pub config: AgentConfig,
    pub ask_user: Option<Arc<dyn AskUser>>,
    pub pricing_table: PricingTable,
    /// Shared todo list written by the agent's TodoWrite tool.
    /// Shared todo list written by the agent's TodoWrite tool.
    #[allow(dead_code)]
    pub todo_store: TodoStore,
    /// Optional fast/cheap provider for utility tasks (summarization, title, commit, sub-agents).
    /// Built from `[profiles.<name>.fast]` config section. If None, main provider is used.
    pub fast_provider: Option<Arc<dyn clido_providers::ModelProvider>>,
    /// Config (model name, etc) for the fast provider. Only meaningful when fast_provider is Some.
    pub fast_config: Option<AgentConfig>,
}

impl AgentSetup {
    /// Build from caller-supplied config and pricing table, avoiding redundant disk reads.
    /// This is the canonical implementation; `build()` is a thin wrapper for callers that
    /// don't have pre-loaded data.
    ///
    /// `reviewer_enabled` is a shared flag the TUI uses to pause/resume the reviewer at runtime
    /// without restarting the agent.  Pass `Arc::new(AtomicBool::new(true))` when not needed.
    pub fn build_with_preloaded(
        cli: &Cli,
        workspace_root: &Path,
        loaded: LoadedConfig,
        pricing_table: PricingTable,
        reviewer_enabled: Arc<AtomicBool>,
    ) -> Result<Self, anyhow::Error> {
        Self::build_with_preloaded_and_store(
            cli,
            workspace_root,
            loaded,
            pricing_table,
            reviewer_enabled,
            None,
            &[],
        )
    }

    pub fn build_with_preloaded_and_store(
        cli: &Cli,
        workspace_root: &Path,
        loaded: LoadedConfig,
        pricing_table: PricingTable,
        reviewer_enabled: Arc<AtomicBool>,
        external_todo_store: Option<TodoStore>,
        allowed_external_paths: &[std::path::PathBuf],
    ) -> Result<Self, anyhow::Error> {
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

        let (mut registry, todo_store) = build_registry(
            cli,
            &loaded,
            workspace_root,
            external_todo_store,
            allowed_external_paths,
        )?;
        registry = load_mcp_tools(cli, registry);

        let permission_mode = parse_permission_mode(cli.permission_mode.as_deref());

        let mut system_prompt = assemble_system_prompt(cli)?;

        // Load project-specific context from .clido.md if present.
        let clido_md_path = workspace_root.join(".clido.md");
        if clido_md_path.is_file() {
            if let Ok(project_ctx) = std::fs::read_to_string(&clido_md_path) {
                let trimmed = project_ctx.trim();
                if !trimmed.is_empty() {
                    system_prompt = format!(
                        "<project_context>\n{}\n</project_context>\n\n{}",
                        trimmed, system_prompt
                    );
                }
            }
        }

        // Load skills from .clido/skills/ directory (project-local and global).
        {
            let mut skill_texts = Vec::new();
            let skills_dir = workspace_root.join(".clido").join("skills");
            if skills_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                    let mut paths: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| {
                            e.path()
                                .extension()
                                .map(|ext| ext == "md" || ext == "txt")
                                .unwrap_or(false)
                        })
                        .map(|e| e.path())
                        .collect();
                    paths.sort();
                    for path in paths {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let trimmed = content.trim();
                            if !trimmed.is_empty() {
                                let name = path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                skill_texts.push(format!("### {}\n{}", name, trimmed));
                            }
                        }
                    }
                }
            }
            // Also load from ~/.clido/skills/ (global skills).
            let global_skills_dir = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".clido").join("skills"))
                .ok();
            if let Some(ref global_skills) = global_skills_dir {
                if global_skills.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(global_skills) {
                        let mut paths: Vec<_> = entries
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.path()
                                    .extension()
                                    .map(|ext| ext == "md" || ext == "txt")
                                    .unwrap_or(false)
                            })
                            .map(|e| e.path())
                            .collect();
                        paths.sort();
                        for path in paths {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                let trimmed = content.trim();
                                if !trimmed.is_empty() {
                                    let name = path
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    // Avoid duplicates if project has same-named skill
                                    if !skill_texts
                                        .iter()
                                        .any(|s| s.starts_with(&format!("### {}\n", name)))
                                    {
                                        skill_texts.push(format!("### {}\n{}", name, trimmed));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !skill_texts.is_empty() {
                system_prompt = format!(
                    "{}\n\n<skills>\n{}\n</skills>",
                    system_prompt,
                    skill_texts.join("\n\n")
                );
            }
        }

        // Append provider-specific prompt instructions.
        let provider_name = profile.provider.as_str();
        let model_name = cli.model.as_deref().unwrap_or(&profile.model);
        let family =
            clido_agent::provider_prompts::ProviderFamily::detect(provider_name, model_name);
        let suffix = clido_agent::provider_prompts::provider_specific_instructions(family);
        if !suffix.is_empty() {
            system_prompt = format!("{}\n{}", system_prompt, suffix);
        }

        let mut config = agent_config_from_loaded(
            &loaded,
            profile_name,
            cli.max_turns,
            cli.max_budget_usd,
            cli.model.clone(),
            Some(system_prompt),
            Some(permission_mode),
            cli.quiet,
            cli.max_parallel_tools,
        )
        .map_err(|e| CliError::Usage(e.to_string()))?;

        // Build fast/utility provider from [profiles.<name>.fast] if configured.
        let (fast_provider, fast_config) = if let Some(ref fast_cfg) = profile.fast {
            match build_fast_provider(fast_cfg) {
                Ok(fp) => {
                    let mut fc = config.clone();
                    fc.model = fast_cfg.model.clone();
                    fc.system_prompt = None;
                    (Some(fp), Some(fc))
                }
                Err(e) => {
                    eprintln!("Warning: fast provider failed to build: {e}");
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        // Worker/reviewer sub-agents use fast provider (or main if fast not configured).
        let sub_provider: Arc<dyn clido_providers::ModelProvider> = fast_provider
            .clone()
            .unwrap_or_else(|| Arc::clone(&provider));
        let sub_model = profile
            .fast
            .as_ref()
            .map(|f| f.model.clone())
            .unwrap_or_else(|| config.model.clone());

        {
            let mut worker_config = config.clone();
            worker_config.system_prompt = None;
            worker_config.max_turns = worker_config.max_turns.min(20);
            worker_config.model = sub_model.clone();
            registry.register(SpawnWorkerTool::new(
                sub_provider.clone(),
                worker_config,
                workspace_root.to_path_buf(),
            ));
        }
        {
            let mut reviewer_config = config.clone();
            reviewer_config.system_prompt = None;
            reviewer_config.max_turns = reviewer_config.max_turns.min(10);
            reviewer_config.model = sub_model;
            registry.register(SpawnReviewerTool::new(
                sub_provider,
                reviewer_config,
                workspace_root.to_path_buf(),
                reviewer_enabled.clone(),
            ));
        }

        // Inject sub-agent routing instructions.
        {
            let routing = build_routing_instructions(true, true);
            if let Some(ref mut sp) = config.system_prompt {
                *sp = format!("{}\n\n{}", sp, routing);
            }
        }

        // Inject project rules into system prompt
        let rules_file_path = cli
            .rules_file
            .as_deref()
            .or_else(|| config.rules_file.as_ref().map(|s| Path::new(s.as_str())));
        let rules = clido_context::load_and_assemble_rules(
            workspace_root,
            cli.no_rules || config.no_rules,
            rules_file_path,
        );
        if !rules.is_empty() {
            if let Some(ref mut sp) = config.system_prompt {
                *sp = format!("{}\n\n{}", rules, sp);
            }
        }

        // Git context is no longer injected here as a one-shot static section.
        // Instead each AgentLoop consumer attaches a `git_context_fn` callback
        // (via `AgentLoop::with_git_context_fn`) so the context is refreshed on
        // every user turn, reflecting the current branch/status/log.

        if config.max_context_tokens.is_none() {
            // Try to get context window from pricing table
            if let Some(entry) = pricing_table.models.get(&config.model) {
                if let Some(cw) = entry.context_window {
                    config.max_context_tokens = Some(cw);
                }
            }
            // If still not set, try to detect from model name patterns
            if config.max_context_tokens.is_none() {
                config.max_context_tokens = Some(detect_context_window(&config.model));
            }
        }

        let ask_user: Option<Arc<dyn AskUser>> =
            if permission_mode == PermissionMode::Default && io::stdin().is_terminal() {
                Some(Arc::new(StdinAskUser))
            } else {
                None
            };

        // Wrap provider with retry logic for transient failures.
        let provider = RetryProvider::wrap(provider);

        Ok(AgentSetup {
            provider,
            provider_name: provider_name.to_string(),
            registry,
            config,
            ask_user,
            pricing_table,
            todo_store,
            fast_provider,
            fast_config,
        })
    }

    /// Convenience wrapper that loads config and pricing from disk.
    /// Prefer `build_with_preloaded` when the caller already has these.
    pub fn build(cli: &Cli, workspace_root: &Path) -> Result<Self, anyhow::Error> {
        let loaded = load_config(workspace_root)
            .map_err(|e| CliError::Usage(format!("load config: {e}")))?;
        let (pricing_table, _) = load_pricing();
        Self::build_with_preloaded(
            cli,
            workspace_root,
            loaded,
            pricing_table,
            Arc::new(AtomicBool::new(true)),
        )
    }
}

fn build_fast_provider(
    cfg: &clido_core::FastProviderConfig,
) -> Result<Arc<dyn clido_providers::ModelProvider>, String> {
    let api_key = cfg
        .api_key
        .clone()
        .or_else(|| {
            cfg.api_key_env
                .as_ref()
                .and_then(|e| std::env::var(e).ok().filter(|v| !v.is_empty()))
        })
        .or_else(|| {
            let env_var = default_api_key_env(&cfg.provider);
            if env_var.is_empty() {
                None
            } else {
                std::env::var(env_var).ok().filter(|v| !v.is_empty())
            }
        })
        .or_else(|| {
            let config_dir = if let Ok(p) = std::env::var("CLIDO_CONFIG") {
                std::path::Path::new(&p).parent().map(|p| p.to_path_buf())
            } else {
                directories::ProjectDirs::from("", "", "clido")
                    .map(|d| d.config_dir().to_path_buf())
            };
            config_dir
                .map(|dir| load_credentials(&dir))
                .and_then(|creds| creds.get(cfg.provider.as_str()).cloned())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();
    clido_providers::build_provider_with_ua(
        &cfg.provider,
        api_key,
        cfg.model.clone(),
        cfg.base_url.as_deref(),
        cfg.user_agent.clone(),
    )
    .map_err(|e| format!("build fast provider: {e}"))
}

/// Public wrapper: build a fast (utility) provider from a [`FastProviderConfig`].
/// Used by the TUI event loop to create a provider for `/enhance` and similar
/// background tasks that don't go through the agent.
pub fn build_tui_fast_provider(
    cfg: &clido_core::FastProviderConfig,
) -> Result<Arc<dyn clido_providers::ModelProvider>, String> {
    build_fast_provider(cfg)
}

fn build_routing_instructions(has_worker: bool, has_reviewer: bool) -> String {
    let mut lines = vec!["## Sub-Agent Routing".to_string(), String::new()];
    if has_worker {
        lines.push(
            "You have a `SpawnWorker` sub-agent available. \
             SpawnWorker has its own tool access (Read, Grep, Glob, Bash) and runs independently."
                .into(),
        );
        lines.push(String::new());
        lines.push("You MUST delegate to SpawnWorker for:".into());
        lines.push("- Selecting or filtering relevant files from a large list".into());
        lines.push(
            "- Summarizing or extracting information from long file content or search results"
                .into(),
        );
        lines.push("- Extracting structured fields from unstructured text or JSON".into());
        lines
            .push("- Formatting, normalizing, or transforming output into a specific shape".into());
        lines.push("- Any mechanical subtask where the result feeds into your next step".into());
        lines.push(String::new());
        lines.push("Do NOT use SpawnWorker for:".into());
        lines.push("- Tasks that require your judgment, creativity, or domain knowledge".into());
        lines.push("- Tasks where the full conversation context is required".into());
        lines.push("- Simple one-liner lookups you can do with a single tool call yourself".into());
        lines.push(String::new());
        lines.push(
            "When calling SpawnWorker: pass only the minimal context slice it needs. \
             Never send the full conversation history. \
             Describe the expected output format explicitly."
                .into(),
        );
        lines.push(String::new());
    }
    if has_reviewer {
        lines.push(
            "You have a `SpawnReviewer` sub-agent available. \
             SpawnReviewer has file system tool access and returns a PASS or FAIL verdict."
                .into(),
        );
        lines.push(String::new());
        lines.push("You MUST call SpawnReviewer after:".into());
        lines.push("- Writing or modifying code files".into());
        lines.push("- Completing a multi-step implementation task".into());
        lines.push("- Generating output that will be handed to the user as final".into());
        lines.push(String::new());
        lines.push(
            "When calling SpawnReviewer: pass the changed code as `output` and the original \
             task description plus specific quality criteria as `criteria`. \
             If the reviewer returns FAIL, fix each listed issue before responding to the user."
                .into(),
        );
        lines.push(String::new());
    }
    lines.push("Sub-agent rules:".into());
    lines.push(
        "- Pass only the context slice the sub-agent needs — never the full conversation.".into(),
    );
    lines.push(
        "- If a sub-agent fails or is disabled, complete the task yourself without blocking."
            .into(),
    );
    lines.push(
        "- Sub-agent cost counts against the session budget. Don't spawn unnecessarily.".into(),
    );
    lines.join("\n")
}

/// If an MCP config path is provided, spawn MCP servers and register their tools.
/// Errors are printed to stderr but never fatal — the agent runs with whatever
/// tools were successfully registered.
pub(crate) fn load_mcp_tools_from_path(
    mcp_path: Option<&std::path::Path>,
    quiet: bool,
    mut registry: ToolRegistry,
) -> ToolRegistry {
    let Some(mcp_path) = mcp_path else {
        return registry;
    };
    use clido_tools::load_mcp_config;
    use clido_tools::McpClient;
    let mcp_cfg = match load_mcp_config(mcp_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("MCP config load failed: {}", e);
            return registry;
        }
    };
    // MCP initialize/list_tools are now async. We're called from a sync context during
    // startup, so we use block_in_place to drive the async calls on the current thread
    // without blocking the entire runtime.
    let rt = tokio::runtime::Handle::current();
    for server_config in mcp_cfg.servers {
        let server_name = server_config.name.clone();
        match McpClient::spawn(server_config) {
            Err(e) => eprintln!("MCP spawn failed for '{}': {}", server_name, e),
            Ok(client) => {
                let init_result = tokio::task::block_in_place(|| rt.block_on(client.initialize()));
                if let Err(e) = init_result {
                    eprintln!("MCP initialize failed for '{}': {}", server_name, e);
                    continue;
                }
                let tools_result = tokio::task::block_in_place(|| rt.block_on(client.list_tools()));
                match tools_result {
                    Err(e) => eprintln!("MCP list_tools failed for '{}': {}", server_name, e),
                    Ok(tools) => {
                        let client_arc = Arc::new(client);
                        for tool_def in tools {
                            let tool_name = tool_def.name.clone();
                            // Guard: MCP tool names must not collide with built-in tools.
                            if registry.get(&tool_name).is_some() {
                                eprintln!(
                                    "MCP tool '{}' from '{}' conflicts with a built-in tool — skipping.",
                                    tool_name, server_name
                                );
                                continue;
                            }
                            let mcp_tool = McpTool::new(tool_def, client_arc.clone());
                            registry.register(mcp_tool);
                            if !quiet {
                                eprintln!("MCP tool registered: {}/{}", server_name, tool_name);
                            }
                        }
                    }
                }
            }
        }
    }
    registry
}

fn load_mcp_tools(cli: &Cli, registry: ToolRegistry) -> ToolRegistry {
    load_mcp_tools_from_path(cli.mcp_config.as_deref(), cli.quiet, registry)
}

fn build_registry(
    cli: &Cli,
    loaded: &clido_core::LoadedConfig,
    workspace_root: &Path,
    external_todo_store: Option<TodoStore>,
    allowed_external_paths: &[std::path::PathBuf],
) -> Result<(ToolRegistry, TodoStore), anyhow::Error> {
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
    let blocked = clido_core::global_config_path()
        .into_iter()
        .collect::<Vec<_>>();
    let sandbox = cli.sandbox;
    let (mut registry, mut todo_store) = if allowed_external_paths.is_empty() {
        default_registry_with_todo_store(workspace_root.to_path_buf(), blocked, sandbox)
    } else {
        clido_tools::default_registry_with_options_and_allowed_paths(
            workspace_root.to_path_buf(),
            blocked,
            sandbox,
            allowed_external_paths.to_vec(),
        )
    };
    // If an external store was provided, replace so the TUI and agent share the same Arc.
    if let Some(ext) = external_todo_store {
        // Re-register TodoWriteTool with the external store.
        registry.register(clido_tools::TodoWriteTool::with_store(ext.clone()));
        todo_store = ext;
    }
    let registry = registry.with_filters(allowed, disallowed);
    if registry.schemas().is_empty() {
        return Err(CliError::Usage(
            "No tools left after --allowed-tools/--disallowed-tools/--tools. Check your filters."
                .into(),
        )
        .into());
    }
    Ok((registry, todo_store))
}

pub fn parse_permission_mode(s: Option<&str>) -> PermissionMode {
    match s {
        None => PermissionMode::Default,
        Some("plan") | Some("plan-only") => PermissionMode::PlanOnly,
        Some("accept-all") => PermissionMode::AcceptAll,
        Some("diff-review") => PermissionMode::DiffReview,
        Some("default") => PermissionMode::Default,
        Some(other) => {
            eprintln!(
                "warning: unknown --permission-mode value {:?}. \
                 Valid values: default, plan-only, accept-all, diff-review. \
                 Falling back to default.",
                other
            );
            PermissionMode::Default
        }
    }
}

fn assemble_system_prompt(cli: &Cli) -> Result<String, anyhow::Error> {
    let base = if let Some(ref path) = cli.system_prompt_file {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Usage(format!("Failed to read system prompt file: {}", e)))?
    } else if let Some(ref s) = cli.system_prompt {
        s.clone()
    } else {
        "\
You are clido, an AI software engineering agent. You help with coding tasks: \
reading, understanding, writing, editing, and running code across any language \
or stack. Always refer to yourself as clido — never as Claude, GPT, Gemini, or \
any other model name.

Respond in the language the user writes in.

## Core behavior

- Be concise and direct. Lead with the answer or action, not the reasoning.
- Do not summarize what you just did — the user can see the diff.
- Do not add filler: \"Great!\", \"Sure!\", \"Of course!\" — just act.
- When you can say it in one sentence, don't use three.

## Working with code

- Always read a file before editing it. Never guess at its contents.
- Understand the existing code before suggesting changes.
- Make the smallest change that solves the problem. Do not refactor \
surrounding code unless asked.
- Do not add comments, docstrings, or type annotations to code you didn't change.
- Do not add error handling for scenarios that cannot happen.
- Three similar lines of code is better than a premature abstraction.
- Do not design for hypothetical future requirements.

## Planning

- Before acting on any non-trivial task, state your plan in 2-3 lines. \
Then execute.
- If the plan turns out to be wrong mid-execution, stop and restate it.
- When asked to create a plan, number each top-level step as \"Step N: description\". \
Sub-bullets and notes under each step are encouraged for clarity.
- When executing a plan, announce each step before starting it: \
\"Step N: description\" on its own line. This helps the user track progress.

## Multi-file and multi-step work

- When a task spans multiple files, state your plan and order of operations \
before starting. Do not begin file 3 while file 1 is still broken.
- Reason about dependencies explicitly before making changes.
- If the task is large, confirm the breakdown with the user before executing.
- When something unexpected appears (unknown files, foreign config, merge \
conflicts), investigate before acting.

## Debugging

- Identify likely causes based on evidence, not guesses.
- Narrow down systematically before proposing a fix.
- Explain why the fix works, not just what it changes.
- Do not brute-force — if blocked, diagnose the root cause.

## Testing

- Write tests for new behavior when a test suite already exists.
- Do not add tests to untested codebases unless explicitly asked.
- Do not modify existing tests to make them pass — fix the code instead.
- Tests should test behavior, not implementation details.

## Tool discipline

- Use the most specific tool available (Read, Grep, Glob) before falling \
back to shell commands.
- Prefer editing existing files over creating new ones.
- Never delete or overwrite without reading first.
- For destructive or irreversible actions (rm, force push, drop table), \
stop and confirm with the user.

## Security

- Never introduce command injection, XSS, SQL injection, or other OWASP \
top 10 vulnerabilities.
- Never commit secrets, API keys, or credentials.
- Validate at system boundaries (user input, external APIs). Trust internal \
code and framework guarantees.

## Git

- Only commit when explicitly asked.
- Write commit messages that explain *why*, not just *what*.
- Never skip hooks (--no-verify) or force-push without explicit instruction.
- Prefer new commits over amending published commits.

## When to ask vs. act

- For small, local, reversible changes: act immediately.
- For ambiguous requirements: ask one focused question, not five.
- For irreversible actions affecting shared state (push, delete, send): \
confirm first.
- If blocked, diagnose the root cause — do not brute-force or bypass \
safety checks.

## Communication

- Reference specific file paths and line numbers when discussing code.
- If the task is unclear, state your interpretation before acting.
- If you discover something unexpected (unknown files, foreign config, \
merge conflicts), investigate before overwriting.\
"
        .to_string()
    };
    Ok(if let Some(ref append) = cli.append_system_prompt {
        format!("{}\n{}", base, append)
    } else {
        base
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_permission_mode ─────────────────────────────────────────────

    #[test]
    fn parse_permission_mode_plan() {
        assert_eq!(
            parse_permission_mode(Some("plan")),
            PermissionMode::PlanOnly
        );
        assert_eq!(
            parse_permission_mode(Some("plan-only")),
            PermissionMode::PlanOnly
        );
    }

    #[test]
    fn parse_permission_mode_accept_all() {
        assert_eq!(
            parse_permission_mode(Some("accept-all")),
            PermissionMode::AcceptAll
        );
    }

    #[test]
    fn parse_permission_mode_diff_review() {
        assert_eq!(
            parse_permission_mode(Some("diff-review")),
            PermissionMode::DiffReview
        );
    }

    #[test]
    fn parse_permission_mode_default_on_none() {
        assert_eq!(parse_permission_mode(None), PermissionMode::Default);
    }

    #[test]
    fn parse_permission_mode_default_on_unknown() {
        assert_eq!(
            parse_permission_mode(Some("garbage")),
            PermissionMode::Default
        );
    }

    // ── build_routing_instructions ────────────────────────────────────────

    #[test]
    fn routing_instructions_worker_only_mentions_spawn_worker() {
        let s = build_routing_instructions(true, false);
        assert!(s.contains("SpawnWorker"), "should mention SpawnWorker");
        assert!(
            !s.contains("SpawnReviewer"),
            "should not mention SpawnReviewer"
        );
        assert!(s.contains("Sub-Agent Routing"));
        assert!(s.contains("Sub-agent rules:"));
    }

    #[test]
    fn routing_instructions_worker_uses_must_delegate_language() {
        let s = build_routing_instructions(true, false);
        assert!(
            s.contains("MUST delegate"),
            "worker instructions must use imperative 'MUST delegate' language, got:\n{}",
            s
        );
    }

    #[test]
    fn routing_instructions_reviewer_only_mentions_spawn_reviewer() {
        let s = build_routing_instructions(false, true);
        assert!(!s.contains("SpawnWorker"), "should not mention SpawnWorker");
        assert!(s.contains("SpawnReviewer"), "should mention SpawnReviewer");
    }

    #[test]
    fn routing_instructions_reviewer_uses_must_call_language() {
        let s = build_routing_instructions(false, true);
        assert!(
            s.contains("MUST call SpawnReviewer"),
            "reviewer instructions must use imperative 'MUST call SpawnReviewer' language, got:\n{}",
            s
        );
    }

    #[test]
    fn routing_instructions_both_mentions_both() {
        let s = build_routing_instructions(true, true);
        assert!(s.contains("SpawnWorker"));
        assert!(s.contains("SpawnReviewer"));
        assert!(s.contains("Sub-agent rules:"));
    }

    #[test]
    fn routing_instructions_neither_still_has_header_and_rules() {
        let s = build_routing_instructions(false, false);
        assert!(s.contains("Sub-Agent Routing"));
        assert!(s.contains("Sub-agent rules:"));
        assert!(!s.contains("SpawnWorker"));
        assert!(!s.contains("SpawnReviewer"));
    }

    #[test]
    fn routing_instructions_has_retry_guidance() {
        let s = build_routing_instructions(true, true);
        // "If a sub-agent fails or is disabled, complete the task yourself."
        assert!(
            s.contains("fails") && s.contains("disabled"),
            "should include guidance for failed/disabled sub-agents"
        );
    }
}
