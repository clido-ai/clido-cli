//! Minimal agent loop: history, provider call, tool execution, repeat.

use async_trait::async_trait;
use clido_context::{
    assemble, estimate_tokens_str, DEFAULT_COMPACTION_THRESHOLD, DEFAULT_MAX_CONTEXT_TOKENS,
};
use clido_core::{
    compute_cost_usd, AgentConfig, ContentBlock, Message, PermissionMode, Role, StopReason,
};
use clido_core::{ClidoError, PricingTable, Result};
use clido_providers::ModelProvider;
use clido_storage::{SessionLine, SessionWriter};
use clido_tools::{ToolOutput, ToolRegistry};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::debug;

/// Callback for asking the user to approve a state-changing tool call (Default permission mode).
#[async_trait]
pub trait AskUser: Send + Sync {
    /// Return true to allow the tool call, false to deny.
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool;
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
    /// When set, overrides config.permission_mode for the rest of the session (e.g. after ExitPlanMode).
    permission_mode_override: Option<PermissionMode>,
    /// Last turn count after run() (for session recording).
    last_turn_count: u32,
    /// Cumulative cost in USD from last run (when pricing provided).
    pub cumulative_cost_usd: f64,
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
            permission_mode_override: None,
            last_turn_count: 0,
            cumulative_cost_usd: 0.0,
        }
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
            permission_mode_override: None,
            last_turn_count: 0,
            cumulative_cost_usd: 0.0,
        }
    }

    /// Turn count from last run (for session result line).
    pub fn turn_count(&self) -> u32 {
        self.last_turn_count
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
            let to_send = assemble(&self.history, system_tokens, max_ctx, threshold)?;

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
                    let mut tool_results = Vec::new();
                    for block in &response.content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            if let Some(ref mut w) = session {
                                let _ = w.write_line(&SessionLine::ToolCall {
                                    tool_use_id: id.clone(),
                                    tool_name: name.clone(),
                                    input: input.clone(),
                                });
                            }
                            let output = self.execute_tool_maybe_gated(name, input).await;
                            if let Some(ref mut w) = session {
                                let _ = w.write_line(&SessionLine::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: output.content.clone(),
                                    is_error: output.is_error,
                                    duration_ms: None,
                                    path: output.path.clone(),
                                    content_hash: output.content_hash.clone(),
                                    mtime_nanos: output.mtime_nanos,
                                });
                            }
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content,
                                is_error: output.is_error,
                            });
                        }
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

    async fn run_completion_loop(
        &mut self,
        mut session: Option<&mut SessionWriter>,
        pricing: Option<&PricingTable>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        let schemas = self.tools.schemas();
        let mut turns = 0;
        self.cumulative_cost_usd = 0.0;
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
            let to_send = assemble(&self.history, system_tokens, max_ctx, threshold)?;

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
                    // Execute each tool use and push results as user message
                    let mut tool_results = Vec::new();
                    for block in &response.content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            if let Some(ref mut w) = session {
                                let _ = w.write_line(&SessionLine::ToolCall {
                                    tool_use_id: id.clone(),
                                    tool_name: name.clone(),
                                    input: input.clone(),
                                });
                            }
                            let output = self.execute_tool_maybe_gated(name, input).await;
                            if let Some(ref mut w) = session {
                                let _ = w.write_line(&SessionLine::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: output.content.clone(),
                                    is_error: output.is_error,
                                    duration_ms: None,
                                    path: output.path.clone(),
                                    content_hash: output.content_hash.clone(),
                                    mtime_nanos: output.mtime_nanos,
                                });
                            }
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: output.content,
                                is_error: output.is_error,
                            });
                        }
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
                        if !ask.ask(name, input).await {
                            return ToolOutput::err("User denied the tool call.".to_string());
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
}
