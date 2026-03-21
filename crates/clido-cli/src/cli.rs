//! CLI argument parsing (V1 surface).

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "clido", version = env!("CARGO_PKG_VERSION"), about = "clido — your coding agent")]
pub struct Cli {
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    /// Prompt (positional). Omit for REPL when TTY.
    #[arg(trailing_var_arg = true)]
    pub prompt: Vec<String>,

    /// Print-only / non-interactive: no REPL, no permission prompts.
    #[arg(short, long)]
    pub print: bool,

    /// Resume session by ID.
    #[arg(long)]
    pub resume: Option<String>,

    /// Continue newest session for current project (cannot combine with --resume).
    #[arg(long)]
    pub r#continue: bool,

    /// Profile name from config.
    #[arg(long, env = "CLIDO_PROFILE")]
    pub profile: Option<String>,

    /// Model override.
    #[arg(long, env = "CLIDO_MODEL")]
    pub model: Option<String>,

    /// Provider override (e.g. anthropic). Unsupported provider → startup error in V1.
    #[arg(long, env = "CLIDO_PROVIDER")]
    pub provider: Option<String>,

    /// Max agent turns.
    #[arg(long, env = "CLIDO_MAX_TURNS", default_value = "10")]
    pub max_turns: u32,

    /// Max budget in USD.
    #[arg(long, env = "CLIDO_MAX_BUDGET_USD")]
    pub max_budget_usd: Option<f64>,

    /// Permission mode: default, accept-all, plan.
    #[arg(long, env = "CLIDO_PERMISSION_MODE")]
    pub permission_mode: Option<String>,

    /// System prompt (replaces config).
    #[arg(long, env = "CLIDO_SYSTEM_PROMPT")]
    pub system_prompt: Option<String>,

    /// System prompt from file.
    #[arg(long)]
    pub system_prompt_file: Option<PathBuf>,

    /// Append to system prompt.
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Allowed tools (comma-separated). Overrides disallowed.
    #[arg(long)]
    pub allowed_tools: Option<String>,

    /// Disallowed tools (comma-separated).
    #[arg(long)]
    pub disallowed_tools: Option<String>,

    /// Tools list (alias for allowed).
    #[arg(long)]
    pub tools: Option<String>,

    /// Output format: text (default), json, or stream-json.
    #[arg(long, env = "CLIDO_OUTPUT_FORMAT", default_value = "text")]
    pub output_format: String,

    /// Disable color (also respects NO_COLOR env when set).
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_color: bool,

    /// Verbose logging.
    #[arg(short, long)]
    pub verbose: bool,

    /// Suppress spinner, tool lifecycle output, and cost footer.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Path to MCP server config (JSON/YAML) for Model Context Protocol tool servers.
    #[arg(long)]
    pub mcp_config: Option<std::path::PathBuf>,

    /// Max parallel tool calls for read-only tools.
    #[arg(long, env = "CLIDO_MAX_PARALLEL_TOOLS")]
    pub max_parallel_tools: Option<u32>,

    /// Ignore stale-file check when resuming.
    #[arg(long)]
    pub resume_ignore_stale: bool,

    /// Working directory (default: current directory).
    #[arg(long, short = 'C', env = "CLIDO_WORKDIR")]
    pub workdir: Option<std::path::PathBuf>,

    /// Input format: text (default) or stream-json (for SDK/subprocess use).
    #[arg(long, env = "CLIDO_INPUT_FORMAT", default_value = "text")]
    pub input_format: String,

    /// Enable Bash sandboxing (sandbox-exec on macOS, bwrap on Linux).
    #[arg(long)]
    pub sandbox: bool,

    /// Enable task decomposition planner (experimental): decomposes the prompt into a DAG
    /// of subtasks before executing. Falls back to the reactive loop on plan failure.
    #[arg(long)]
    pub planner: bool,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Subcommand {
    /// Manage sessions (list, show, fork, resume).
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },

    /// Print version.
    Version,

    /// Explicit setup / config wizard.
    Init,

    /// Check environment, API key, config, and tool health.
    Doctor,

    /// Show or edit config values.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },

    /// Run declarative YAML workflows (run, validate, inspect, list).
    Workflow {
        #[command(subcommand)]
        cmd: WorkflowCmd,
    },

    /// List available models by provider.
    ListModels {
        /// Provider filter (anthropic, openrouter, local).
        #[arg(long)]
        provider: Option<String>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Update model pricing data from remote (shows current file path and age).
    UpdatePricing,

    /// Run an agent with a prompt (scriptable alias for positional prompt).
    Run {
        /// Prompt to send to the agent.
        #[arg(trailing_var_arg = true)]
        prompt: Vec<String>,
    },

    /// Show session statistics (cost, turns, timing).
    Stats {
        /// Filter to a specific session ID.
        #[arg(long)]
        session: Option<String>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// View the tool call audit log.
    Audit {
        /// Tail mode (last N entries).
        #[arg(long)]
        tail: Option<usize>,
        /// Filter by session ID.
        #[arg(long)]
        session: Option<String>,
        /// Filter by tool name.
        #[arg(long)]
        tool: Option<String>,
        /// Filter by start time (ISO 8601, e.g. 2026-01-01).
        #[arg(long)]
        since: Option<String>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Generate shell completion scripts.
    Completions {
        /// Shell: bash, zsh, fish, powershell, elvish.
        shell: String,
    },

    /// Generate man page (output to stdout, pipe to man or save to file).
    Man,

    /// Long-term memory management (list, prune, reset).
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
    },

    /// Fetch model list from a provider's API.
    FetchModels {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Repository index management (build, stats, clear) — enables SemanticSearch tool.
    Index {
        #[command(subcommand)]
        cmd: IndexCmd,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum IndexCmd {
    /// Build the repository index (file + symbol index).
    Build {
        /// Directory to index (default: current directory).
        #[arg(long, short = 'd')]
        dir: Option<PathBuf>,
        /// File extensions to index, comma-separated (default: rs,py,js,ts,go).
        #[arg(long, default_value = "rs,py,js,ts,go")]
        ext: String,
    },
    /// Show index statistics.
    Stats,
    /// Clear the index.
    Clear,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum ConfigCmd {
    /// Show current config.
    Show,
    /// Set a config value. Keys: model, provider, api-key.
    Set {
        /// Config key (model, provider, api-key).
        key: String,
        /// New value.
        value: String,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum WorkflowCmd {
    /// Run a workflow from file or name.
    Run {
        /// Workflow file path or name (from workflows directory).
        workflow: String,
        /// Input overrides: key=value (repeatable).
        #[arg(long, short = 'i')]
        input: Vec<String>,
        /// Validate and render prompts only; no API calls.
        #[arg(long)]
        dry_run: bool,
        /// Skip cost confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Validate workflow YAML.
    Validate { path: PathBuf },
    /// Inspect workflow: list steps and dependencies.
    Inspect { path: PathBuf },
    /// List workflows from configured directories.
    List,
    /// Run preflight checks on a workflow (profiles, tools, inputs).
    Check {
        path: PathBuf,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum MemoryCmd {
    /// List memories (most recent first).
    List {
        /// Maximum entries to show (default: 20).
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Prune old memories, keeping the most recent N.
    Prune {
        /// Number of recent memories to keep (default: 100).
        #[arg(long)]
        keep: Option<usize>,
    },
    /// Reset (delete all) memories.
    Reset {
        /// Skip confirmation prompt.
        #[arg(long)]
        force: bool,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum SessionsCmd {
    List,
    Show {
        id: String,
    },
    /// Fork a session: copy it to a new session ID.
    Fork {
        id: String,
    },
}

impl Cli {
    /// Single prompt string from positional args.
    pub fn prompt_str(&self) -> String {
        self.prompt.join(" ").trim().to_string()
    }

    /// True if this is a run (no subcommand or subcommand is not sessions/version/init). Used for REPL.
    #[allow(dead_code)]
    pub fn is_run(&self) -> bool {
        match &self.subcommand {
            None => true,
            Some(Subcommand::Sessions { .. }) => false,
            Some(Subcommand::Version) => false,
            Some(Subcommand::Init) => false,
            Some(Subcommand::Doctor) => false,
            Some(Subcommand::Config { .. }) => false,
            Some(Subcommand::Workflow { .. }) => false,
            Some(Subcommand::ListModels { .. }) => false,
            Some(Subcommand::UpdatePricing) => false,
            Some(Subcommand::Run { .. }) => false,
            Some(Subcommand::Stats { .. }) => false,
            Some(Subcommand::Audit { .. }) => false,
            Some(Subcommand::Completions { .. }) => false,
            Some(Subcommand::Man) => false,
            Some(Subcommand::Memory { .. }) => false,
            Some(Subcommand::FetchModels { .. }) => false,
            Some(Subcommand::Index { .. }) => false,
        }
    }
}
