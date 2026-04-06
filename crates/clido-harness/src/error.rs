//! Harness domain errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Protocol(String),

    #[error("task id already exists: {0}")]
    DuplicateTaskId(String),

    #[error("unknown task id: {0}")]
    UnknownTaskId(String),

    #[error("task {0} is already verified pass")]
    AlreadyPass(String),

    #[error("verification rejected: {0}")]
    VerificationRejected(String),

    #[error("loop guard: {0}")]
    LoopGuard(String),
}

pub type Result<T> = std::result::Result<T, HarnessError>;
