//! Unified error type for Clido.

use thiserror::Error;

/// Top-level error type.
#[derive(Error, Debug)]
pub enum ClidoError {
    #[error("provider error: {0}")]
    Provider(String),

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
