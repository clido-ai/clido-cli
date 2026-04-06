//! Optional counters for agent loop observability (default: no-op).

use clido_core::ToolFailureKind;

/// Hooks for production metrics / tracing backends.
pub trait AgentMetrics: Send + Sync {
    fn model_turn_completed(&self, _turn_index: u32) {}
    fn tool_call_finished(&self, _name: &str, _is_error: bool, _kind: Option<ToolFailureKind>) {}
    fn tool_retry_scheduled(&self, _name: &str, _attempt: u32) {}
    fn validation_rejected(&self, _tool: &str) {}
    fn stall_detected(&self) {}
    fn doom_detected(&self, _tool: &str) {}
}

/// Default implementation that does nothing.
pub struct NoopAgentMetrics;

impl AgentMetrics for NoopAgentMetrics {}
