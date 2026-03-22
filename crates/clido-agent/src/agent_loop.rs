//! Minimal agent loop: history, provider call, tool execution, repeat.

use async_trait::async_trait;
use clido_context::{
    assemble, dedup_file_reads, estimate_tokens_message, estimate_tokens_messages,
    estimate_tokens_str, DEFAULT_COMPACTION_THRESHOLD, DEFAULT_MAX_CONTEXT_TOKENS,
};
use clido_core::{
    compute_cost_usd, AgentConfig, ContentBlock, HooksConfig, Message, PermissionMode, Role,
    StopReason,
};
use clido_core::{ClidoError, PricingTable, Result};
use clido_memory::MemoryStore;
use clido_providers::ModelProvider;
use clido_storage::{AuditEntry, AuditLog, SessionLine, SessionWriter};
use clido_tools::{ToolOutput, ToolRegistry};
use futures::future::join_all;
use similar::TextDiff;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tracing::debug;

/// The result of a permission request — what the user decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermGrant {
    /// Allow this single invocation.
    Allow,
    /// Deny this invocation.
    Deny,
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
    async fn on_tool_start(&self, name: &str, input: &serde_json::Value);
    /// Called after a tool completes. `diff` is set for Edit operations.
    async fn on_tool_done(&self, name: &str, is_error: bool, diff: Option<String>);
    /// Called for any text the model emits while it's still calling tools (thinking aloud).
    /// Default impl is a no-op so existing code compiles without changes.
    async fn on_assistant_text(&self, _text: &str) {}
}

/// Reconstruct conversation history from session JSONL lines (for resume).
pub fn session_lines_to_messages(lines: &[SessionLine]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut tool_result_buf: Vec<ContentBlock> = Vec::new();

    let flush_tool_results = |msgs: &mut Vec<Message>, buf: &mut Vec<ContentBlock>| {
        if !buf.is_empty() {
            msgs.push(Message {
                role: Role::User,
                content: std::mem::take(buf),
            });
        }
    };

    for line in lines {
        match line {
            SessionLine::UserMessage { content, .. } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let content: Vec<ContentBlock> = content
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                messages.push(Message {
                    role: Role::User,
                    content,
                });
            }
            SessionLine::AssistantMessage { content } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let content: Vec<ContentBlock> = content
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                messages.push(Message {
                    role: Role::Assistant,
                    content,
                });
            }
            SessionLine::ToolCall { .. } => {}
            SessionLine::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                tool_result_buf.push(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                });
            }
            _ => {}
        }
    }
    flush_tool_results(&mut messages, &mut tool_result_buf);
    messages
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
    /// Optional audit log for recording tool calls.
    audit_log: Option<Arc<std::sync::Mutex<AuditLog>>>,
    /// Optional hooks config for pre/post tool use.
    hooks: Option<HooksConfig>,
    /// Optional long-term memory store for context injection.
    memory: Option<Arc<Mutex<MemoryStore>>>,
    /// When true, the agent will emit a planning step on the first turn (--planner flag).
    /// The plan is purely informational: the reactive loop still drives execution.
    pub planner_mode: bool,
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        tools: ToolRegistry,
        config: AgentConfig,
        ask_user: Option<Arc<dyn AskUser>>,
    ) -> Self {
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
            audit_log: None,
            hooks: None,
            memory: None,
            planner_mode: false,
        }
    }

    /// Enable or disable planner mode (--planner CLI flag).
    pub fn with_planner(mut self, enabled: bool) -> Self {
        self.planner_mode = enabled;
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
            audit_log: None,
            hooks: None,
            memory: None,
            planner_mode: false,
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

    /// Return the model currently active for this session.
    pub fn current_model(&self) -> &str {
        &self.config.model
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
        let base = self
            .config
            .system_prompt
            .as_deref()
            .unwrap_or("You are a helpful coding assistant.");
        Some(format!("{}\n\n[Relevant memories]\n{}", base, memory_text))
    }

    /// Turn count from last run (for session result line).
    pub fn turn_count(&self) -> u32 {
        self.last_turn_count
    }

    /// Replace the current conversation history (for session resume).
    pub fn replace_history(&mut self, history: Vec<clido_core::Message>) {
        self.history = history;
        self.last_turn_count = 0;
        self.cumulative_cost_usd = 0.0;
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
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
        let compacted = compact_with_summary(
            &self.history,
            sys_tokens,
            max_ctx,
            0.0,
            self.provider.as_ref(),
            &self.config,
        )
        .await?;
        let after = compacted.len();
        self.history = compacted;
        Ok((before, after))
    }

    /// Make a single LLM completion call with no tools — used for planning.
    /// Returns the first text block from the response, or an error.
    pub async fn complete_simple(&self, prompt: &str) -> clido_core::Result<String> {
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
        Ok(text)
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
            let to_send = compact_with_summary(
                &self.history,
                system_tokens,
                max_ctx,
                threshold,
                self.provider.as_ref(),
                &self.config,
            )
            .await?;

            let response = self
                .provider
                .complete(&to_send, &schemas, &self.config)
                .await?;

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

            if let Some(limit) = self.config.max_budget_usd {
                if self.cumulative_cost_usd > limit {
                    return Err(ClidoError::BudgetExceeded);
                }
            }

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
                let _ = w.write_line(&SessionLine::AssistantMessage { content });
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
                            let _ = w.write_line(&SessionLine::ToolCall {
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
                            for (_, name, input) in &tool_uses {
                                e.on_tool_start(name, input).await;
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
                        let results = self.execute_tool_batch(&tool_uses).await;
                        let batch_ms = t0.elapsed().as_millis() as u64;
                        if let Some(ref e) = self.emit {
                            for ((_, name, _), output) in tool_uses.iter().zip(results.iter()) {
                                e.on_tool_done(name, output.is_error, output.diff.clone())
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
                        for (_, name, input) in &tool_uses {
                            if let Some(ref e) = self.emit {
                                e.on_tool_start(name, input).await;
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
                            let output = self.execute_tool_maybe_gated(name, input).await;
                            let duration_ms = t0.elapsed().as_millis() as u64;
                            if let Some(ref e) = self.emit {
                                e.on_tool_done(name, output.is_error, output.diff.clone())
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
                    for ((id, _, _), (output, duration_ms)) in tool_uses.iter().zip(outputs.iter())
                    {
                        if let Some(ref mut w) = session {
                            let _ = w.write_line(&SessionLine::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content.clone(),
                                is_error: output.is_error,
                                duration_ms: Some(*duration_ms),
                                path: output.path.clone(),
                                content_hash: output.content_hash.clone(),
                                mtime_nanos: output.mtime_nanos,
                            });
                        }
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                        });
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
            let _ = w.write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content,
            });
        }

        self.run_completion_loop(session, pricing, cancel).await
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
            let _ = w.write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content,
            });
        }

        self.run_completion_loop(session, pricing, cancel).await
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
            let _ = w.write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content: content_json,
            });
        }

        self.run_completion_loop(session, pricing, cancel).await
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
            let _ = w.write_line(&SessionLine::UserMessage {
                role: "user".to_string(),
                content: content_json,
            });
        }

        self.run_completion_loop(session, pricing, cancel).await
    }

    async fn run_completion_loop(
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
            let to_send = compact_with_summary(
                &self.history,
                system_tokens,
                max_ctx,
                threshold,
                self.provider.as_ref(),
                &self.config,
            )
            .await?;

            let response = self
                .provider
                .complete(&to_send, &schemas, &self.config)
                .await?;

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

            if let Some(limit) = self.config.max_budget_usd {
                if self.cumulative_cost_usd > limit {
                    return Err(ClidoError::BudgetExceeded);
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
                let _ = w.write_line(&SessionLine::AssistantMessage { content });
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
                            let _ = w.write_line(&SessionLine::ToolCall {
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
                            for (_, name, input) in &tool_uses {
                                e.on_tool_start(name, input).await;
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
                        let results = self.execute_tool_batch(&tool_uses).await;
                        let batch_ms = t0.elapsed().as_millis() as u64;
                        if let Some(ref e) = self.emit {
                            for ((_, name, _), output) in tool_uses.iter().zip(results.iter()) {
                                e.on_tool_done(name, output.is_error, output.diff.clone())
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
                        for (_, name, input) in &tool_uses {
                            if let Some(ref e) = self.emit {
                                e.on_tool_start(name, input).await;
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
                            let output = self.execute_tool_maybe_gated(name, input).await;
                            let duration_ms = t0.elapsed().as_millis() as u64;
                            if let Some(ref e) = self.emit {
                                e.on_tool_done(name, output.is_error, output.diff.clone())
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
                    for ((id, _, _), (output, duration_ms)) in tool_uses.iter().zip(outputs.iter())
                    {
                        if let Some(ref mut w) = session {
                            let _ = w.write_line(&SessionLine::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content.clone(),
                                is_error: output.is_error,
                                duration_ms: Some(*duration_ms),
                                path: output.path.clone(),
                                content_hash: output.content_hash.clone(),
                                mtime_nanos: output.mtime_nanos,
                            });
                        }
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                        });
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
                            PermGrant::Allow | PermGrant::AllowAll => {}
                            PermGrant::Deny | PermGrant::EditInEditor => {
                                return ToolOutput::err("User denied the tool call.".to_string());
                            }
                        }
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
                            PermGrant::AllowAll => {}
                            PermGrant::Deny => {
                                return ToolOutput::ok("Write rejected by user.".to_string());
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

    async fn execute_tool(&self, name: &str, input: &serde_json::Value) -> ToolOutput {
        match self.tools.get(name) {
            Some(tool) => tool.execute(input.clone()).await,
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
            let entry = AuditEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: String::new(),
                tool_name: tool_name.to_string(),
                input_summary: serde_json::to_string(tool_input)
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect(),
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
                        let _permit = sem.acquire().await.expect("semaphore closed");
                        match tools.get(&name) {
                            Some(tool) => tool.execute(input).await,
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
    let (path_key, content_key) = match tool_name {
        "Write" | "write" => ("file_path", "content"),
        "Edit" | "edit" => ("file_path", "new_string"),
        _ => return (None, None, None),
    };

    let path_str = match input.get(path_key).and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return (None, None, None),
    };
    let proposed = match input.get(content_key).and_then(|v| v.as_str()) {
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

// ── Context compaction with LLM summarization ─────────────────────────────────

/// Drop-in async replacement for `assemble()` that uses the provider to produce
/// a meaningful summary of the dropped history instead of a static placeholder.
///
/// Falls back to the static-placeholder path (identical to `assemble()`) if the
/// summarization call fails for any reason, so the agent loop is never blocked.
async fn compact_with_summary(
    messages: &[Message],
    system_prompt_tokens: u32,
    max_context_tokens: u32,
    compaction_threshold: f64,
    provider: &dyn ModelProvider,
    config: &AgentConfig,
) -> Result<Vec<Message>> {
    // Deduplicate repeated file reads before counting tokens.
    let deduped = dedup_file_reads(messages);
    let msgs = deduped.as_slice();

    let threshold_limit = ((max_context_tokens as f64) * compaction_threshold) as u32;
    let total = system_prompt_tokens + estimate_tokens_messages(msgs);

    // Under threshold — nothing to do.
    if total <= threshold_limit {
        return Ok(msgs.to_vec());
    }

    // Find the split point: keep the tail that fits within max_context_tokens.
    // Reserve 512 tokens for the summary message.
    const SUMMARY_RESERVE: u32 = 512;
    let mut kept_tokens = 0u32;
    let mut start = msgs.len();
    for (i, m) in msgs.iter().enumerate().rev() {
        let mt = estimate_tokens_message(m);
        if kept_tokens + mt + system_prompt_tokens + SUMMARY_RESERVE > max_context_tokens {
            break;
        }
        kept_tokens += mt;
        start = i;
    }

    // Nothing to compact (entire history fits in tail) — let assemble() handle it.
    if start == 0 {
        return assemble(
            msgs,
            system_prompt_tokens,
            max_context_tokens,
            compaction_threshold,
        );
    }

    let to_compact = &msgs[..start];
    let tail = &msgs[start..];

    // Try LLM summarization; log and fall back to static text on failure.
    let summary_text = match summarize_messages(to_compact, provider, config).await {
        Ok(s) => {
            tracing::info!(
                dropped = to_compact.len(),
                kept = tail.len(),
                summary_chars = s.len(),
                "context compacted with LLM summary"
            );
            format!("[Compacted history] {s}")
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "context compaction: summarization failed, using static placeholder"
            );
            "[Compacted history] Earlier messages were omitted to fit context.".to_string()
        }
    };

    // Verify the compacted result still fits.
    let summary_tokens = estimate_tokens_str(&summary_text) + 4;
    let total_after = system_prompt_tokens + summary_tokens + kept_tokens;
    if total_after > max_context_tokens {
        return Err(ClidoError::ContextLimit {
            tokens: total_after as u64,
        });
    }

    let mut out = vec![Message {
        role: Role::System,
        content: vec![ContentBlock::Text { text: summary_text }],
    }];
    out.extend_from_slice(tail);
    Ok(out)
}

/// Format `messages` as a flat transcript and ask the provider to summarize them.
async fn summarize_messages(
    messages: &[Message],
    provider: &dyn ModelProvider,
    config: &AgentConfig,
) -> Result<String> {
    const MAX_TOOL_RESULT_CHARS: usize = 1_500;

    let mut transcript = String::new();
    for msg in messages {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    transcript.push_str(&format!("[{role_label}]: {text}\n\n"));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    transcript.push_str(&format!("[{role_label}] Tool call: {name}({input})\n\n"));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    let body = if content.len() > MAX_TOOL_RESULT_CHARS {
                        format!("{}… (truncated)", &content[..MAX_TOOL_RESULT_CHARS])
                    } else {
                        content.clone()
                    };
                    transcript.push_str(&format!("[{role_label}] {label}: {body}\n\n"));
                }
                _ => {} // skip Image / Thinking blocks
            }
        }
    }

    let prompt = format!(
        "You are a summarizer for a coding agent session.\n\
        Summarize the following conversation history in 2–4 concise paragraphs.\n\
        Preserve:\n\
        - Every file path that was read or edited (list them).\n\
        - Every tool name that was called (list them).\n\
        - The user's high-level goal and any constraints they stated.\n\
        - The current state of the task (what was done, what might be left).\n\
        \n\
        Output only the summary, no preamble.\n\
        \n\
        ---\n\n\
        {transcript}"
    );

    let request = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: prompt }],
    }];

    let response = provider.complete(&request, &[], config).await?;

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

    if text.is_empty() {
        return Err(ClidoError::Other(anyhow::anyhow!(
            "summarization returned empty response"
        )));
    }

    Ok(text)
}

/// Fire-and-forget hook execution (blocking, errors silently ignored).
fn run_hook(cmd: &str, env_vars: &[(&str, &str)]) {
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(cmd);
    for (k, v) in env_vars {
        command.env(k, v);
    }
    let _ = command.spawn();
}
