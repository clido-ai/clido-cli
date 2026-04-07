//! Optional counters for agent loop observability (default: no-op).

use clido_core::ToolFailureKind;

/// Hooks for production metrics / tracing backends.
pub trait AgentMetrics: Send + Sync {
    fn model_turn_completed(&self, _turn_index: u32) {}
    fn tool_call_finished(&self, _name: &str, _is_error: bool, _kind: Option<ToolFailureKind>) {}
    fn tool_retry_scheduled(&self, _name: &str, _attempt: u32) {}
    /// Retry was chosen from legacy substring heuristics (not `ToolFailureKind` alone).
    fn tool_retry_legacy_heuristic(&self, _name: &str) {}
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

    fn tool_retry_legacy_heuristic(&self, name: &str) {
        tracing::debug!(
            target: "clido::metrics",
            event = "tool_retry_legacy_heuristic",
            tool = name
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

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::ToolFailureKind;

    #[test]
    fn noop_agent_metrics_no_panic() {
        let m = NoopAgentMetrics;
        m.model_turn_completed(0);
        m.tool_call_finished("Read", false, None);
        m.tool_retry_scheduled("Read", 1);
        m.tool_retry_legacy_heuristic("Read");
        m.validation_rejected("Write");
        m.stall_detected();
        m.doom_detected("Bash");
    }

    #[test]
    fn tracing_agent_metrics_emits_debug_events() {
        let m = TracingAgentMetrics;
        m.model_turn_completed(2);
        m.tool_call_finished("Grep", true, Some(ToolFailureKind::Timeout));
        m.tool_retry_scheduled("Read", 3);
        m.tool_retry_legacy_heuristic("WebFetch");
        m.validation_rejected("Edit");
        m.stall_detected();
        m.doom_detected("MultiEdit");
    }
}
