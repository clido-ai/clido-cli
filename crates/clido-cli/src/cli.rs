//! CLI argument parsing (V1 surface).

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "clido", version = env!("CARGO_PKG_VERSION"), about = "Coding agent CLI")]
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

    /// Output format: text, json, stream-json.
    #[arg(long, env = "CLIDO_OUTPUT_FORMAT", default_value = "text")]
    pub output_format: String,

    /// Disable color (also respects NO_COLOR env when set).
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_color: bool,

    /// Verbose logging.
    #[arg(short, long)]
    pub verbose: bool,

    /// Ignore stale-file check when resuming.
    #[arg(long)]
    pub resume_ignore_stale: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum Subcommand {
    /// Session commands.
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },

    /// Legacy: list sessions (deprecated). Use 'sessions list'.
    #[command(name = "list-sessions")]
    ListSessions,

    /// Legacy: show session by ID (deprecated). Use 'sessions show <id>'.
    #[command(name = "show-session")]
    ShowSession { id: String },

    /// Print version.
    Version,

    /// Explicit setup / config wizard.
    Init,

    /// Run health checks (API key, session dir, pricing).
    Doctor,

    /// Workflow commands (run, validate, inspect, list).
    Workflow {
        #[command(subcommand)]
        cmd: WorkflowCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
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
}

#[derive(clap::Subcommand, Debug)]
pub enum SessionsCmd {
    List,
    Show { id: String },
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
            Some(Subcommand::ListSessions) => false,
            Some(Subcommand::ShowSession { .. }) => false,
            Some(Subcommand::Sessions { .. }) => false,
            Some(Subcommand::Version) => false,
            Some(Subcommand::Init) => false,
            Some(Subcommand::Doctor) => false,
            Some(Subcommand::Workflow { .. }) => false,
        }
    }
}
