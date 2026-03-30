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
}
