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

/// Emit metric events as `tracing` events (target `clido::metrics`, level DEBUG).
/// Enable with `CLIDO_TRACE_METRICS=1` in the CLI / TUI setup path.
pub struct TracingAgentMetrics;

impl AgentMetrics for TracingAgentMetrics {
    fn model_turn_completed(&self, turn_index: u32) {
        tracing::debug!(target: "clido::metrics", event = "model_turn_completed", turn_index);
    }

    fn tool_call_finished(&self, name: &str, is_error: bool, kind: Option<ToolFailureKind>) {
        tracing::debug!(
            target: "clido::metrics",
            event = "tool_call_finished",
            tool = name,
            is_error,
            ?kind
        );
    }

    fn tool_retry_scheduled(&self, name: &str, attempt: u32) {
        tracing::debug!(
            target: "clido::metrics",
            event = "tool_retry_scheduled",
            tool = name,
            attempt
        );
    }

    fn validation_rejected(&self, tool: &str) {
        tracing::debug!(
            target: "clido::metrics",
            event = "validation_rejected",
            tool
        );
    }

    fn stall_detected(&self) {
        tracing::debug!(target: "clido::metrics", event = "stall_detected");
    }

    fn doom_detected(&self, tool: &str) {
        tracing::debug!(target: "clido::metrics", event = "doom_detected", tool);
    }
}
