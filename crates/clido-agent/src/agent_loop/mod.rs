//! Minimal agent loop: history, provider call, tool execution, repeat.

mod context;
pub mod history;
mod planning;
mod security;

use async_trait::async_trait;
use clido_context::{
    estimate_tokens_messages, estimate_tokens_str, DEFAULT_COMPACTION_THRESHOLD,
    DEFAULT_MAX_CONTEXT_TOKENS,
};
use clido_core::{
    compute_cost_usd, AgentConfig, ContentBlock, HooksConfig, Message, PermissionMode, Role,
    StopReason,
};
use clido_core::{evaluate_rules, RuleAction};
use clido_core::{ClidoError, PricingTable, Result};
use clido_memory::MemoryStore;
use clido_providers::ModelProvider;
use clido_storage::{AuditEntry, AuditLog, SessionLine, SessionWriter};
use clido_tools::{ToolOutput, ToolRegistry};
use futures::future::join_all;
use similar::TextDiff;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use context::{compact_for_model_request, CONTEXT_OUTPUT_RESERVE, PROACTIVE_SUMMARIZE_THRESHOLD};
pub use history::session_lines_to_messages;
use security::{detect_injection, enhanced_edit_error};
/// How many consecutive identical tool failures trigger doom-loop detection.
const DOOM_LOOP_THRESHOLD: usize = 3;
/// Budget warning thresholds (percentage of limit consumed).
const BUDGET_WARNING_PCTS: &[u8] = &[50, 80, 90];

/// When tools fail, prepend explicit recovery instructions so the model does not stop early.
fn prepend_tool_recovery_nudge(
    tool_uses: &[(String, String, serde_json::Value)],
    outputs: &[(ToolOutput, u64)],
    tool_results: &mut Vec<ContentBlock>,
) {
    let failures: Vec<(&str, &str)> = tool_uses
        .iter()
        .zip(outputs.iter())
        .filter_map(|((_, name, _), (out, _))| {
            if out.is_error {
                Some((name.as_str(), out.content.as_str()))
            } else {
                None
            }
        })
        .collect();
    if failures.is_empty() {
        return;
    }
    tool_results.insert(
        0,
        ContentBlock::Text {
            text: crate::prompts::tool_failure_recovery_nudge(&failures),
        },
    );
}

/// Retry strategies for failed tool calls.
#[derive(Debug, Clone, Copy)]
enum RetryStrategy {
    /// Retry immediately with no delay.
    RetryOnce,
    /// Wait for specified milliseconds before retrying.
    WaitAndRetry { delay_ms: u64 },
}

/// Create a git checkpoint of dirty working tree before AI edits.
/// Only runs once per agent session to avoid excessive commits.
async fn maybe_create_checkpoint(workspace: Option<&std::path::Path>) -> Option<String> {
    let ws = workspace?;
    // Check if we're in a git repo and have dirty state
    let status = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(ws)
        .output()
        .await
        .ok()?;
    let output = String::from_utf8_lossy(&status.stdout);
    if output.trim().is_empty() {
        return None; // Clean working tree, no checkpoint needed
    }
    // Stage all and create checkpoint commit
    let add = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(ws)
        .output()
        .await
        .ok()?;
    if !add.status.success() {
        return None;
    }
    let commit = tokio::process::Command::new("git")
        .args([
            "commit",
            "-m",
            "chore: pre-clido checkpoint (auto)",
            "--no-verify",
        ])
        .current_dir(ws)
        .output()
        .await
        .ok()?;
    if !commit.status.success() {
        // Reset staged changes if commit failed
        let _ = tokio::process::Command::new("git")
            .args(["reset", "HEAD"])
            .current_dir(ws)
            .output()
            .await;
        return None;
    }
    // Get the commit hash
    let hash = tokio::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(ws)
        .output()
        .await
        .ok()?;
    let hash_str = String::from_utf8_lossy(&hash.stdout).trim().to_string();
    tracing::info!("Created pre-edit checkpoint: {}", hash_str);
    Some(hash_str)
}

/// The result of a permission request — what the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermGrant {
    /// Allow this single invocation.
    Allow,
    /// Deny this invocation.
    Deny,
    /// Deny and send feedback text back to the agent so it can adjust its approach.
    DenyWithFeedback(String),
    /// Open proposed content in `$EDITOR` and use whatever the user saves.
    EditInEditor,
    /// Allow all remaining Write/Edit operations in this session without asking again.
    AllowAll,
}

/// A request for permission before a state-changing operation.
#[derive(Debug, Default)]
pub struct PermRequest {
    pub tool_name: String,
    pub description: String,
    /// Pre-rendered unified diff string (populated for Write/Edit in diff-review mode).
    pub diff: Option<String>,
    /// Full proposed file content (used when the user presses 'e' to open in editor).
    pub proposed_content: Option<String>,
    /// Path of the file being written/edited (used for temp file extension in editor).
    pub file_path: Option<std::path::PathBuf>,
}

/// Callback for asking the user to approve a state-changing tool call.
#[async_trait]
pub trait AskUser: Send + Sync {
    /// Ask the user for permission. Returns a `PermGrant` indicating the decision.
    async fn ask(&self, req: PermRequest) -> PermGrant;
}

/// Callback for observing tool calls in real time (used by the TUI to show progress).
#[async_trait]
pub trait EventEmitter: Send + Sync {
    async fn on_tool_start(&self, tool_use_id: &str, name: &str, input: &serde_json::Value);
    /// Called after a tool completes. `diff` is set for Edit operations.
    async fn on_tool_done(
        &self,
        tool_use_id: &str,
        name: &str,
        is_error: bool,
        diff: Option<String>,
    );
    /// Called for any text the model emits while it's still calling tools (thinking aloud).
    /// Default impl is a no-op so existing code compiles without changes.
    async fn on_assistant_text(&self, _text: &str) {}
    /// Ask the UI to approve access outside the workspace (TUI shows y/n/a prompt).
    async fn on_path_permission_request(&self, _path: &std::path::Path, _tool_name: &str) {}
    /// Called when cumulative cost crosses a budget threshold (50%, 80%, 90%).
    /// `pct` is the percentage (50, 80, or 90), `spent_usd` and `limit_usd` are raw values.
    /// Default impl is a no-op.
    async fn on_budget_warning(&self, _pct: u8, _spent_usd: f64, _limit_usd: f64) {}
}

/// PoC agent loop: messages + provider + tools.
pub struct AgentLoop {
    provider: Arc<dyn ModelProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    history: Vec<Message>,
    ask_user: Option<Arc<dyn AskUser>>,
    emit: Option<Arc<dyn EventEmitter>>,
    /// When set, overrides config.permission_mode for the rest of the session (e.g. after ExitPlanMode).
    permission_mode_override: Option<PermissionMode>,
    /// Last turn count after run() (for session recording).
    last_turn_count: u32,
    /// Cumulative cost in USD from last run (when pricing provided).
    pub cumulative_cost_usd: f64,
    /// Cumulative input tokens from last run.
    pub cumulative_input_tokens: u64,
    /// Cumulative output tokens from last run.
    pub cumulative_output_tokens: u64,
    /// Cumulative cache-read input tokens from last run.
    pub cumulative_cache_read_tokens: u64,
    /// Cumulative cache-creation input tokens from last run.
    pub cumulative_cache_creation_tokens: u64,
    /// Optional audit log for recording tool calls.
    audit_log: Option<Arc<std::sync::Mutex<AuditLog>>>,
    /// Optional hooks config for pre/post tool use.
    hooks: Option<HooksConfig>,
    /// Optional long-term memory store for context injection.
    memory: Option<Arc<Mutex<MemoryStore>>>,
    /// The original system prompt from config, captured at construction time.
    /// Used as the base for memory injection so repeated turns don't accumulate
    /// memory blocks on top of an already-injected prompt.
    base_system_prompt: Option<String>,
    /// When true, the agent will emit a planning step on the first turn (--planner flag).
    /// The plan is purely informational: the reactive loop still drives execution.
    pub planner_mode: bool,
    /// Tracks recent tool failures for doom-loop detection.
    doom_monitor: VecDeque<String>,
    /// Maximum retries for failed tool calls (0 = disable auto-retry).
    max_tool_retries: u32,
    /// Current retry attempts for the active tool batch.
    retry_attempts: HashMap<String, u32>,
    /// Tracks which budget warning percentages have already been emitted this run.
    budget_warned_pcts: Vec<u8>,
    /// Optional callback to compute fresh git context each turn. When set, the
    /// returned string (if any) is injected as a `<git_context>` addendum to the
    /// system prompt on every call to `run()` / `run_with_extra_blocks()`.
    git_context_fn: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
    /// Optional fast/cheap provider for utility tasks (titles, commits, summaries, sub-agents).
    /// When set, utility calls go through this provider instead of the main one.
    fast_provider: Option<Arc<dyn ModelProvider>>,
    /// Config for the fast provider (model name, etc). Only meaningful if fast_provider is Some.
    fast_agent_config: Option<AgentConfig>,
    /// Count of consecutive turns with tool errors (resets on success).
    consecutive_tool_errors: usize,
    /// Whether we've already created a pre-edit checkpoint this session.
    checkpoint_created: bool,
    /// Receiver for path permission grants from the TUI (for interactive external path access).
    path_permission_rx: Option<tokio::sync::mpsc::UnboundedReceiver<std::path::PathBuf>>,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        tools: ToolRegistry,
        config: AgentConfig,
        ask_user: Option<Arc<dyn AskUser>>,
    ) -> Self {
        let base_system_prompt = config.system_prompt.clone();
        Self {
            provider,
            tools,
            config,
            history: Vec::new(),
            ask_user,
            emit: None,
            permission_mode_override: None,
            last_turn_count: 0,
            cumulative_cost_usd: 0.0,
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            cumulative_cache_read_tokens: 0,
            cumulative_cache_creation_tokens: 0,
            audit_log: None,
            hooks: None,
            memory: None,
            planner_mode: false,
            base_system_prompt,
            doom_monitor: VecDeque::new(),
            budget_warned_pcts: Vec::new(),
            max_tool_retries: 3,
            retry_attempts: HashMap::new(),
            path_permission_rx: None,
            git_context_fn: None,
            fast_provider: None,
            fast_agent_config: None,
            consecutive_tool_errors: 0,
            checkpoint_created: false,
        }
    }

    /// Enable or disable planner mode (--planner CLI flag).
    pub fn with_planner(mut self, enabled: bool) -> Self {
        self.planner_mode = enabled;
        self
    }

    /// Set a fast/cheap provider for utility tasks (summarization, title, commit, sub-agents).
    /// If not set, the main provider handles everything.
    pub fn with_fast_provider(
        mut self,
        provider: Option<Arc<dyn ModelProvider>>,
        config: Option<AgentConfig>,
    ) -> Self {
        self.fast_provider = provider;
        self.fast_agent_config = config;
        self
    }

    /// Create an agent loop with pre-filled history (for resume).
    pub fn new_with_history(
        provider: Arc<dyn ModelProvider>,
        tools: ToolRegistry,
        config: AgentConfig,
        history: Vec<Message>,
        ask_user: Option<Arc<dyn AskUser>>,
    ) -> Self {
        let base_system_prompt = config.system_prompt.clone();
        Self {
            provider,
            tools,
            config,
            history,
            ask_user,
            emit: None,
            permission_mode_override: None,
            last_turn_count: 0,
            cumulative_cost_usd: 0.0,
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            cumulative_cache_read_tokens: 0,
            cumulative_cache_creation_tokens: 0,
            audit_log: None,
            hooks: None,
            memory: None,
            planner_mode: false,
            base_system_prompt,
            doom_monitor: VecDeque::new(),
            budget_warned_pcts: Vec::new(),
            max_tool_retries: 3,
            retry_attempts: HashMap::new(),
            path_permission_rx: None,
            git_context_fn: None,
            fast_provider: None,
            fast_agent_config: None,
            consecutive_tool_errors: 0,
            checkpoint_created: false,
        }
    }

    /// Attach an event emitter for tool call observability (used by TUI).
    pub fn with_emitter(mut self, emit: Arc<dyn EventEmitter>) -> Self {
        self.emit = Some(emit);
        self
    }

    /// Attach an audit log.
    pub fn with_audit_log(mut self, log: Arc<std::sync::Mutex<AuditLog>>) -> Self {
        self.audit_log = Some(log);
        self
    }

    /// Attach a per-turn git context provider. The closure is called before each
    /// `run()` invocation and its output (if any) is appended to the system prompt
    /// as a `<git_context>` block so the model always sees fresh repo state.
    pub fn with_git_context_fn(mut self, f: Box<dyn Fn() -> Option<String> + Send + Sync>) -> Self {
        self.git_context_fn = Some(f);
        self
    }

    /// Attach hooks config.
    pub fn with_hooks(mut self, hooks: HooksConfig) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Attach a long-term memory store. Before each turn, relevant memories for the
    /// current prompt are retrieved and injected into the system prompt.
    pub fn with_memory(mut self, store: Arc<Mutex<MemoryStore>>) -> Self {
        self.memory = Some(store);
        self
    }

    /// Switch the model used for subsequent turns. Conversation history is preserved.
    pub fn set_model(&mut self, model: String) {
        self.config.model = model;
    }

    /// Switch to a new profile (provider + config) while preserving conversation history.
    /// This allows seamless profile switching within a session.
    pub fn switch_profile(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        config: AgentConfig,
        tools: ToolRegistry,
    ) {
        // Preserve history - this is the key for seamless switching
        // Only update provider, config, and tools
        self.provider = provider;
        self.config = config;
        self.tools = tools;
        // Reset doom monitor since we're starting fresh with a new provider
        self.doom_monitor.clear();
        // Reset retry attempts
        self.retry_attempts.clear();
        // Keep cumulative costs/tokens as they're session-level metrics
    }

    /// Return the model currently active for this session.
    pub fn current_model(&self) -> &str {
        &self.config.model
    }

    /// Check if the budget has been exceeded and return an error if so.
    fn check_budget_exceeded(&self) -> Result<()> {
        if let Some(limit) = self.config.max_budget_usd {
            if self.cumulative_cost_usd > limit {
                return Err(ClidoError::BudgetExceeded);
            }
        }
        Ok(())
    }

    /// Return the provider + config to use for utility tasks (summarization, title, planning).
    /// If a fast provider is configured, uses that; otherwise falls back to the main provider.
    /// Returns owned `Arc` so the caller can borrow other fields of `self` mutably.
    fn utility_provider(&self) -> (Arc<dyn ModelProvider>, AgentConfig) {
        if let (Some(ref fp), Some(ref fc)) = (&self.fast_provider, &self.fast_agent_config) {
            (fp.clone(), fc.clone())
        } else {
            (self.provider.clone(), self.config.clone())
        }
    }

    /// Replace the active tool registry (used by TUI workdir changes).
    pub fn replace_tools(&mut self, tools: ToolRegistry) {
        self.tools = tools;
    }

    /// Reset the runtime permission mode override (used when the workdir changes so
    /// previously-granted AllowAll does not silently carry over to a new project).
    pub fn reset_permission_mode_override(&mut self) {
        self.permission_mode_override = None;
    }

    /// Retrieve relevant memories for the given prompt and prepend them to
    /// the system prompt override for one turn.
    fn inject_memories(&self, prompt: &str) -> Option<String> {
        let store = self.memory.as_ref()?;
        let lock = store.lock().ok()?;
        let results = lock.search_keyword(prompt, 5).ok()?;
        if results.is_empty() {
            return None;
        }
        let memory_text: String = results
            .iter()
            .map(|e| format!("- {}", e.content))
            .collect::<Vec<_>>()
            .join("\n");
        // Always inject relative to the original system prompt captured at construction,
        // not config.system_prompt which may already contain injected memories from a
        // prior turn and would cause unbounded growth across multi-turn sessions.
        let base = self
            .base_system_prompt
            .as_deref()
            .unwrap_or("You are a helpful coding assistant.");
        Some(format!("{}\n\n[Relevant memories]\n{}", base, memory_text))
    }

    /// Compute fresh git context via `git_context_fn` (if set) and return a new
    /// system prompt string with the context appended. Returns `None` when no
    /// git context function is registered or when the function returns nothing.
    /// Like `inject_memories`, always builds from `base_system_prompt` so the
    /// block does not accumulate across repeated turns.
    fn inject_git_context(&self, current_system_prompt: &str) -> Option<String> {
        let git_section = (self.git_context_fn.as_ref()?)()?;
        Some(format!("{}\n\n{}", current_system_prompt, git_section))
    }

    /// Prune the memory store to keep the most recent 5000 entries, preventing
    /// unbounded SQLite growth during long-running sessions.
    fn prune_memory_if_needed(&self) {
        if let Some(store) = self.memory.as_ref() {
            if let Ok(mut lock) = store.lock() {
                let _ = lock.prune_old(5000);
            }
        }
    }

    /// Turn count from last run (for session result line).
    pub fn turn_count(&self) -> u32 {
        self.last_turn_count
    }

    /// Set the path permission receiver for interactive external path access.
    pub fn with_path_permission_receiver(
        mut self,
        rx: tokio::sync::mpsc::UnboundedReceiver<std::path::PathBuf>,
    ) -> Self {
        self.path_permission_rx = Some(rx);
        self
    }

    /// Replace the current conversation history (for session resume).
    pub fn replace_history(&mut self, history: Vec<clido_core::Message>) {
        self.history = history;
        self.last_turn_count = 0;
        self.cumulative_cost_usd = 0.0;
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
        self.cumulative_cache_read_tokens = 0;
        self.cumulative_cache_creation_tokens = 0;
        self.doom_monitor.clear();
        self.budget_warned_pcts.clear();
    }

    /// Push a user message directly into history without running the completion loop.
    /// Used by the TUI to inject notes/hints mid-conversation.
    pub fn push_user_message(&mut self, text: impl Into<String>) {
        use clido_core::{ContentBlock, Message, Role};
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        };
        self.history.push(msg);
    }

    /// Extract a file path from tool input JSON for permission requests.
    fn extract_path_from_input(input: &serde_json::Value) -> Option<std::path::PathBuf> {
        // Try common path field names (Read/Write/Edit use `file_path`).
        for key in &[
            "file_path",
            "path",
            "file",
            "target",
            "source",
            "dest",
            "destination",
        ] {
            if let Some(path_str) = input.get(key).and_then(|v| v.as_str()) {
                return Some(std::path::PathBuf::from(path_str));
            }
        }
        // For Glob/SemanticSearch, try "pattern" or "query"
        if let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) {
            return Some(std::path::PathBuf::from(pattern));
        }
        None
    }

    /// Determine if a tool error is retryable and what strategy to use.
    fn classify_retry_strategy(&self, tool_name: &str, error: &str) -> Option<RetryStrategy> {
        let err_lower = error.to_lowercase();

        // Network/timeout errors - always retryable
        if err_lower.contains("timeout")
            || err_lower.contains("timed out")
            || err_lower.contains("connection")
            || err_lower.contains("network")
            || err_lower.contains("temporarily unavailable")
            || err_lower.contains("rate limit")
            || err_lower.contains("too many requests")
        {
            return Some(RetryStrategy::WaitAndRetry { delay_ms: 1000 });
        }

        // File not found - might be race condition or typo
        if err_lower.contains("no such file")
            || err_lower.contains("file not found")
            || err_lower.contains("cannot find")
            || err_lower.contains("does not exist")
        {
            // Only retry Read, Glob, Grep operations - not Write/Edit
            if matches!(
                tool_name,
                "Read" | "Glob" | "Grep" | "Ls" | "SemanticSearch"
            ) {
                return Some(RetryStrategy::RetryOnce);
            }
        }

        // Permission denied - might be transient
        if err_lower.contains("permission denied")
            || err_lower.contains("access denied")
            || err_lower.contains("unauthorized")
        {
            return Some(RetryStrategy::RetryOnce);
        }

        // Bash transient errors
        if tool_name == "Bash"
            && (err_lower.contains("resource temporarily unavailable")
                || err_lower.contains("try again")
                || err_lower.contains("device or resource busy"))
        {
            return Some(RetryStrategy::WaitAndRetry { delay_ms: 500 });
        }

        // Web fetch/search specific errors
        if matches!(tool_name, "WebFetch" | "WebSearch")
            && (err_lower.contains("dns")
                || err_lower.contains("resolve")
                || err_lower.contains("certificate")
                || err_lower.contains("ssl"))
        {
            return Some(RetryStrategy::RetryOnce);
        }

        // Generic I/O and connection flakiness
        if err_lower.contains("i/o error")
            || err_lower.contains("io error")
            || err_lower.contains("os error")
            || err_lower.contains("broken pipe")
            || err_lower.contains("connection reset")
            || err_lower.contains("resource temporarily unavailable")
        {
            return Some(RetryStrategy::WaitAndRetry { delay_ms: 400 });
        }

        None // Not retryable
    }

    /// Execute a tool with automatic retry logic for transient failures.
    async fn execute_tool_with_retry(
        &mut self,
        name: &str,
        input: &serde_json::Value,
    ) -> ToolOutput {
        let max_retries = self.max_tool_retries;
        let mut last_output: Option<ToolOutput> = None;

        for attempt in 0..=max_retries {
            let output = self.execute_tool_maybe_gated(name, input).await;

            if !output.is_error {
                // Success - clear retry tracking for this tool
                let key = format!(
                    "{}:{}",
                    name,
                    serde_json::to_string(input).unwrap_or_default()
                );
                self.retry_attempts.remove(&key);
                return output;
            }

            // Check for "path outside working directory" error - request interactive permission
            if output.content.contains("path outside working directory")
                || output.content.contains("Access denied: path outside")
            {
                if let Some(ref mut rx) = self.path_permission_rx {
                    // Extract the path from the error or input
                    let requested_path = Self::extract_path_from_input(input).unwrap_or_default();

                    // Emit request to TUI (modal y/n/a), not only as "thinking" text.
                    if let Some(ref e) = self.emit {
                        e.on_path_permission_request(&requested_path, name).await;
                    }

                    // Wait for user response (with timeout)
                    match tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv()).await
                    {
                        Ok(Some(granted_path)) if !granted_path.as_os_str().is_empty() => {
                            // User granted permission - the TUI will have updated allowed paths
                            // and rebuilt the tools. We need to retry with the new registry.
                            if let Some(ref e) = self.emit {
                                let _ = e
                                    .on_assistant_text(&format!(
                                        "[Permission granted for: {}]",
                                        granted_path.display()
                                    ))
                                    .await;
                            }
                            // Return a special output indicating we need to retry with new tools
                            // The caller (execute_tool_batch_with_retry) will handle the actual retry
                            return ToolOutput {
                                content: format!(
                                    "External path access granted by user. Retrying with: {}",
                                    granted_path.display()
                                ),
                                is_error: false,
                                path: None,
                                content_hash: None,
                                mtime_nanos: None,
                                diff: None,
                            };
                        }
                        Ok(Some(_)) => {
                            // User denied (empty path means denial)
                            return ToolOutput {
                                content: format!(
                                    "{}\n\n[User denied access to external path]",
                                    output.content
                                ),
                                is_error: true,
                                path: None,
                                content_hash: None,
                                mtime_nanos: None,
                                diff: None,
                            };
                        }
                        Ok(None) => {
                            // Channel closed
                            return output;
                        }
                        Err(_) => {
                            // Timeout
                            return ToolOutput {
                                content: format!(
                                    "{}\n\n[Permission request timed out after 60s]",
                                    output.content
                                ),
                                is_error: true,
                                path: None,
                                content_hash: None,
                                mtime_nanos: None,
                                diff: None,
                            };
                        }
                    }
                }
            }

            // Check if we've exhausted retries (last iteration)
            if attempt == max_retries {
                // Max retries exceeded - return last error with context
                if let Some(mut final_output) = last_output {
                    final_output.content = format!(
                        "{}\n\n[Auto-retry exhausted after {} attempts]",
                        final_output.content,
                        max_retries + 1
                    );
                    return final_output;
                }
                return output;
            }

            // Check if this error is retryable
            match self.classify_retry_strategy(name, &output.content) {
                Some(strategy) => {
                    let key = format!(
                        "{}:{}",
                        name,
                        serde_json::to_string(input).unwrap_or_default()
                    );
                    self.retry_attempts.insert(key.clone(), attempt);

                    // Log retry attempt (attempt+1 since we're about to retry)
                    if let Some(ref e) = self.emit {
                        let _ = e
                            .on_assistant_text(&format!(
                                "[Tool '{}' failed, retrying ({}/{})...]",
                                name,
                                attempt + 1,
                                max_retries
                            ))
                            .await;
                    }

                    // Apply retry strategy
                    match strategy {
                        RetryStrategy::WaitAndRetry { delay_ms } => {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        RetryStrategy::RetryOnce => {
                            // No delay for simple retry
                        }
                    }

                    last_output = Some(output);
                    // Continue to next retry attempt
                }
                None => {
                    // Not retryable - return error immediately
                    return output;
                }
            }
        }

        unreachable!()
    }

    /// Execute a batch of tools with automatic retry for failed calls.
    /// First runs all tools in parallel (if read-only), then retries any failures.
    async fn execute_tool_batch_with_retry(
        &mut self,
        tool_uses: &[(String, String, serde_json::Value)],
    ) -> Vec<ToolOutput> {
        // First pass: execute all tools using the optimized batch method
        let mut results = self.execute_tool_batch(tool_uses).await;

        // Second pass: retry any failures
        for (i, result) in results.iter_mut().enumerate() {
            if result.is_error {
                let (_, name, input) = &tool_uses[i];
                *result = self.execute_tool_with_retry(name, input).await;
            }
        }

        results
    }

    /// Immediately compact the conversation history, regardless of the compaction threshold.
    /// Returns `(before, after)` message counts. Useful for the `/compact` TUI command.
    pub async fn compact_history_now(&mut self) -> Result<(usize, usize)> {
        let before = self.history.len();
        let sys_tokens = self
            .config
            .system_prompt
            .as_ref()
            .map(|s| estimate_tokens_str(s))
            .unwrap_or(0);
        let max_ctx = self
            .config
            .max_context_tokens
            .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS);
        // Pass threshold=0 to force compaction unconditionally.
        let (util_provider, summarize_config) = self.utility_provider();
        let compacted = compact_for_model_request(
            &self.history,
            sys_tokens,
            max_ctx,
            0.0,
            util_provider.as_ref(),
            &summarize_config,
        )
        .await?;
        let after = compacted.len();
        self.history = compacted;
        Ok((before, after))
    }

    /// Make a single LLM completion call with no tools — used for planning.
    /// Returns the first text block from the response, or an error.
    pub async fn complete_simple(&self, prompt: &str) -> clido_core::Result<String> {
        self.complete_simple_with_usage(prompt)
            .await
            .map(|(text, _)| text)
    }

    /// Like [`complete_simple`](Self::complete_simple) but also returns provider token usage.
    pub async fn complete_simple_with_usage(
        &self,
        prompt: &str,
    ) -> clido_core::Result<(String, clido_core::Usage)> {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
        }];
        let response = self.provider.complete(&messages, &[], &self.config).await?;
        let text = response
            .content
            .iter()
            .find_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok((text, response.usage))
    }

    /// Send a user prompt to the utility (fast) provider and return the text response.
    /// Falls back to the main provider if no fast provider is configured.
    pub async fn complete_simple_fast(&self, prompt: &str) -> clido_core::Result<String> {
        self.complete_simple_fast_with_usage(prompt)
            .await
            .map(|(text, _)| text)
    }

    /// Send a user prompt to the utility (fast) provider and return the text response
    /// together with token usage. Falls back to the main provider if no fast provider
    /// is configured.
    pub async fn complete_simple_fast_with_usage(
        &self,
        prompt: &str,
    ) -> clido_core::Result<(String, clido_core::Usage)> {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
        }];
        let (util_provider, config) = self.utility_provider();
        let response = util_provider
            .as_ref()
            .complete(&messages, &[], &config)
            .await?;
        let text = response
            .content
            .iter()
            .find_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok((text, response.usage))
    }

    /// Send a user prompt to the utility provider with a custom system prompt.
    /// Used for prompt enhancement and other utility tasks that need steering.
    pub async fn complete_with_system_fast(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> clido_core::Result<String> {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_prompt.to_string(),
            }],
        }];
        let (util_provider, mut config) = self.utility_provider();
        config.system_prompt = Some(system_prompt.to_string());
        let response = util_provider
            .as_ref()
            .complete(&messages, &[], &config)
            .await?;
        let text = response
            .content
            .iter()
            .find_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok(text)
    }

    /// Use the fast/utility provider to generate a plan for complex prompts.
    /// Returns None if the prompt is too simple or planning fails.
    async fn architect_plan(&self, user_input: &str) -> Option<String> {
        let (util_provider, util_config) = self.utility_provider();
        planning::architect_plan(user_input, &util_config, util_provider.as_ref()).await
    }

    /// Continue from existing history (resume). Does not push a new user message; runs the loop until EndTurn or max_turns.
    pub async fn run_continue(
        &mut self,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let schemas = self.tools.schemas();
        let mut turns = 0;
        self.cumulative_cost_usd = 0.0;
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
        self.cumulative_cache_read_tokens = 0;
        self.cumulative_cache_creation_tokens = 0;
        const DEFAULT_INPUT_USD_PER_1M: f64 = 3.0;
        const DEFAULT_OUTPUT_USD_PER_1M: f64 = 15.0;

        loop {
            if cancel
                .as_ref()
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                return Err(ClidoError::Interrupted);
            }
            if turns >= self.config.max_turns {
                return Err(ClidoError::MaxTurnsExceeded);
            }
            turns += 1;
            self.last_turn_count = turns;

            // Proactive summarization: at 50% capacity, start replacing oldest tool pairs
            // with 1-sentence summaries to delay full compaction.
            {
                let sys_tok = self
                    .config
                    .system_prompt
                    .as_ref()
                    .map(|s| estimate_tokens_str(s))
                    .unwrap_or(0);
                let max_tok = self
                    .config
                    .max_context_tokens
                    .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS);
                let effective_max = max_tok.saturating_sub(CONTEXT_OUTPUT_RESERVE).max(32_000);
                let current = sys_tok + estimate_tokens_messages(&self.history);
                let proactive_limit =
                    ((effective_max as f64) * PROACTIVE_SUMMARIZE_THRESHOLD) as u32;
                if current > proactive_limit {
                    let (util_prov, util_cfg) = self.utility_provider();
                    let count = context::proactive_summarize_pairs(
                        &mut self.history,
                        util_prov.as_ref(),
                        &util_cfg,
                        8, // preserve last 8 messages
                    )
                    .await;
                    if count > 0 {
                        debug!("Proactively summarized {} tool pairs", count);
                    }
                }
            }

            let system_tokens = self
                .config
                .system_prompt
                .as_ref()
                .map(|s| estimate_tokens_str(s))
                .unwrap_or(0);
            let max_ctx = self
                .config
                .max_context_tokens
                .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS);
            let threshold = self
                .config
                .compaction_threshold
                .unwrap_or(DEFAULT_COMPACTION_THRESHOLD);
            let (util_prov, summarize_config) = self.utility_provider();
            let to_send = compact_for_model_request(
                &self.history,
                system_tokens,
                max_ctx,
                threshold,
                util_prov.as_ref(),
                &summarize_config,
            )
            .await?;

            let response = self
                .provider
                .complete(&to_send, &schemas, &self.config)
                .await?;

            // Cancel check after the blocking LLM call (10-60s).
            if cancel
                .as_ref()
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                return Err(ClidoError::Interrupted);
            }

            let turn_cost = pricing
                .map(|t| compute_cost_usd(&response.usage, &self.config.model, t))
                .unwrap_or_else(|| {
                    (response.usage.input_tokens as f64 * DEFAULT_INPUT_USD_PER_1M
                        + response.usage.output_tokens as f64 * DEFAULT_OUTPUT_USD_PER_1M)
                        / 1_000_000.0
                });
            self.cumulative_cost_usd += turn_cost;
            self.cumulative_input_tokens += response.usage.input_tokens;
            self.cumulative_output_tokens += response.usage.output_tokens;
            self.cumulative_cache_read_tokens +=
                response.usage.cache_read_input_tokens.unwrap_or(0);
            self.cumulative_cache_creation_tokens +=
                response.usage.cache_creation_input_tokens.unwrap_or(0);

            self.check_budget_exceeded()?;

            self.history.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
            });

            if let Some(ref mut w) = session {
                let content: Vec<serde_json::Value> = response
                    .content
                    .iter()
                    .filter_map(|b| serde_json::to_value(b).ok())
                    .collect();
                w.log_write_line(&SessionLine::AssistantMessage { content });
            }

            match response.stop_reason {
                StopReason::EndTurn => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(text.trim().to_string());
                }
                StopReason::ToolUse => {
                    let tool_uses: Vec<(String, String, serde_json::Value)> = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse { id, name, input } = b {
                                Some((id.clone(), name.clone(), input.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if let Some(ref mut w) = session {
                        for (id, name, input) in &tool_uses {
                            w.log_write_line(&SessionLine::ToolCall {
                                tool_use_id: id.clone(),
                                tool_name: name.clone(),
                                input: input.clone(),
                            });
                        }
                    }

                    let all_read_only = tool_uses.iter().all(|(_, name, _)| {
                        self.tools
                            .get(name)
                            .map(|t| t.is_read_only())
                            .unwrap_or(false)
                    });

                    let outputs: Vec<(ToolOutput, u64)> = if all_read_only && tool_uses.len() > 1 {
                        if let Some(ref e) = self.emit {
                            for (id, name, input) in &tool_uses {
                                e.on_tool_start(id, name, input).await;
                            }
                        }
                        for (_, name, input) in &tool_uses {
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.pre_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                        ],
                                    );
                                }
                            }
                        }
                        let t0 = std::time::Instant::now();
                        let results = self.execute_tool_batch_with_retry(&tool_uses).await;
                        let batch_ms = t0.elapsed().as_millis() as u64;
                        if let Some(ref e) = self.emit {
                            for ((id, name, _), output) in tool_uses.iter().zip(results.iter()) {
                                e.on_tool_done(id, name, output.is_error, output.diff.clone())
                                    .await;
                            }
                        }
                        for ((_, name, input), output) in tool_uses.iter().zip(results.iter()) {
                            self.write_audit(name, input, output, batch_ms);
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.post_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                            (
                                                "CLIDO_TOOL_OUTPUT",
                                                &output
                                                    .content
                                                    .chars()
                                                    .take(500)
                                                    .collect::<String>(),
                                            ),
                                            (
                                                "CLIDO_TOOL_IS_ERROR",
                                                if output.is_error { "true" } else { "false" },
                                            ),
                                            ("CLIDO_TOOL_DURATION_MS", &batch_ms.to_string()),
                                        ],
                                    );
                                }
                            }
                        }
                        results.into_iter().map(|o| (o, batch_ms)).collect()
                    } else {
                        let mut outputs = Vec::new();
                        for (id, name, input) in &tool_uses {
                            if let Some(ref e) = self.emit {
                                e.on_tool_start(id, name, input).await;
                            }
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.pre_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                        ],
                                    );
                                }
                            }
                            let t0 = std::time::Instant::now();
                            let output = self.execute_tool_with_retry(name, input).await;
                            let duration_ms = t0.elapsed().as_millis() as u64;
                            if let Some(ref e) = self.emit {
                                e.on_tool_done(id, name, output.is_error, output.diff.clone())
                                    .await;
                            }
                            self.write_audit(name, input, &output, duration_ms);
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.post_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                            (
                                                "CLIDO_TOOL_OUTPUT",
                                                &output
                                                    .content
                                                    .chars()
                                                    .take(500)
                                                    .collect::<String>(),
                                            ),
                                            (
                                                "CLIDO_TOOL_IS_ERROR",
                                                if output.is_error { "true" } else { "false" },
                                            ),
                                            ("CLIDO_TOOL_DURATION_MS", &duration_ms.to_string()),
                                        ],
                                    );
                                }
                            }
                            outputs.push((output, duration_ms));
                        }
                        outputs
                    };

                    let mut tool_results = Vec::new();
                    let mut had_errors = false;
                    for ((id, name, input), (output, duration_ms)) in
                        tool_uses.iter().zip(outputs.iter())
                    {
                        if let Some(ref mut w) = session {
                            w.log_write_line(&SessionLine::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content.clone(),
                                is_error: output.is_error,
                                duration_ms: Some(*duration_ms),
                                path: output.path.clone(),
                                content_hash: output.content_hash.clone(),
                                mtime_nanos: output.mtime_nanos,
                            });
                        }
                        if output.is_error {
                            had_errors = true;
                        }
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: if output.is_error {
                                enhanced_edit_error(name, &output.content, input)
                            } else {
                                output.content.clone()
                            },
                            is_error: output.is_error,
                        });
                    }
                    prepend_tool_recovery_nudge(&tool_uses, &outputs, &mut tool_results);
                    // Track consecutive tool errors for escalating hints.
                    if had_errors {
                        self.consecutive_tool_errors += 1;
                        if self.consecutive_tool_errors >= 3 {
                            tool_results.push(ContentBlock::Text {
                                text: format!(
                                    "[Warning] You've had {} consecutive turns with tool errors. \
                                     Step back and reconsider your approach before trying again.",
                                    self.consecutive_tool_errors
                                ),
                            });
                        }
                    } else {
                        self.consecutive_tool_errors = 0;
                    }
                    self.history.push(Message {
                        role: Role::User,
                        content: tool_results,
                    });
                }
                StopReason::MaxTokens | StopReason::StopSequence => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(text.trim().to_string());
                }
            }
        }
    }

    /// Push a new user message and run until EndTurn (for REPL next turn).
    pub async fn run_next_turn(
        &mut self,
        user_input: &str,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let history_before = self.history.len();
        let user_msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_input.to_string(),
            }],
        };
        self.history.push(user_msg.clone());

        if let Some(ref mut w) = session {
            let content: Vec<serde_json::Value> = user_msg
                .content
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect();
            w.log_write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content,
            });
        }

        let result = self.run_completion_loop(session, pricing, cancel).await;
        match &result {
            Ok(_) => self.prune_memory_if_needed(),
            Err(e) if e.should_truncate_history_after_failed_run(self.history.len(), history_before) => {
                self.history.truncate(history_before);
            }
            Err(_) => {}
        }
        result
    }

    /// Run until stop_reason is EndTurn or max_turns reached.
    /// If `session` is Some, writes UserMessage, AssistantMessage, ToolCall, ToolResult each turn.
    /// If `pricing` is Some, uses it for cost and budget; updates self.cumulative_cost_usd.
    pub async fn run(
        &mut self,
        user_input: &str,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        // Inject relevant memories into system prompt before running.
        if let Some(injected) = self.inject_memories(user_input) {
            self.config.system_prompt = Some(injected);
        }
        // Refresh git context each turn so the model always sees the current branch/diff.
        let base_for_git = self.config.system_prompt.clone().unwrap_or_default();
        if let Some(with_git) = self.inject_git_context(&base_for_git) {
            self.config.system_prompt = Some(with_git);
        }

        let history_before = self.history.len();

        // Architect→Editor pipeline: if reasoning model is configured, generate a plan
        // and prepend it to the user message so the editor model has structured guidance.
        let plan_prefix = self.architect_plan(user_input).await;
        let effective_input = if let Some(ref plan) = plan_prefix {
            format!(
                "<architect_plan>\n{}\n</architect_plan>\n\n{}",
                plan, user_input
            )
        } else {
            user_input.to_string()
        };

        let user_msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: effective_input,
            }],
        };
        self.history.push(user_msg.clone());

        if let Some(ref mut w) = session {
            let content: Vec<serde_json::Value> = user_msg
                .content
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect();
            w.log_write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content,
            });
        }

        let result = self.run_completion_loop(session, pricing, cancel).await;
        match &result {
            Ok(_) => self.prune_memory_if_needed(),
            Err(e) if e.should_truncate_history_after_failed_run(self.history.len(), history_before) => {
                self.history.truncate(history_before);
            }
            Err(_) => {}
        }
        result
    }

    /// Like `run`, but prepends `extra_blocks` (e.g. image blocks) before the text block.
    pub async fn run_with_extra_blocks(
        &mut self,
        user_input: &str,
        extra_blocks: Vec<ContentBlock>,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        if let Some(injected) = self.inject_memories(user_input) {
            self.config.system_prompt = Some(injected);
        }
        let base_for_git = self.config.system_prompt.clone().unwrap_or_default();
        if let Some(with_git) = self.inject_git_context(&base_for_git) {
            self.config.system_prompt = Some(with_git);
        }

        let history_before = self.history.len();
        let mut content = extra_blocks;
        content.push(ContentBlock::Text {
            text: user_input.to_string(),
        });
        let user_msg = Message {
            role: Role::User,
            content,
        };
        self.history.push(user_msg.clone());

        if let Some(ref mut w) = session {
            let content_json: Vec<serde_json::Value> = user_msg
                .content
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect();
            w.log_write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content: content_json,
            });
        }

        let result = self.run_completion_loop(session, pricing, cancel).await;
        match &result {
            Ok(_) => self.prune_memory_if_needed(),
            Err(e) if e.should_truncate_history_after_failed_run(self.history.len(), history_before) => {
                self.history.truncate(history_before);
            }
            Err(_) => {}
        }
        result
    }

    /// Like `run_next_turn`, but prepends `extra_blocks` before the text block.
    pub async fn run_next_turn_with_extra_blocks(
        &mut self,
        user_input: &str,
        extra_blocks: Vec<ContentBlock>,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let history_before = self.history.len();
        let mut content = extra_blocks;
        content.push(ContentBlock::Text {
            text: user_input.to_string(),
        });
        let user_msg = Message {
            role: Role::User,
            content,
        };
        self.history.push(user_msg.clone());

        if let Some(ref mut w) = session {
            let content_json: Vec<serde_json::Value> = user_msg
                .content
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect();
            w.log_write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content: content_json,
            });
        }

        let result = self.run_completion_loop(session, pricing, cancel).await;
        match &result {
            Ok(_) => self.prune_memory_if_needed(),
            Err(e) if e.should_truncate_history_after_failed_run(self.history.len(), history_before) => {
                self.history.truncate(history_before);
            }
            Err(_) => {}
        }
        result
    }

    async fn run_completion_loop(
        &mut self,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let effective_mode = self
            .permission_mode_override
            .unwrap_or(self.config.permission_mode);
        let in_plan_mode = effective_mode == PermissionMode::PlanOnly;
        let schemas = self.tools.schemas_for_context(in_plan_mode);
        let mut turns = 0;
        self.cumulative_cost_usd = 0.0;
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
        self.cumulative_cache_read_tokens = 0;
        self.cumulative_cache_creation_tokens = 0;
        self.doom_monitor.clear();
        self.budget_warned_pcts.clear();
        const DEFAULT_INPUT_USD_PER_1M: f64 = 3.0;
        const DEFAULT_OUTPUT_USD_PER_1M: f64 = 15.0;

        loop {
            if cancel
                .as_ref()
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                return Err(ClidoError::Interrupted);
            }
            if turns >= self.config.max_turns {
                return Err(ClidoError::MaxTurnsExceeded);
            }
            turns += 1;
            self.last_turn_count = turns;

            // Proactive summarization: at 50% capacity, start replacing oldest tool pairs
            // with 1-sentence summaries to delay full compaction.
            {
                let sys_tok = self
                    .config
                    .system_prompt
                    .as_ref()
                    .map(|s| estimate_tokens_str(s))
                    .unwrap_or(0);
                let max_tok = self
                    .config
                    .max_context_tokens
                    .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS);
                let effective_max = max_tok.saturating_sub(CONTEXT_OUTPUT_RESERVE).max(32_000);
                let current = sys_tok + estimate_tokens_messages(&self.history);
                let proactive_limit =
                    ((effective_max as f64) * PROACTIVE_SUMMARIZE_THRESHOLD) as u32;
                if current > proactive_limit {
                    let (util_prov, util_cfg) = self.utility_provider();
                    let count = context::proactive_summarize_pairs(
                        &mut self.history,
                        util_prov.as_ref(),
                        &util_cfg,
                        8, // preserve last 8 messages
                    )
                    .await;
                    if count > 0 {
                        debug!("Proactively summarized {} tool pairs", count);
                    }
                }
            }

            let system_tokens = self
                .config
                .system_prompt
                .as_ref()
                .map(|s| estimate_tokens_str(s))
                .unwrap_or(0);
            let max_ctx = self
                .config
                .max_context_tokens
                .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS);
            let threshold = self
                .config
                .compaction_threshold
                .unwrap_or(DEFAULT_COMPACTION_THRESHOLD);
            let (util_prov, summarize_config) = self.utility_provider();
            let to_send = compact_for_model_request(
                &self.history,
                system_tokens,
                max_ctx,
                threshold,
                util_prov.as_ref(),
                &summarize_config,
            )
            .await?;

            let response = self
                .provider
                .complete(&to_send, &schemas, &self.config)
                .await?;

            // ── Cancel check after the blocking LLM call ──────────────────
            // provider.complete() can take 10-60s. The user may have pressed
            // /stop during that window — bail out immediately instead of
            // continuing to process the response and run tools.
            if cancel
                .as_ref()
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                return Err(ClidoError::Interrupted);
            }

            let turn_cost = pricing
                .map(|t| compute_cost_usd(&response.usage, &self.config.model, t))
                .unwrap_or_else(|| {
                    (response.usage.input_tokens as f64 * DEFAULT_INPUT_USD_PER_1M
                        + response.usage.output_tokens as f64 * DEFAULT_OUTPUT_USD_PER_1M)
                        / 1_000_000.0
                });
            self.cumulative_cost_usd += turn_cost;
            self.cumulative_input_tokens += response.usage.input_tokens;
            self.cumulative_output_tokens += response.usage.output_tokens;
            self.cumulative_cache_read_tokens +=
                response.usage.cache_read_input_tokens.unwrap_or(0);
            self.cumulative_cache_creation_tokens +=
                response.usage.cache_creation_input_tokens.unwrap_or(0);

            // Budget hard stop
            self.check_budget_exceeded()?;
            // Budget warnings at 50%, 80%, 90% of limit
            if let (Some(limit), Some(ref e)) = (self.config.max_budget_usd, &self.emit) {
                let pct_used = (self.cumulative_cost_usd / limit * 100.0).floor() as u8;
                for &threshold_pct in BUDGET_WARNING_PCTS {
                    if pct_used >= threshold_pct
                        && !self.budget_warned_pcts.contains(&threshold_pct)
                    {
                        self.budget_warned_pcts.push(threshold_pct);
                        e.on_budget_warning(threshold_pct, self.cumulative_cost_usd, limit)
                            .await;
                    }
                }
            }

            debug!(
                "turn {} stop_reason={:?} usage={}/{}",
                turns,
                response.stop_reason,
                response.usage.input_tokens,
                response.usage.output_tokens
            );

            // Append assistant message
            self.history.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
            });

            if let Some(ref mut w) = session {
                let content: Vec<serde_json::Value> = response
                    .content
                    .iter()
                    .filter_map(|b| serde_json::to_value(b).ok())
                    .collect();
                w.log_write_line(&SessionLine::AssistantMessage { content });
            }

            match response.stop_reason {
                StopReason::EndTurn => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(text.trim().to_string());
                }
                StopReason::ToolUse => {
                    // Emit any text blocks the model produced before/alongside tool calls.
                    let thinking: String = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !thinking.trim().is_empty() {
                        if let Some(ref e) = self.emit {
                            e.on_assistant_text(&thinking).await;
                        }
                    }

                    let tool_uses: Vec<(String, String, serde_json::Value)> = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse { id, name, input } = b {
                                Some((id.clone(), name.clone(), input.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if let Some(ref mut w) = session {
                        for (id, name, input) in &tool_uses {
                            w.log_write_line(&SessionLine::ToolCall {
                                tool_use_id: id.clone(),
                                tool_name: name.clone(),
                                input: input.clone(),
                            });
                        }
                    }

                    let all_read_only = tool_uses.iter().all(|(_, name, _)| {
                        self.tools
                            .get(name)
                            .map(|t| t.is_read_only())
                            .unwrap_or(false)
                    });

                    let outputs: Vec<(ToolOutput, u64)> = if all_read_only && tool_uses.len() > 1 {
                        if let Some(ref e) = self.emit {
                            for (id, name, input) in &tool_uses {
                                e.on_tool_start(id, name, input).await;
                            }
                        }
                        for (_, name, input) in &tool_uses {
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.pre_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                        ],
                                    );
                                }
                            }
                        }
                        let t0 = std::time::Instant::now();
                        let results = self.execute_tool_batch_with_retry(&tool_uses).await;
                        let batch_ms = t0.elapsed().as_millis() as u64;
                        if let Some(ref e) = self.emit {
                            for ((id, name, _), output) in tool_uses.iter().zip(results.iter()) {
                                e.on_tool_done(id, name, output.is_error, output.diff.clone())
                                    .await;
                            }
                        }
                        for ((_, name, input), output) in tool_uses.iter().zip(results.iter()) {
                            self.write_audit(name, input, output, batch_ms);
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.post_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                            (
                                                "CLIDO_TOOL_OUTPUT",
                                                &output
                                                    .content
                                                    .chars()
                                                    .take(500)
                                                    .collect::<String>(),
                                            ),
                                            (
                                                "CLIDO_TOOL_IS_ERROR",
                                                if output.is_error { "true" } else { "false" },
                                            ),
                                            ("CLIDO_TOOL_DURATION_MS", &batch_ms.to_string()),
                                        ],
                                    );
                                }
                            }
                        }
                        results.into_iter().map(|o| (o, batch_ms)).collect()
                    } else {
                        let mut outputs = Vec::new();
                        for (id, name, input) in &tool_uses {
                            if let Some(ref e) = self.emit {
                                e.on_tool_start(id, name, input).await;
                            }
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.pre_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                        ],
                                    );
                                }
                            }
                            let t0 = std::time::Instant::now();
                            let output = self.execute_tool_with_retry(name, input).await;
                            let duration_ms = t0.elapsed().as_millis() as u64;
                            if let Some(ref e) = self.emit {
                                e.on_tool_done(id, name, output.is_error, output.diff.clone())
                                    .await;
                            }
                            self.write_audit(name, input, &output, duration_ms);
                            if let Some(ref hooks) = self.hooks {
                                if let Some(cmd) = &hooks.post_tool_use {
                                    run_hook(
                                        cmd,
                                        &[
                                            ("CLIDO_TOOL_NAME", name.as_str()),
                                            (
                                                "CLIDO_TOOL_INPUT",
                                                &serde_json::to_string(input).unwrap_or_default(),
                                            ),
                                            (
                                                "CLIDO_TOOL_OUTPUT",
                                                &output
                                                    .content
                                                    .chars()
                                                    .take(500)
                                                    .collect::<String>(),
                                            ),
                                            (
                                                "CLIDO_TOOL_IS_ERROR",
                                                if output.is_error { "true" } else { "false" },
                                            ),
                                            ("CLIDO_TOOL_DURATION_MS", &duration_ms.to_string()),
                                        ],
                                    );
                                }
                            }
                            outputs.push((output, duration_ms));
                        }
                        outputs
                    };

                    let mut tool_results = Vec::new();
                    let mut had_errors = false;
                    for ((id, name, input), (output, duration_ms)) in
                        tool_uses.iter().zip(outputs.iter())
                    {
                        if let Some(ref mut w) = session {
                            w.log_write_line(&SessionLine::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content.clone(),
                                is_error: output.is_error,
                                duration_ms: Some(*duration_ms),
                                path: output.path.clone(),
                                content_hash: output.content_hash.clone(),
                                mtime_nanos: output.mtime_nanos,
                            });
                        }
                        if output.is_error {
                            had_errors = true;
                        }
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: if output.is_error {
                                enhanced_edit_error(name, &output.content, input)
                            } else {
                                output.content.clone()
                            },
                            is_error: output.is_error,
                        });

                        // Doom-loop detection: N consecutive identical failures from
                        // the same tool → stop rather than keep spending tokens in a loop.
                        if output.is_error {
                            let key = format!(
                                "{}:{}",
                                name,
                                output.content.chars().take(120).collect::<String>()
                            );
                            self.doom_monitor.push_back(key.clone());
                            if self.doom_monitor.len() > DOOM_LOOP_THRESHOLD {
                                self.doom_monitor.pop_front();
                            }
                            if self.doom_monitor.len() >= DOOM_LOOP_THRESHOLD
                                && self.doom_monitor.iter().all(|k| k == &key)
                            {
                                return Err(ClidoError::DoomLoop {
                                    tool: name.clone(),
                                    error: output.content.chars().take(200).collect::<String>(),
                                });
                            }
                        } else {
                            // A successful tool call resets the doom counter.
                            self.doom_monitor.clear();
                        }
                    }
                    prepend_tool_recovery_nudge(&tool_uses, &outputs, &mut tool_results);
                    // Track consecutive tool errors for escalating hints.
                    if had_errors {
                        self.consecutive_tool_errors += 1;
                        if self.consecutive_tool_errors >= 3 {
                            tool_results.push(ContentBlock::Text {
                                text: format!(
                                    "[Warning] You've had {} consecutive turns with tool errors. \
                                     Step back and reconsider your approach before trying again.",
                                    self.consecutive_tool_errors
                                ),
                            });
                        }
                    } else {
                        self.consecutive_tool_errors = 0;
                    }

                    // Check if cancelled after tool execution - if so, add results and exit cleanly
                    if cancel
                        .as_ref()
                        .map(|c| c.load(Ordering::Relaxed))
                        .unwrap_or(false)
                    {
                        // Add tool results to history before returning Interrupted
                        self.history.push(Message {
                            role: Role::User,
                            content: tool_results,
                        });
                        return Err(ClidoError::Interrupted);
                    }

                    self.history.push(Message {
                        role: Role::User,
                        content: tool_results,
                    });
                }
                StopReason::MaxTokens | StopReason::StopSequence => {
                    let text: String = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(text.trim().to_string());
                }
            }
        }
    }

    async fn execute_tool_maybe_gated(
        &mut self,
        name: &str,
        input: &serde_json::Value,
    ) -> ToolOutput {
        let effective = self
            .permission_mode_override
            .unwrap_or(self.config.permission_mode);

        if name == "ExitPlanMode" {
            self.permission_mode_override = Some(PermissionMode::Default);
            return self.execute_tool(name, input).await;
        }

        // Auto-checkpoint: create a git commit of dirty state before first write operation.
        if !self.checkpoint_created {
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    self.checkpoint_created = true;
                    if let Some(hash) =
                        maybe_create_checkpoint(std::env::current_dir().ok().as_deref()).await
                    {
                        debug!("Pre-edit git checkpoint created: {hash}");
                    }
                }
            }
        }

        // ── Prompt injection detection ────────────────────────────────────────
        // Warn on suspicious tool arguments; for write-capable tools, ask the user.
        if let Some(category) = detect_injection(input) {
            warn!(
                "Potential prompt injection detected in {} args: {}",
                name, category
            );
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    if let Some(ref ask) = self.ask_user {
                        let req = PermRequest {
                            tool_name: name.to_string(),
                            description: format!(
                                "⚠️ Potential prompt injection ({}) detected in tool arguments. Allow?",
                                category
                            ),
                            diff: None,
                            proposed_content: None,
                            file_path: None,
                        };
                        match ask.ask(req).await {
                            PermGrant::Allow | PermGrant::AllowAll => {}
                            PermGrant::Deny | PermGrant::EditInEditor => {
                                return ToolOutput::err(format!(
                                    "Blocked: potential prompt injection ({}) in tool arguments.",
                                    category
                                ));
                            }
                            PermGrant::DenyWithFeedback(msg) => {
                                return ToolOutput::err(msg);
                            }
                        }
                    }
                }
            }
        }

        // ── Per-file permission rules ────────────────────────────────────────
        // If the config has `permission_rules`, evaluate them against the tool's
        // primary file argument (the first string value in `input`) before falling
        // through to the mode-level logic.
        if !self.config.permission_rules.is_empty() {
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    // Extract the primary file argument: first string value in the
                    // input object, or the value of a key named "path" / "file".
                    let file_arg = input
                        .get("path")
                        .or_else(|| input.get("file"))
                        .or_else(|| input.get("target"))
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            input
                                .as_object()
                                .and_then(|m| m.values().find_map(|v| v.as_str()))
                        })
                        .unwrap_or("");

                    if let Some((action, reason)) =
                        evaluate_rules(&self.config.permission_rules, file_arg)
                    {
                        match action {
                            RuleAction::Allow => {
                                // Rule explicitly allows — bypass mode checks.
                                return self.execute_tool(name, input).await;
                            }
                            RuleAction::Deny => {
                                let msg = match reason {
                                    Some(r) => format!("Permission denied by rule: {}", r),
                                    None => format!(
                                        "Permission denied by rule for '{}' on path '{}'",
                                        name, file_arg
                                    ),
                                };
                                return ToolOutput::err(msg);
                            }
                            RuleAction::Ask => {
                                // Fall through to interactive prompt below.
                            }
                        }
                    }
                }
            }
        }

        if effective == PermissionMode::PlanOnly {
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    return ToolOutput::err(
                        "In plan-only mode, only Read, Glob, and Grep are allowed. Use ExitPlanMode to switch.".to_string(),
                    );
                }
            }
        }
        if effective == PermissionMode::Default {
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    if let Some(ref ask) = self.ask_user {
                        let req = PermRequest {
                            tool_name: name.to_string(),
                            description: serde_json::to_string(input).unwrap_or_default(),
                            diff: None,
                            proposed_content: None,
                            file_path: None,
                        };
                        match ask.ask(req).await {
                            PermGrant::Allow => {}
                            PermGrant::AllowAll => {
                                // Persist: skip permission checks for all subsequent calls.
                                self.permission_mode_override = Some(PermissionMode::AcceptAll);
                            }
                            PermGrant::DenyWithFeedback(feedback) => {
                                return ToolOutput::err(format!(
                                    "User denied '{}': {feedback}",
                                    name
                                ));
                            }
                            PermGrant::Deny | PermGrant::EditInEditor => {
                                return ToolOutput::err(format!("User denied tool '{}'.", name));
                            }
                        }
                    } else {
                        // Non-interactive (no TTY / no ask_user): auto-deny state-changing tools
                        // in Default permission mode to prevent unattended writes.
                        // Use --permission-mode accept-all to allow writes in non-interactive mode.
                        return ToolOutput::err(format!(
                            "Tool '{}' requires user approval but no interactive terminal is available. \
                             Re-run with --permission-mode accept-all to allow state-changing tools \
                             in non-interactive mode.",
                            name
                        ));
                    }
                }
            }
        }

        if effective == PermissionMode::DiffReview {
            if let Some(tool) = self.tools.get(name) {
                if !tool.is_read_only() {
                    if let Some(ref ask) = self.ask_user {
                        // For Write/Edit, compute diff before asking
                        let (diff, proposed_content, file_path) =
                            compute_diff_for_tool(name, input).await;
                        let req = PermRequest {
                            tool_name: name.to_string(),
                            description: serde_json::to_string(input).unwrap_or_default(),
                            diff,
                            proposed_content: proposed_content.clone(),
                            file_path: file_path.clone(),
                        };
                        match ask.ask(req).await {
                            PermGrant::Allow => {}
                            PermGrant::AllowAll => {
                                self.permission_mode_override = Some(PermissionMode::AcceptAll);
                            }
                            PermGrant::DenyWithFeedback(feedback) => {
                                return ToolOutput::err(format!(
                                    "User rejected '{}': {feedback}",
                                    name
                                ));
                            }
                            PermGrant::Deny => {
                                return ToolOutput::err(format!(
                                    "User denied tool '{}' in diff-review mode.",
                                    name
                                ));
                            }
                            PermGrant::EditInEditor => {
                                // Open editor, then re-route to write the edited content
                                if let (Some(content), Some(path)) = (proposed_content, file_path) {
                                    match open_in_editor_blocking(&content, &path).await {
                                        Ok(edited) => {
                                            // Write the edited content directly
                                            let mut new_input = input.clone();
                                            if let Some(obj) = new_input.as_object_mut() {
                                                obj.insert(
                                                    "content".to_string(),
                                                    serde_json::Value::String(edited),
                                                );
                                            }
                                            return self.execute_tool(name, &new_input).await;
                                        }
                                        Err(e) => {
                                            return ToolOutput::err(format!(
                                                "Editor failed: {}",
                                                e
                                            ));
                                        }
                                    }
                                }
                                // If no proposed_content/file_path (e.g. EditTool), just proceed
                            }
                        }
                    }
                }
            }
        }

        self.execute_tool(name, input).await
    }

    /// Global tool execution timeout to prevent indefinite hangs.
    /// Individual tools (like Bash) may have shorter timeouts.
    const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

    async fn execute_tool(&self, name: &str, input: &serde_json::Value) -> ToolOutput {
        match self.tools.get(name) {
            Some(tool) => {
                // Wrap tool execution in a timeout to prevent hangs
                match tokio::time::timeout(Self::TOOL_TIMEOUT, tool.execute(input.clone())).await {
                    Ok(output) => output,
                    Err(_) => ToolOutput::err(format!(
                        "Tool '{}' timed out after {} seconds - operation took too long",
                        name,
                        Self::TOOL_TIMEOUT.as_secs()
                    )),
                }
            }
            None => ToolOutput::err(format!("Tool not found: {}", name)),
        }
    }

    /// Write an audit entry for a completed tool call.
    fn write_audit(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        output: &ToolOutput,
        duration_ms: u64,
    ) {
        if let Some(ref audit) = self.audit_log {
            let input_summary = clido_storage::redact_secrets(
                &serde_json::to_string(tool_input).unwrap_or_default(),
            )
            .chars()
            .take(200)
            .collect();
            let entry = AuditEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: String::new(),
                tool_name: tool_name.to_string(),
                input_summary,
                is_error: output.is_error,
                duration_ms,
            };
            let _ = audit.lock().unwrap().append(&entry);
        }
    }

    /// Execute a batch of tool calls, using parallel execution if all are read-only.
    /// Returns results in the same order as the input tool_uses slice.
    async fn execute_tool_batch(
        &self,
        tool_uses: &[(String, String, serde_json::Value)],
    ) -> Vec<ToolOutput> {
        let all_read_only = tool_uses.iter().all(|(_, name, _)| {
            self.tools
                .get(name)
                .map(|t| t.is_read_only())
                .unwrap_or(false)
        });

        if all_read_only && tool_uses.len() > 1 {
            // Parallel execution with bounded concurrency
            let max_parallel = self.config.max_parallel_tools.max(1) as usize;
            let semaphore = Arc::new(Semaphore::new(max_parallel));
            let tools = &self.tools;
            let futures: Vec<_> = tool_uses
                .iter()
                .map(|(_, name, input)| {
                    let sem = semaphore.clone();
                    let name = name.clone();
                    let input = input.clone();
                    async move {
                        let _permit = match sem.acquire().await {
                            Ok(p) => p,
                            Err(_) => {
                                return ToolOutput::err("internal: semaphore closed".to_string())
                            }
                        };
                        match tools.get(&name) {
                            Some(tool) => {
                                // Wrap in timeout like sequential execution
                                match tokio::time::timeout(
                                    Duration::from_secs(60),
                                    tool.execute(input),
                                )
                                .await
                                {
                                    Ok(output) => output,
                                    Err(_) => ToolOutput::err(format!(
                                        "Tool '{}' timed out after 60 seconds",
                                        name
                                    )),
                                }
                            }
                            None => ToolOutput::err(format!("Tool not found: {}", name)),
                        }
                    }
                })
                .collect();
            join_all(futures).await
        } else {
            // Sequential execution (state-changing or single tool)
            let mut results = Vec::with_capacity(tool_uses.len());
            for (_, name, input) in tool_uses {
                let output = self.execute_tool(name, input).await;
                results.push(output);
            }
            results
        }
    }
}

/// For Write/Edit tool calls in diff-review mode, extract the proposed file path
/// and content from the input JSON, read the current on-disk content, and compute
/// a unified diff.  Returns `(diff, proposed_content, file_path)`.
async fn compute_diff_for_tool(
    tool_name: &str,
    input: &serde_json::Value,
) -> (Option<String>, Option<String>, Option<std::path::PathBuf>) {
    match tool_name {
        "Write" | "write" => {
            let path_str = match input.get("file_path").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return (None, None, None),
            };
            let proposed = match input.get("content").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return (None, None, None),
            };
            let file_path = std::path::PathBuf::from(&path_str);
            let old_content = std::fs::read_to_string(&file_path).unwrap_or_default();
            let diff = TextDiff::from_lines(old_content.as_str(), proposed.as_str())
                .unified_diff()
                .header(&format!("a/{}", path_str), &format!("b/{}", path_str))
                .to_string();
            let diff = if diff.is_empty() { None } else { Some(diff) };
            (diff, Some(proposed), Some(file_path))
        }
        "Edit" | "edit" => {
            let path_str = match input.get("file_path").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return (None, None, None),
            };
            let old_str = match input.get("old_string").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return (None, None, None),
            };
            let new_str = match input.get("new_string").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return (None, None, None),
            };
            let file_path = std::path::PathBuf::from(&path_str);
            let old_content = std::fs::read_to_string(&file_path).unwrap_or_default();
            // Apply the edit to produce the full proposed file content.
            let proposed = old_content.replacen(old_str, new_str, 1);
            let diff = TextDiff::from_lines(old_content.as_str(), proposed.as_str())
                .unified_diff()
                .header(&format!("a/{}", path_str), &format!("b/{}", path_str))
                .to_string();
            let diff = if diff.is_empty() { None } else { Some(diff) };
            (diff, Some(proposed), Some(file_path))
        }
        _ => (None, None, None),
    }
}

/// Open proposed content in `$EDITOR` (fallback `$VISUAL`, then `vi`),
/// wait for the editor to exit, and return the saved content.
async fn open_in_editor_blocking(proposed: &str, file_path: &std::path::Path) -> Result<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    let suffix = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();

    let proposed = proposed.to_string();
    let editor_clone = editor.clone();

    tokio::task::spawn_blocking(move || {
        let tmp = tempfile::Builder::new()
            .suffix(&suffix)
            .tempfile()
            .map_err(|e| ClidoError::Other(anyhow::anyhow!("tempfile: {}", e)))?;

        std::fs::write(tmp.path(), &proposed)
            .map_err(|e| ClidoError::Other(anyhow::anyhow!("write tempfile: {}", e)))?;

        let status = std::process::Command::new(&editor_clone)
            .arg(tmp.path())
            .status()
            .map_err(|e| {
                ClidoError::Other(anyhow::anyhow!("spawn editor '{}': {}", editor_clone, e))
            })?;

        if !status.success() {
            return Err(ClidoError::Other(anyhow::anyhow!(
                "editor '{}' exited with non-zero status",
                editor_clone
            )));
        }

        std::fs::read_to_string(tmp.path())
            .map_err(|e| ClidoError::Other(anyhow::anyhow!("read tempfile: {}", e)))
    })
    .await
    .map_err(|e| ClidoError::Other(anyhow::anyhow!("spawn_blocking: {}", e)))?
}
/// Fire-and-forget hook execution.
///
/// stdio is redirected to /dev/null so hook output never corrupts the TUI's
/// alternate screen. The spawned child is not waited on — it runs detached.
/// Zombie reaping is handled by the OS when the parent process exits.
fn run_hook(cmd: &str, env_vars: &[(&str, &str)]) {
    use std::process::Stdio;
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env_vars {
        command.env(k, v);
    }
    if let Err(e) = command.spawn() {
        warn!("hook spawn failed (cmd={:?}): {}", cmd, e);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clido_core::{
        AgentConfig, ContentBlock, ModelResponse, Message, PermissionMode, Role, StopReason,
        ToolSchema, Usage,
    };
    use clido_providers::ModelProvider;
    use clido_storage::SessionLine;
    use clido_tools::ToolRegistry;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    struct DenyAskUser;

    #[async_trait]
    impl AskUser for DenyAskUser {
        async fn ask(&self, _req: PermRequest) -> PermGrant {
            PermGrant::Deny
        }
    }

    /// Minimal mock provider that always returns a fixed text response.
    struct MockProvider {
        response_text: String,
    }

    impl MockProvider {
        fn new(text: &str) -> Self {
            Self {
                response_text: text.to_string(),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            Ok(ModelResponse {
                id: "mock-id".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlock::Text {
                    text: self.response_text.clone(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            })
        }

        async fn complete_stream(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }
        async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
            vec![]
        }
    }

    /// First `complete` returns rate-limited; later calls succeed (simulates retry / resume).
    struct RateLimitedThenOkProvider {
        calls: AtomicU32,
    }

    impl RateLimitedThenOkProvider {
        fn new() -> Self {
            Self {
                calls: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for RateLimitedThenOkProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(ClidoError::RateLimited {
                    message: "too many requests".into(),
                    retry_after_secs: Some(1),
                    is_subscription_limit: false,
                });
            }
            Ok(ModelResponse {
                id: "mock-id".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlock::Text {
                    text: "after rate limit".to_string(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            })
        }

        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }

        async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
            vec![]
        }
    }

    fn mock_config() -> AgentConfig {
        AgentConfig {
            model: "mock".to_string(),
            system_prompt: None,
            max_turns: 3,
            max_budget_usd: None,
            permission_mode: PermissionMode::AcceptAll,
            permission_rules: vec![],
            max_context_tokens: None,
            compaction_threshold: None,
            quiet: false,
            max_parallel_tools: 1,
            use_planner: false,
            use_index: false,
            no_rules: false,
            rules_file: None,
            max_output_tokens: None,
        }
    }

    fn empty_registry() -> ToolRegistry {
        clido_tools::default_registry_with_blocked(std::env::temp_dir(), vec![])
    }

    // ── session_lines_to_messages ──────────────────────────────────────────

    #[test]
    fn session_lines_empty_returns_empty() {
        let msgs = session_lines_to_messages(&[]);
        assert!(msgs.is_empty());
    }

    #[test]
    fn session_lines_user_message_converted() {
        let lines = vec![SessionLine::UserMessage {
            role: "user".to_string(),
            content: vec![serde_json::json!({"type": "text", "text": "hello"})],
        }];
        let msgs = session_lines_to_messages(&lines);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
    }

    #[test]
    fn session_lines_assistant_message_converted() {
        let lines = vec![SessionLine::AssistantMessage {
            content: vec![serde_json::json!({"type": "text", "text": "hi back"})],
        }];
        let msgs = session_lines_to_messages(&lines);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
    }

    #[test]
    fn session_lines_tool_results_grouped_into_user_message() {
        let lines = vec![
            SessionLine::AssistantMessage {
                content: vec![serde_json::json!({"type": "text", "text": "thinking"})],
            },
            SessionLine::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "result text".to_string(),
                is_error: false,
                duration_ms: None,
                path: None,
                content_hash: None,
                mtime_nanos: None,
            },
        ];
        let msgs = session_lines_to_messages(&lines);
        // Should have: assistant message + user message (tool result)
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[1].role, Role::User);
        assert!(matches!(
            msgs[1].content[0],
            ContentBlock::ToolResult { .. }
        ));
    }

    #[test]
    fn session_lines_tool_call_is_skipped() {
        let lines = vec![SessionLine::ToolCall {
            tool_use_id: "call-1".to_string(),
            tool_name: "Read".to_string(),
            input: serde_json::json!({}),
        }];
        let msgs = session_lines_to_messages(&lines);
        assert!(msgs.is_empty());
    }

    // ── AgentLoop builder methods ──────────────────────────────────────────

    #[test]
    fn agent_loop_new_defaults() {
        let provider = Arc::new(MockProvider::new("ok"));
        let agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        assert_eq!(agent.current_model(), "mock");
        assert_eq!(agent.turn_count(), 0);
        assert_eq!(agent.cumulative_cost_usd, 0.0);
        assert_eq!(agent.cumulative_input_tokens, 0);
        assert_eq!(agent.cumulative_output_tokens, 0);
        assert!(!agent.planner_mode);
    }

    #[test]
    fn agent_loop_with_planner_sets_flag() {
        let provider = Arc::new(MockProvider::new("ok"));
        let agent =
            AgentLoop::new(provider, empty_registry(), mock_config(), None).with_planner(true);
        assert!(agent.planner_mode);
    }

    #[test]
    fn agent_loop_with_planner_false_clears_flag() {
        let provider = Arc::new(MockProvider::new("ok"));
        let agent =
            AgentLoop::new(provider, empty_registry(), mock_config(), None).with_planner(false);
        assert!(!agent.planner_mode);
    }

    #[test]
    fn agent_loop_new_with_history_sets_history() {
        let history = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        }];
        let provider = Arc::new(MockProvider::new("ok"));
        let agent =
            AgentLoop::new_with_history(provider, empty_registry(), mock_config(), history, None);
        assert_eq!(agent.history.len(), 1);
    }

    #[test]
    fn agent_loop_set_model() {
        let provider = Arc::new(MockProvider::new("ok"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        assert_eq!(agent.current_model(), "mock");
        agent.set_model("claude-sonnet-4-5".to_string());
        assert_eq!(agent.current_model(), "claude-sonnet-4-5");
    }

    #[test]
    fn agent_loop_replace_history_resets_counters() {
        let provider = Arc::new(MockProvider::new("ok"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        agent.cumulative_cost_usd = 5.0;
        agent.cumulative_input_tokens = 100;
        agent.cumulative_output_tokens = 50;
        agent.replace_history(vec![]);
        assert_eq!(agent.cumulative_cost_usd, 0.0);
        assert_eq!(agent.cumulative_input_tokens, 0);
        assert_eq!(agent.cumulative_output_tokens, 0);
        assert_eq!(agent.history.len(), 0);
    }

    // ── compact_history_now ────────────────────────────────────────────────

    #[tokio::test]
    async fn compact_history_now_returns_before_after_counts() {
        let provider = Arc::new(MockProvider::new("ok"));
        let history = (0..5)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: "x".repeat(100),
                }],
            })
            .collect();
        let mut agent =
            AgentLoop::new_with_history(provider, empty_registry(), mock_config(), history, None);
        let (before, _after) = agent.compact_history_now().await.unwrap();
        assert_eq!(before, 5);
        // After can differ from before (compaction adds a placeholder or drops messages)
        // Just check the call succeeded and we got some counts back.
        assert!(agent.history.len() <= before + 1); // +1 for possible compacted placeholder
    }

    // ── run() with mock provider ───────────────────────────────────────────

    #[tokio::test]
    async fn agent_run_returns_response_text() {
        let provider = Arc::new(MockProvider::new("test response"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let result = agent.run("say hello", None, None, None).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result.err());
        let text = result.unwrap();
        assert_eq!(text, "test response");
    }

    #[tokio::test]
    async fn agent_run_increments_token_counters() {
        let provider = Arc::new(MockProvider::new("hello"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        agent.run("prompt", None, None, None).await.unwrap();
        assert!(agent.cumulative_input_tokens > 0);
        assert!(agent.cumulative_output_tokens > 0);
    }

    #[tokio::test]
    async fn complete_simple_returns_text() {
        let provider = Arc::new(MockProvider::new("simple response"));
        let agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let result = agent.complete_simple("what is 2+2?").await.unwrap();
        assert_eq!(result, "simple response");
    }

    // ── fast provider fallback chain ───────────────────────────────────────

    #[tokio::test]
    async fn complete_simple_fast_uses_fast_provider() {
        let main = Arc::new(MockProvider::new("main response"));
        let fast = Arc::new(MockProvider::new("fast response"));
        let mut fast_cfg = mock_config();
        fast_cfg.model = "fast-model".to_string();
        let agent = AgentLoop::new(main, empty_registry(), mock_config(), None)
            .with_fast_provider(Some(fast), Some(fast_cfg));
        let result = agent.complete_simple_fast("test").await.unwrap();
        assert_eq!(result, "fast response");
    }

    #[tokio::test]
    async fn complete_simple_fast_falls_back_to_main() {
        let main = Arc::new(MockProvider::new("main response"));
        let agent = AgentLoop::new(main, empty_registry(), mock_config(), None);
        // No fast provider configured — should use main.
        let result = agent.complete_simple_fast("test").await.unwrap();
        assert_eq!(result, "main response");
    }

    #[tokio::test]
    async fn complete_simple_fast_with_usage_returns_tokens() {
        let fast = Arc::new(MockProvider::new("fast"));
        let main = Arc::new(MockProvider::new("main"));
        let agent = AgentLoop::new(main, empty_registry(), mock_config(), None)
            .with_fast_provider(Some(fast), Some(mock_config()));
        let (text, usage) = agent
            .complete_simple_fast_with_usage("prompt")
            .await
            .unwrap();
        assert_eq!(text, "fast");
        assert!(usage.input_tokens > 0);
        assert!(usage.output_tokens > 0);
    }

    #[tokio::test]
    async fn complete_with_system_fast_uses_fast_provider() {
        let main = Arc::new(MockProvider::new("main"));
        let fast = Arc::new(MockProvider::new("enhanced prompt"));
        let agent = AgentLoop::new(main, empty_registry(), mock_config(), None)
            .with_fast_provider(Some(fast), Some(mock_config()));
        let result = agent
            .complete_with_system_fast("You are a prompt enhancer", "make this better")
            .await
            .unwrap();
        assert_eq!(result, "enhanced prompt");
    }

    #[test]
    fn with_fast_provider_none_leaves_fallback() {
        let main = Arc::new(MockProvider::new("main"));
        let agent = AgentLoop::new(main.clone(), empty_registry(), mock_config(), None)
            .with_fast_provider(None, None);
        // fast_provider is None — utility_provider will return main.
        // We can't call utility_provider directly (private), but complete_simple_fast
        // exercises the same path.  Just verify construction succeeds.
        assert_eq!(agent.current_model(), "mock");
    }

    // ── run_next_turn ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_next_turn_appends_to_history() {
        let provider = Arc::new(MockProvider::new("second response"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        // First run
        agent.run("first", None, None, None).await.unwrap();
        let len_after_first = agent.history.len();
        // Second turn
        let result = agent
            .run_next_turn("second", None, None, None)
            .await
            .unwrap();
        assert_eq!(result, "second response");
        // History grew
        assert!(agent.history.len() > len_after_first);
    }

    // ── run_with_extra_blocks ──────────────────────────────────────────────

    #[tokio::test]
    async fn run_with_extra_blocks_includes_extra_content() {
        let provider = Arc::new(MockProvider::new("handled image"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let extra = vec![ContentBlock::Text {
            text: "[image placeholder]".to_string(),
        }];
        let result = agent
            .run_with_extra_blocks("describe image", extra, None, None, None)
            .await
            .unwrap();
        assert_eq!(result, "handled image");
        // User message in history should have 2 content blocks
        let first_user = agent.history.iter().find(|m| m.role == Role::User).unwrap();
        assert_eq!(first_user.content.len(), 2);
    }

    #[tokio::test]
    async fn run_next_turn_with_extra_blocks_works() {
        let provider = Arc::new(MockProvider::new("extra blocks response"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        // Initial run
        agent.run("start", None, None, None).await.unwrap();
        let extra = vec![ContentBlock::Text {
            text: "[image]".to_string(),
        }];
        let result = agent
            .run_next_turn_with_extra_blocks("follow up", extra, None, None, None)
            .await
            .unwrap();
        assert_eq!(result, "extra blocks response");
    }

    // ── history rollback on failed run ─────────────────────────────────────

    #[tokio::test]
    async fn run_next_turn_preserves_history_when_rate_limited() {
        let provider = Arc::new(RateLimitedThenOkProvider::new());
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        agent.run("first", None, None, None).await.unwrap();
        let len_after_first = agent.history.len();

        let r = agent.run_next_turn("second", None, None, None).await;
        assert!(
            matches!(r, Err(ClidoError::RateLimited { .. })),
            "expected RateLimited, got {r:?}"
        );
        assert_eq!(
            agent.history.len(),
            len_after_first + 1,
            "user line for the rate-limited turn must stay in history for resume"
        );

        let r2 = agent.run_next_turn("continue", None, None, None).await.unwrap();
        assert_eq!(r2, "after rate limit");
    }

    /// Provider that always fails to simulate a network/API error.
    struct FailingProvider;

    #[async_trait]
    impl ModelProvider for FailingProvider {
        async fn complete(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            Err(ClidoError::Other(anyhow::anyhow!("simulated API error")))
        }

        async fn complete_stream(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }
        async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
            vec![]
        }
    }

    #[tokio::test]
    async fn run_rolls_back_user_message_on_provider_failure() {
        let provider = Arc::new(FailingProvider);
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        assert_eq!(agent.history.len(), 0);
        let result = agent.run("hello", None, None, None).await;
        assert!(result.is_err());
        // History must be empty — the dangling user message should be rolled back.
        assert_eq!(
            agent.history.len(),
            0,
            "failed run should roll back the user message from history"
        );
    }

    #[tokio::test]
    async fn run_next_turn_rolls_back_user_message_on_provider_failure() {
        // First turn succeeds with a working provider.
        let good_provider = Arc::new(MockProvider::new("first response"));
        let mut agent = AgentLoop::new(good_provider, empty_registry(), mock_config(), None);
        agent.run("first", None, None, None).await.unwrap();
        let len_after_first = agent.history.len();

        // Swap the provider to a failing one, then try a second turn.
        agent.provider = Arc::new(FailingProvider);
        let result = agent.run_next_turn("second", None, None, None).await;
        assert!(result.is_err());
        // History must not have grown — the dangling user message is rolled back.
        assert_eq!(
            agent.history.len(),
            len_after_first,
            "failed run_next_turn should roll back the user message"
        );
    }

    // ── cancel signal ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_returns_interrupted_when_cancel_already_set() {
        let provider = Arc::new(MockProvider::new("response"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let cancel = Arc::new(AtomicBool::new(true)); // pre-cancelled
        let result = agent.run("hi", None, None, Some(cancel)).await;
        assert!(matches!(result, Err(ClidoError::Interrupted)));
    }

    // ── max_turns ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_returns_max_turns_exceeded_when_config_is_zero() {
        let provider = Arc::new(MockProvider::new("response"));
        let mut cfg = mock_config();
        cfg.max_turns = 0;
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, None);
        let result = agent.run("hi", None, None, None).await;
        assert!(matches!(result, Err(ClidoError::MaxTurnsExceeded)));
    }

    // ── budget exceeded ────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_returns_budget_exceeded_when_limit_is_zero() {
        let provider = Arc::new(MockProvider::new("response"));
        let mut cfg = mock_config();
        cfg.max_budget_usd = Some(0.0); // any non-zero cost exceeds this
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, None);
        let result = agent.run("hi", None, None, None).await;
        assert!(matches!(result, Err(ClidoError::BudgetExceeded)));
    }

    // ── run_continue ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_continue_returns_response_when_history_already_set() {
        let provider = Arc::new(MockProvider::new("continued"));
        let history = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "prior message".to_string(),
            }],
        }];
        let mut agent =
            AgentLoop::new_with_history(provider, empty_registry(), mock_config(), history, None);
        let result = agent.run_continue(None, None, None).await.unwrap();
        assert_eq!(result, "continued");
    }

    #[tokio::test]
    async fn run_continue_with_cancel() {
        let provider = Arc::new(MockProvider::new("response"));
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let cancel = Arc::new(AtomicBool::new(true));
        let result = agent.run_continue(None, None, Some(cancel)).await;
        assert!(matches!(result, Err(ClidoError::Interrupted)));
    }

    // ── with_emitter / with_hooks / with_memory ──────────────────────────

    #[test]
    fn agent_loop_builder_methods_compile_and_chain() {
        let provider = Arc::new(MockProvider::new("ok"));
        let agent =
            AgentLoop::new(provider, empty_registry(), mock_config(), None).with_planner(true);
        assert!(agent.planner_mode);
    }

    // ── session_lines ToolResult flushed at end ────────────────────────────

    #[test]
    fn session_lines_trailing_tool_results_flushed() {
        // Two tool results at end of lines (no following user/assistant message)
        let lines = vec![
            SessionLine::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "r1".to_string(),
                is_error: false,
                duration_ms: None,
                path: None,
                content_hash: None,
                mtime_nanos: None,
            },
            SessionLine::ToolResult {
                tool_use_id: "t2".to_string(),
                content: "r2".to_string(),
                is_error: true,
                duration_ms: None,
                path: None,
                content_hash: None,
                mtime_nanos: None,
            },
        ];
        let msgs = session_lines_to_messages(&lines);
        // Both tool results should be in a single user message
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[0].content.len(), 2);
    }

    // ── compute_diff_for_tool (pure helper) ───────────────────────────────

    #[tokio::test]
    async fn compute_diff_returns_none_for_unknown_tool() {
        let (diff, content, path) =
            compute_diff_for_tool("UnknownTool", &serde_json::json!({})).await;
        assert!(diff.is_none());
        assert!(content.is_none());
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn compute_diff_returns_none_for_write_with_missing_path() {
        let input = serde_json::json!({ "content": "hello" }); // no file_path
        let (diff, content, path) = compute_diff_for_tool("Write", &input).await;
        assert!(diff.is_none());
        assert!(content.is_none());
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn compute_diff_returns_none_for_write_with_missing_content() {
        let input = serde_json::json!({ "file_path": "/tmp/clido_test_missing.txt" }); // no content
        let (diff, content, path) = compute_diff_for_tool("Write", &input).await;
        assert!(diff.is_none());
        assert!(content.is_none());
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn compute_diff_returns_diff_for_write_with_new_file() {
        use std::fs;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), "old content\n").unwrap();
        let path_str = tmp.path().to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "content": "new content\n"
        });
        let (diff, content, file_path) = compute_diff_for_tool("Write", &input).await;
        assert!(diff.is_some()); // should have a diff
        assert!(diff.unwrap().contains('+'));
        assert_eq!(content.unwrap(), "new content\n");
        assert_eq!(file_path.unwrap().to_str().unwrap(), path_str);
    }

    #[tokio::test]
    async fn compute_diff_returns_none_diff_when_content_unchanged() {
        use std::fs;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), "same\n").unwrap();
        let path_str = tmp.path().to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "content": "same\n"
        });
        let (diff, _, _) = compute_diff_for_tool("Write", &input).await;
        assert!(diff.is_none()); // no diff when content is identical
    }

    #[tokio::test]
    async fn compute_diff_uses_edit_tool_keys() {
        use std::fs;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), "old\n").unwrap();
        let path_str = tmp.path().to_str().unwrap().to_string();
        let input = serde_json::json!({
            "file_path": path_str,
            "old_string": "old\n",
            "new_string": "new\n"
        });
        let (diff, content, _) = compute_diff_for_tool("Edit", &input).await;
        assert!(diff.is_some());
        // proposed_content is the full file after applying the edit
        assert_eq!(content.unwrap(), "new\n");
    }

    // ── summarize_messages (pure helper) ─────────────────────────────────

    #[tokio::test]
    async fn summarize_messages_calls_provider_and_returns_text() {
        let provider = MockProvider::new("Summary of conversation.");
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "hi there".to_string(),
                }],
            },
        ];
        let result = context::summarize_messages(&messages, &provider, &mock_config())
            .await
            .unwrap();
        assert_eq!(result, "Summary of conversation.");
    }

    #[tokio::test]
    async fn summarize_messages_error_on_empty_response() {
        let provider = MockProvider::new(""); // empty response
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        }];
        let result = context::summarize_messages(&messages, &provider, &mock_config()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn summarize_messages_handles_tool_use_and_result_blocks() {
        let provider = MockProvider::new("Summarized tool work.");
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "/foo"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "file contents".to_string(),
                    is_error: false,
                }],
            },
        ];
        let result = context::summarize_messages(&messages, &provider, &mock_config())
            .await
            .unwrap();
        assert_eq!(result, "Summarized tool work.");
    }

    // ── compact_with_summary ──────────────────────────────────────────────

    #[tokio::test]
    async fn compact_with_summary_under_threshold_returns_as_is() {
        let provider = MockProvider::new("irrelevant");
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "short".to_string(),
            }],
        }];
        // Use a very high threshold so no compaction happens
        let result =
            context::compact_with_summary(&messages, 0, 200_000, 0.9, &provider, &mock_config())
                .await
                .unwrap();
        assert_eq!(result.len(), messages.len());
    }

    #[tokio::test]
    async fn compact_with_summary_compacts_large_history() {
        let provider = MockProvider::new("Compacted summary text.");
        // Create many messages to exceed the threshold
        let messages: Vec<Message> = (0..50)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: "x".repeat(500),
                }],
            })
            .collect();
        // Small context to force compaction
        let result = context::compact_with_summary(
            &messages,
            0,
            2000, // very small max context
            0.1,  // very low threshold to trigger compaction
            &provider,
            &mock_config(),
        )
        .await;
        // Either succeeds (compact happened) or fails with ContextLimit
        match result {
            Ok(compacted) => {
                assert!(compacted.len() < messages.len());
            }
            Err(ClidoError::ContextLimit { .. }) => {
                // Acceptable: summary + tail doesn't fit
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    // ── run() with max_tokens stop reason ─────────────────────────────────

    struct MaxTokensProvider;

    #[async_trait]
    impl ModelProvider for MaxTokensProvider {
        async fn complete(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            Ok(ModelResponse {
                id: "id".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlock::Text {
                    text: "truncated".to_string(),
                }],
                stop_reason: StopReason::MaxTokens,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            })
        }
        async fn complete_stream(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }
        async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
            vec![]
        }
    }

    #[tokio::test]
    async fn run_returns_text_on_max_tokens_stop() {
        let provider = Arc::new(MaxTokensProvider);
        let mut agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let result = agent.run("hello", None, None, None).await.unwrap();
        assert_eq!(result, "truncated");
    }

    // ── run() with tool_use that gets accepted (AcceptAll mode) ───────────

    struct ToolUseProvider {
        call_count: std::sync::atomic::AtomicU32,
    }

    impl ToolUseProvider {
        fn new() -> Self {
            Self {
                call_count: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for ToolUseProvider {
        async fn complete(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                // First call: return a tool use
                Ok(ModelResponse {
                    id: "id1".to_string(),
                    model: "mock".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call1".to_string(),
                        name: "Bash".to_string(),
                        input: serde_json::json!({"command": "echo hello"}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                    },
                })
            } else {
                // Second call: end turn
                Ok(ModelResponse {
                    id: "id2".to_string(),
                    model: "mock".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "done".to_string(),
                    }],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                    },
                })
            }
        }
        async fn complete_stream(
            &self,
            _messages: &[clido_core::Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }
        async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
            vec![]
        }
    }

    #[tokio::test]
    async fn run_with_tool_use_executes_tool_and_continues() {
        let provider = Arc::new(ToolUseProvider::new());
        let mut cfg = mock_config();
        cfg.max_turns = 5;
        cfg.permission_mode = PermissionMode::AcceptAll;
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, None);
        let result = agent.run("do something", None, None, None).await.unwrap();
        assert_eq!(result, "done");
        // Turn count should be 2
        assert_eq!(agent.turn_count(), 2);
    }

    // ── PermissionMode::PlanOnly blocks write tools ────────────────────────

    #[tokio::test]
    async fn plan_only_mode_blocks_write_and_returns_error_in_tool_result() {
        // We need a provider that first calls a write tool, then ends.
        struct WriteToolProvider {
            call_count: std::sync::atomic::AtomicU32,
        }
        impl WriteToolProvider {
            fn new() -> Self {
                Self {
                    call_count: std::sync::atomic::AtomicU32::new(0),
                }
            }
        }
        #[async_trait]
        impl ModelProvider for WriteToolProvider {
            async fn complete(
                &self,
                _messages: &[clido_core::Message],
                _tools: &[ToolSchema],
                _config: &AgentConfig,
            ) -> clido_core::Result<ModelResponse> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Ok(ModelResponse {
                        id: "id".to_string(),
                        model: "mock".to_string(),
                        content: vec![ContentBlock::ToolUse {
                            id: "c1".to_string(),
                            name: "Write".to_string(),
                            input: serde_json::json!({"file_path": "/tmp/test.txt", "content": "hi"}),
                        }],
                        stop_reason: StopReason::ToolUse,
                        usage: Usage {
                            input_tokens: 1,
                            output_tokens: 1,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                        },
                    })
                } else {
                    Ok(ModelResponse {
                        id: "id2".to_string(),
                        model: "mock".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "blocked".to_string(),
                        }],
                        stop_reason: StopReason::EndTurn,
                        usage: Usage {
                            input_tokens: 1,
                            output_tokens: 1,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                        },
                    })
                }
            }
            async fn complete_stream(
                &self,
                _: &[clido_core::Message],
                _: &[ToolSchema],
                _: &AgentConfig,
            ) -> clido_core::Result<
                Pin<
                    Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>,
                >,
            > {
                unimplemented!()
            }
            async fn list_models(&self) -> Vec<clido_providers::ModelEntry> {
                vec![]
            }
        }

        let provider = Arc::new(WriteToolProvider::new());
        let mut cfg = mock_config();
        cfg.max_turns = 5;
        cfg.permission_mode = PermissionMode::PlanOnly;
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, None);
        let result = agent.run("write a file", None, None, None).await.unwrap();
        // Should complete successfully (blocked tool returned error message to model)
        assert_eq!(result, "blocked");
        // History should include the tool result (error)
        let has_tool_result = agent.history.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }))
        });
        assert!(has_tool_result);
    }

    // ── run_hook ────────────────────────────────────────────────────────────

    #[test]
    fn run_hook_executes_without_panic() {
        // Just test that run_hook doesn't panic (fire-and-forget)
        run_hook("true", &[("MY_VAR", "hello")]);
        run_hook("echo $CLIDO_TOOL_NAME", &[("CLIDO_TOOL_NAME", "Read")]);
    }

    // ── with_hooks integration ─────────────────────────────────────────────

    #[tokio::test]
    async fn run_with_hooks_config_executes_successfully() {
        use clido_core::HooksConfig;
        let provider = Arc::new(MockProvider::new("hooked response"));
        let cfg = mock_config();
        let hooks = HooksConfig {
            pre_tool_use: Some("true".to_string()),
            post_tool_use: Some("true".to_string()),
        };
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, None).with_hooks(hooks);
        let result = agent.run("hello", None, None, None).await.unwrap();
        assert_eq!(result, "hooked response");
    }

    #[test]
    fn write_audit_redacts_secret_tokens_in_input_summary() {
        let provider = Arc::new(MockProvider::new("ok"));
        let reg = empty_registry();
        let cfg = mock_config();
        let project = tempfile::tempdir().unwrap();
        let audit = clido_storage::AuditLog::open(project.path()).unwrap();
        let audit = Arc::new(std::sync::Mutex::new(audit));
        let loop_ = AgentLoop::new(provider, reg, cfg, None).with_audit_log(audit);

        loop_.write_audit(
            "Bash",
            &serde_json::json!({"command":"echo sk-or-v1-verysecretkey"}),
            &clido_tools::ToolOutput::ok("ok".to_string()),
            1,
        );

        let audit_path = clido_storage::audit_log_path(project.path()).unwrap();
        let content = std::fs::read_to_string(audit_path).unwrap();
        assert!(
            !content.contains("verysecretkey"),
            "audit input summary should not contain raw secret values"
        );
        assert!(
            content.contains("[REDACTED]"),
            "audit input summary should include redaction marker"
        );
    }

    // ── session_lines_to_messages edge cases ──────────────────────────────

    #[test]
    fn session_lines_unknown_variant_is_skipped() {
        // SessionLine::Result (a synthetic line type) should just be skipped
        let lines = vec![
            SessionLine::UserMessage {
                role: "user".to_string(),
                content: vec![serde_json::json!({"type": "text", "text": "hi"})],
            },
            // Add a ToolCall (which is also skipped)
            SessionLine::ToolCall {
                tool_use_id: "x".to_string(),
                tool_name: "Read".to_string(),
                input: serde_json::json!({}),
            },
        ];
        let msgs = session_lines_to_messages(&lines);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
    }

    #[test]
    fn session_lines_user_then_tool_result_flushes_correctly() {
        let lines = vec![
            SessionLine::UserMessage {
                role: "user".to_string(),
                content: vec![serde_json::json!({"type": "text", "text": "hello"})],
            },
            SessionLine::AssistantMessage {
                content: vec![
                    serde_json::json!({"type": "tool_use", "id": "t1", "name": "Read", "input": {}}),
                ],
            },
            SessionLine::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "file data".to_string(),
                is_error: false,
                duration_ms: Some(100),
                path: None,
                content_hash: None,
                mtime_nanos: None,
            },
            SessionLine::UserMessage {
                role: "user".to_string(),
                content: vec![serde_json::json!({"type": "text", "text": "next"})],
            },
        ];
        let msgs = session_lines_to_messages(&lines);
        // user, assistant, user(tool_result), user
        assert_eq!(msgs.len(), 4);
    }

    // ── inject_memories returns None when no memory ───────────────────────

    #[test]
    fn inject_memories_returns_none_when_no_memory_store() {
        let provider = Arc::new(MockProvider::new("ok"));
        let agent = AgentLoop::new(provider, empty_registry(), mock_config(), None);
        let result = agent.inject_memories("some prompt");
        assert!(result.is_none());
    }

    // ── with_emitter (no panic) ────────────────────────────────────────────

    #[tokio::test]
    async fn run_with_emitter_does_not_panic() {
        use std::sync::Mutex as StdMutex;

        struct RecordingEmitter {
            starts: StdMutex<Vec<String>>,
            dones: StdMutex<Vec<String>>,
        }

        #[async_trait]
        impl EventEmitter for RecordingEmitter {
            async fn on_tool_start(
                &self,
                _tool_use_id: &str,
                name: &str,
                _input: &serde_json::Value,
            ) {
                self.starts.lock().unwrap().push(name.to_string());
            }
            async fn on_tool_done(
                &self,
                _tool_use_id: &str,
                name: &str,
                _is_error: bool,
                _diff: Option<String>,
            ) {
                self.dones.lock().unwrap().push(name.to_string());
            }
        }

        let emitter = Arc::new(RecordingEmitter {
            starts: StdMutex::new(vec![]),
            dones: StdMutex::new(vec![]),
        });
        let provider = Arc::new(MockProvider::new("emitted response"));
        let mut agent =
            AgentLoop::new(provider, empty_registry(), mock_config(), None).with_emitter(emitter);
        let result = agent.run("hello", None, None, None).await.unwrap();
        assert_eq!(result, "emitted response");
    }

    #[tokio::test]
    async fn execute_tool_maybe_gated_denial_mentions_tool_name() {
        let provider = Arc::new(MockProvider::new("ok"));
        let mut cfg = mock_config();
        cfg.permission_mode = PermissionMode::Default;
        let ask_user: Arc<dyn AskUser> = Arc::new(DenyAskUser);
        let mut agent = AgentLoop::new(provider, empty_registry(), cfg, Some(ask_user));

        let out = agent
            .execute_tool_maybe_gated(
                "Write",
                &serde_json::json!({"path":"deny_test.txt","content":"x"}),
            )
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("Write"),
            "error should include tool name"
        );
    }
}
