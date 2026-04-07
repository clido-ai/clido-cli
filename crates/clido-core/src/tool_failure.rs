//! Classification of tool failures for retry policy and observability.

/// Stable categories for tool and transport failures (used by the agent loop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolFailureKind {
    /// Input failed JSON Schema validation before execute.
    ValidationInput,
    /// Network, DNS, TLS, connection reset, etc.
    Transport,
    /// HTTP 429 or provider rate-limit message.
    RateLimited,
    /// Tool or subprocess exceeded its timeout.
    Timeout,
    /// User or policy denied the operation.
    PermissionDenied,
    /// Non-transient tool failure (file not found after retry policy, bad args, etc.).
    Logical,
    /// Resource not found (tool missing from registry — usually pre-execute).
    NotFound,
    /// Local I/O from the tool runtime.
    Io,
    /// Legacy paths: infer from message heuristics in the agent.
    #[default]
    Unknown,
}

impl ToolFailureKind {
    #[must_use]
    pub fn is_retryable_heuristic(self) -> bool {
        matches!(
            self,
            Self::Transport | Self::RateLimited | Self::Timeout | Self::Io
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ToolFailureKind;

    #[test]
    fn retryable_heuristic_matches_transport_like() {
        for k in [
            ToolFailureKind::Transport,
            ToolFailureKind::RateLimited,
            ToolFailureKind::Timeout,
            ToolFailureKind::Io,
        ] {
            assert!(k.is_retryable_heuristic(), "{k:?}");
        }
    }

    #[test]
    fn retryable_heuristic_false_for_validation_and_logical() {
        for k in [
            ToolFailureKind::ValidationInput,
            ToolFailureKind::PermissionDenied,
            ToolFailureKind::Logical,
            ToolFailureKind::NotFound,
            ToolFailureKind::Unknown,
        ] {
            assert!(!k.is_retryable_heuristic(), "{k:?}");
        }
    }
}
