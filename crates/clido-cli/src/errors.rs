//! CLI exit codes and error types.

use thiserror::Error;

/// CLI exit codes per spec: 0 success, 1 runtime, 2 usage/config, 3 soft limit.
/// Doctor: 0 all pass, 1 mandatory failure, 2 warnings only.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    /// Config-related error; display with "Error [Config]: {}" per CLI spec.
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    SoftLimit(String),
    #[error("{0}")]
    Interrupted(String),
    #[error("{0}")]
    DoctorMandatory(String),
    #[error("{0}")]
    DoctorWarnings(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Config(_) => 2,
            CliError::SoftLimit(_) => 3,
            CliError::Interrupted(_) => 130,
            CliError::DoctorMandatory(_) => 1,
            CliError::DoctorWarnings(_) => 2,
        }
    }
}
