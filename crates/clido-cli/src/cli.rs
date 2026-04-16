//! CLI argument parsing (V1 surface).

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "clido",
    version = env!("CARGO_PKG_VERSION"),
    about = "clido — terminal AI coding agent (TUI, sessions, tools, skills, workflows)"
)]
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

    /// Max agent turns per prompt. Defaults to the config value (200 if unset).
    #[arg(long, env = "CLIDO_MAX_TURNS")]
    pub max_turns: Option<u32>,

    /// Max budget in USD.
    #[arg(long, env = "CLIDO_MAX_BUDGET_USD")]
    pub max_budget_usd: Option<f64>,

    /// Permission mode: default, accept-all, plan-only, diff-review.
    /// default: ask for each tool call. accept-all: allow all without prompting.
    /// plan-only: generate a plan and stop (no execution). diff-review: show diffs before writes.
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
    #[arg(long, alias = "plan")]
    pub planner: bool,

    /// With --planner/--plan: generate the plan and show it but never execute.
    #[arg(long)]
    pub plan_dry_run: bool,

    /// With --planner/--plan: skip the interactive editor and execute immediately (CI-friendly).
    #[arg(long)]
    pub plan_no_edit: bool,

    /// Enable harness mode: `.clido/harness/` JSON tasks, progress log, `HarnessControl` tool, strict verify-before-pass protocol.
    #[arg(long, env = "CLIDO_HARNESS")]
    pub harness: bool,

    /// Skip all CLIDO.md / rules file injection for this invocation.
    #[arg(long)]
    pub no_rules: bool,

    /// Use this specific rules file instead of the standard hierarchical lookup.
    #[arg(long, env = "CLIDO_RULES_FILE")]
    pub rules_file: Option<std::path::PathBuf>,

    /// Force-enable desktop notification + terminal bell for this run,
    /// overriding the config and the minimum-duration gate.
    #[arg(long, conflicts_with = "no_notify")]
    pub notify: bool,

    /// Suppress desktop notifications and terminal bell for this run.
    #[arg(long)]
    pub no_notify: bool,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Subcommand {
    /// Manage sessions (list, show, fork, verify).
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

    /// Commit current changes with an AI-generated message.
    Commit {
        /// Skip confirmation (accept generated message immediately).
        #[arg(long)]
        yes: bool,

        /// Generate and print the commit message but do not run git commit.
        #[arg(long)]
        dry_run: bool,
    },

    /// Checkpoint management (list, save, rollback, diff).
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },

    /// Roll back to a checkpoint (shows diff and prompts for confirmation).
    Rollback {
        /// Checkpoint ID to restore. If omitted, lists available checkpoints.
        id: Option<String>,
        /// Session ID (defaults to most recent session).
        #[arg(long)]
        session: Option<String>,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Plan management (list, show, run, delete saved plans).
    Plan {
        #[command(subcommand)]
        cmd: PlanCmd,
    },

    /// Agent profile management (list, create, switch, edit, delete).
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },

    /// List and toggle reusable agent skills (`.clido/skills/`, config `[skills]`).
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },

    /// Deprecated: use `sessions list` instead.
    #[command(hide = true, name = "list-sessions")]
    ListSessions,

    /// Deprecated: use `sessions show <id>` instead.
    #[command(hide = true, name = "show-session")]
    ShowSession {
        /// Session ID to display.
        id: String,
    },

    /// Clear the terminal screen.
    Clear,

    /// Search conversation history in a session.
    Search {
        /// Search query.
        query: String,
        /// Session ID (defaults to current session).
        #[arg(long)]
        session: Option<String>,
    },

    /// Export a session to markdown.
    Export {
        /// Session ID (defaults to current session).
        #[arg(long)]
        session: Option<String>,
        /// Output file path.
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Send a note/hint to the agent (injects into next context).
    Note {
        /// Note text.
        text: Vec<String>,
    },

    /// Compact the session context immediately.
    Compact {
        /// Session ID (defaults to current session).
        #[arg(long)]
        session: Option<String>,
    },

    /// Copy session messages to clipboard.
    Copy {
        /// Copy all messages (default: only last assistant message).
        #[arg(long)]
        all: bool,
        /// Session ID (defaults to current session).
        #[arg(long)]
        session: Option<String>,
    },

    /// Toggle desktop notifications.
    Notify {
        /// Enable or disable notifications.
        #[arg(value_name = "STATE")]
        state: Option<String>,
    },

    /// Show the agent's current todo list.
    Todo {
        /// Session ID (defaults to current session).
        #[arg(long)]
        session: Option<String>,
    },

    /// Show or set the working directory.
    Workdir {
        /// New working directory (omit to show current).
        path: Option<PathBuf>,
    },

    /// Run diagnostics on the current project.
    Check,

    /// Show active project rules files.
    Rules,

    /// Attach an image to the next message.
    Image {
        /// Path to image file.
        path: PathBuf,
    },

    /// Allow one-off access outside the workspace.
    AllowPath {
        /// Path to allow.
        path: PathBuf,
    },

    /// List paths allowed for this session.
    AllowedPaths,

    /// Check for updates and install the latest version.
    Update,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum IndexCmd {
    /// Build the repository index (file + symbol index).
    Build {
        /// Directory to index (default: current directory).
        #[arg(long, short = 'd')]
        dir: Option<PathBuf>,
        /// File extensions to index, comma-separated.
        /// Default includes Web3/smart-contract languages (sol,move,vy,fe,yul,cairo)
        /// plus common general-purpose languages.
        #[arg(
            long,
            default_value = "sol,move,vy,fe,yul,rell,cairo,rs,py,js,ts,go,java,c,cpp,h,md"
        )]
        ext: String,
        /// Bypass .gitignore rules and index all files including build artifacts.
        #[arg(long)]
        include_ignored: bool,
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
        /// Profile to use for steps without an explicit profile (overrides config default).
        #[arg(long, short = 'p')]
        profile: Option<String>,
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
    /// Strict-load check: verify session JSONL decodes to agent history (same as resume).
    Verify {
        id: String,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum CheckpointCmd {
    /// List checkpoints for the current (or specified) session.
    List {
        /// Session ID (defaults to most recent session).
        #[arg(long)]
        session: Option<String>,
    },
    /// Save a named checkpoint.
    Save {
        /// Optional name for the checkpoint.
        name: Option<String>,
    },
    /// Roll back to a checkpoint (interactive file picker).
    Rollback {
        /// Checkpoint ID to restore.
        id: Option<String>,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Show diff since a checkpoint.
    Diff {
        /// Checkpoint ID to diff against.
        id: String,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum PlanCmd {
    /// List all saved plans.
    List,
    /// Show a saved plan with its tasks and status.
    Show {
        /// Plan ID.
        id: String,
    },
    /// Execute a saved plan (resumes from the first pending task).
    Run {
        /// Plan ID.
        id: String,
    },
    /// Delete a saved plan.
    Delete {
        /// Plan ID.
        id: String,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum ProfileCmd {
    /// List all profiles with active model per slot.
    List,
    /// Create a new profile (guided wizard).
    Create {
        /// Profile name (prompted if omitted).
        name: Option<String>,
    },
    /// Switch the active profile.
    Switch {
        /// Profile name to activate.
        name: String,
    },
    /// Edit a profile (guided, pre-filled with current values).
    Edit {
        /// Profile name to edit.
        name: String,
    },
    /// Delete a profile (default profile cannot be deleted).
    Delete {
        /// Profile name to delete.
        name: String,
        /// Skip confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum SkillsCmd {
    /// List skills on disk and whether each is active for this workspace.
    List,
    /// Print skill search paths (workspace, global, config extras, env).
    Paths,
    /// Disable a skill (writes `[skills].disabled` in the project `.clido/config.toml`).
    Disable {
        /// Skill id (from YAML `id` or file stem).
        id: String,
    },
    /// Remove a skill from the disabled list.
    Enable {
        /// Skill id.
        id: String,
    },
}

impl Cli {
    /// Single prompt string from positional args.
    pub fn prompt_str(&self) -> String {
        self.prompt.join(" ").trim().to_string()
    }
}
