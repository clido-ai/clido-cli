//! Unified error type for Clido.

use thiserror::Error;

/// Top-level error type.
#[derive(Error, Debug)]
pub enum ClidoError {
    #[error("provider error: {0}")]
    Provider(String),

    /// Rate limited by provider — includes reset time if known.
    #[error("rate limited: {message}")]
    RateLimited {
        message: String,
        /// Seconds until the rate limit resets (from Retry-After or X-RateLimit-Reset).
        retry_after_secs: Option<u64>,
        /// Whether this is a subscription/quota limit (long reset) vs burst limit (short reset).
        is_subscription_limit: bool,
    },

    #[error("tool error: {tool_name}: {message}")]
    Tool { tool_name: String, message: String },

    #[error("context limit exceeded: {tokens} tokens")]
    ContextLimit { tokens: u64 },

    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("permission denied: {tool_name}")]
    PermissionDenied { tool_name: String },

    #[error("planner error: {0}")]
    Planner(String),

    #[error("workflow error: {0}")]
    Workflow(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("budget exceeded")]
    BudgetExceeded,

    /// Agent called the same tool with identical failing output 3+ times in a row.
    /// This indicates a stuck loop that would otherwise spend tokens indefinitely.
    #[error(
        "doom loop detected: tool '{tool}' failed with the same error 3 times in a row: {error}"
    )]
    DoomLoop { tool: String, error: String },

    #[error("max_turns exceeded")]
    MaxTurnsExceeded,

    /// Provider returned `tool_use` stop reason but no valid tool blocks (or duplicate ids).
    #[error("malformed model output: {detail}")]
    MalformedModelOutput { detail: String },

    /// Per-user-turn wall clock budget exceeded.
    #[error("max wall time per turn exceeded")]
    MaxWallTimeExceeded,

    /// Too many tool invocations in a single user turn.
    #[error("max tool calls per turn exceeded")]
    MaxToolCallsPerTurnExceeded,

    /// Heuristic: agent produced tool traffic without measurable progress.
    #[error("stall detected: {reason}")]
    StallDetected { reason: String },

    #[error("interrupted by user")]
    Interrupted,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Result type using ClidoError.
pub type Result<T> = std::result::Result<T, ClidoError>;

impl ClidoError {
    /// Whether to rewind in-memory conversation history to `history_before_turn` after a failed
    /// agent `run` / `run_next_turn` / `run_continue`.
    ///
    /// **Why:** On generic failures we drop the whole turn so the next message does not create
    /// invalid same-role sequences. **Recoverable** errors are different:
    /// - `RateLimited` — the user or TUI retries; truncating would erase tool traces and break
    ///   "continue" / auto-resume (model sees no prior task).
    /// - `ContextLimit` — compaction failed; keeping the user turn allows `/compact` or a
    ///   follow-up without losing the prompt.
    /// - `MaxTurnsExceeded` / `BudgetExceeded` — if the model already produced assistant (and
    ///   possibly tool) content this turn, we **keep** it so auto-continue and budget handling
    ///   still see a consistent transcript. If the turn never got past the new user line
    ///   (`history_len == history_before_turn + 1`), we truncate like a bare failure.
    ///   The same check applies to `run_continue`: `history_before_turn` is the length at
    ///   continue-entry; truncate only when exactly one message was appended since then (e.g. one
    ///   assistant reply before hitting the limit).
    #[must_use]
    pub fn should_truncate_history_after_failed_run(
        &self,
        history_len: usize,
        history_before_turn: usize,
    ) -> bool {
        match self {
            ClidoError::RateLimited { .. } | ClidoError::ContextLimit { .. } => false,
            ClidoError::MaxTurnsExceeded
            | ClidoError::BudgetExceeded
            | ClidoError::MaxWallTimeExceeded
            | ClidoError::MaxToolCallsPerTurnExceeded
            | ClidoError::StallDetected { .. } => {
                history_len == history_before_turn.saturating_add(1)
            }
            ClidoError::MalformedModelOutput { .. } => {
                // Usually raised before an assistant message is committed; truncate if nothing
                // beyond the user line was appended.
                history_len <= history_before_turn.saturating_add(1)
            }
            _ => true,
        }
    }

    /// Stable label for CLI JSON / stream-json `exit_status` and session `Result` lines when a run fails.
    ///
    /// Successful runs use `"completed"` in JSON output (`emit_result`); the TUI writes `"success"` to the session file on success.
    #[must_use]
    pub fn agent_exit_status(&self) -> &'static str {
        match self {
            ClidoError::MaxTurnsExceeded => "max_turns_reached",
            ClidoError::BudgetExceeded => "budget_exceeded",
            ClidoError::MaxWallTimeExceeded => "max_wall_time_exceeded",
            ClidoError::MaxToolCallsPerTurnExceeded => "max_tool_calls_per_turn",
            ClidoError::StallDetected { .. } => "stall_detected",
            ClidoError::MalformedModelOutput { .. } => "malformed_model_output",
            ClidoError::Interrupted => "interrupted",
            ClidoError::RateLimited { .. } => "rate_limited",
            ClidoError::DoomLoop { .. } => "doom_loop",
            _ => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_display() {
        let e = ClidoError::Provider("bad api key".to_string());
        assert!(e.to_string().contains("provider error"));
        assert!(e.to_string().contains("bad api key"));
    }

    #[test]
    fn tool_error_display() {
        let e = ClidoError::Tool {
            tool_name: "Read".to_string(),
            message: "file not found".to_string(),
        };
        assert!(e.to_string().contains("tool error"));
        assert!(e.to_string().contains("Read"));
        assert!(e.to_string().contains("file not found"));
    }

    #[test]
    fn context_limit_display() {
        let e = ClidoError::ContextLimit { tokens: 200_001 };
        assert!(e.to_string().contains("context limit exceeded"));
        assert!(e.to_string().contains("200001"));
    }

    #[test]
    fn session_not_found_display() {
        let e = ClidoError::SessionNotFound {
            session_id: "abc-123".to_string(),
        };
        assert!(e.to_string().contains("session not found"));
        assert!(e.to_string().contains("abc-123"));
    }

    #[test]
    fn permission_denied_display() {
        let e = ClidoError::PermissionDenied {
            tool_name: "Write".to_string(),
        };
        assert!(e.to_string().contains("permission denied"));
        assert!(e.to_string().contains("Write"));
    }

    #[test]
    fn planner_error_display() {
        let e = ClidoError::Planner("bad plan".to_string());
        assert!(e.to_string().contains("planner error"));
    }

    #[test]
    fn workflow_error_display() {
        let e = ClidoError::Workflow("step failed".to_string());
        assert!(e.to_string().contains("workflow error"));
    }

    #[test]
    fn config_error_display() {
        let e = ClidoError::Config("missing field".to_string());
        assert!(e.to_string().contains("config error"));
    }

    #[test]
    fn budget_exceeded_display() {
        let e = ClidoError::BudgetExceeded;
        assert!(e.to_string().contains("budget exceeded"));
    }

    #[test]
    fn doom_loop_display() {
        let e = ClidoError::DoomLoop {
            tool: "Bash".to_string(),
            error: "command not found".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("doom loop"), "got: {s}");
        assert!(s.contains("Bash"), "got: {s}");
        assert!(s.contains("command not found"), "got: {s}");
    }

    #[test]
    fn max_turns_exceeded_display() {
        let e = ClidoError::MaxTurnsExceeded;
        assert!(e.to_string().contains("max_turns exceeded"));
    }

    #[test]
    fn interrupted_display() {
        let e = ClidoError::Interrupted;
        assert!(e.to_string().contains("interrupted"));
    }

    #[test]
    fn io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = ClidoError::from(io_err);
        assert!(e.to_string().contains("io error"));
    }

    #[test]
    fn json_error_from() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let e = ClidoError::from(json_err);
        assert!(e.to_string().contains("json error"));
    }

    #[test]
    fn truncate_policy_rate_limit_never() {
        let e = ClidoError::RateLimited {
            message: "slow down".into(),
            retry_after_secs: Some(60),
            is_subscription_limit: false,
        };
        assert!(!e.should_truncate_history_after_failed_run(99, 0));
        assert!(!e.should_truncate_history_after_failed_run(3, 1));
    }

    #[test]
    fn truncate_policy_max_turns_only_when_just_user() {
        let e = ClidoError::MaxTurnsExceeded;
        assert!(e.should_truncate_history_after_failed_run(1, 0)); // only new user
        assert!(!e.should_truncate_history_after_failed_run(3, 0)); // user + more
    }

    #[test]
    fn truncate_policy_provider_always() {
        let e = ClidoError::Provider("x".into());
        assert!(e.should_truncate_history_after_failed_run(5, 0));
    }

    #[test]
    fn malformed_model_output_display() {
        let e = ClidoError::MalformedModelOutput {
            detail: "duplicate tool id".into(),
        };
        let s = e.to_string();
        assert!(s.contains("malformed model output"), "got: {s}");
        assert!(s.contains("duplicate"), "got: {s}");
    }

    #[test]
    fn max_wall_time_display() {
        let e = ClidoError::MaxWallTimeExceeded;
        assert!(e.to_string().contains("max wall time"));
    }

    #[test]
    fn max_tool_calls_per_turn_display() {
        let e = ClidoError::MaxToolCallsPerTurnExceeded;
        assert!(e.to_string().contains("max tool calls per turn"));
    }

    #[test]
    fn stall_detected_display() {
        let e = ClidoError::StallDetected {
            reason: "all errors".into(),
        };
        let s = e.to_string();
        assert!(s.contains("stall detected"), "got: {s}");
        assert!(s.contains("all errors"), "got: {s}");
    }

    #[test]
    fn truncate_policy_malformed_when_only_user_line() {
        let e = ClidoError::MalformedModelOutput {
            detail: "x".into(),
        };
        assert!(e.should_truncate_history_after_failed_run(1, 0));
        assert!(!e.should_truncate_history_after_failed_run(3, 0));
    }

    #[test]
    fn truncate_policy_stall_like_max_turns() {
        let e = ClidoError::StallDetected {
            reason: "x".into(),
        };
        assert!(e.should_truncate_history_after_failed_run(1, 0));
        assert!(!e.should_truncate_history_after_failed_run(3, 0));
    }

    #[test]
    fn agent_exit_status_labels() {
        assert_eq!(
            ClidoError::MaxWallTimeExceeded.agent_exit_status(),
            "max_wall_time_exceeded"
        );
        assert_eq!(
            ClidoError::MalformedModelOutput {
                detail: "x".into()
            }
            .agent_exit_status(),
            "malformed_model_output"
        );
        assert_eq!(
            ClidoError::DoomLoop {
                tool: "Bash".into(),
                error: "x".into(),
            }
            .agent_exit_status(),
            "doom_loop"
        );
        assert_eq!(ClidoError::Provider("x".into()).agent_exit_status(), "error");
    }
}
