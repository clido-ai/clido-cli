//! Typed-first retry classification for tool failures.

use clido_core::ToolFailureKind;

#[derive(Debug, Clone, Copy)]
pub(crate) enum RetryStrategy {
    RetryOnce,
    WaitAndRetry { delay_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryDecisionSource {
    TypedKind,
    LegacyHeuristic,
}

pub(crate) struct ClassifiedRetry {
    pub strategy: RetryStrategy,
    pub source: RetryDecisionSource,
}

pub(crate) fn classify_retry(
    kind: Option<ToolFailureKind>,
    tool_name: &str,
    message: &str,
) -> Option<ClassifiedRetry> {
    match kind {
        Some(ToolFailureKind::Transport | ToolFailureKind::RateLimited) => {
            return Some(ClassifiedRetry {
                strategy: RetryStrategy::WaitAndRetry { delay_ms: 1000 },
                source: RetryDecisionSource::TypedKind,
            });
        }
        Some(ToolFailureKind::Timeout) => {
            return Some(ClassifiedRetry {
                strategy: RetryStrategy::WaitAndRetry { delay_ms: 800 },
                source: RetryDecisionSource::TypedKind,
            });
        }
        Some(ToolFailureKind::Io) => {
            return Some(ClassifiedRetry {
                strategy: RetryStrategy::WaitAndRetry { delay_ms: 400 },
                source: RetryDecisionSource::TypedKind,
            });
        }
        Some(
            ToolFailureKind::ValidationInput
            | ToolFailureKind::PermissionDenied
            | ToolFailureKind::Logical
            | ToolFailureKind::NotFound,
        ) => return None,
        Some(ToolFailureKind::Unknown) | None => {}
    }

    legacy_string_retry(tool_name, message).map(|strategy| ClassifiedRetry {
        strategy,
        source: RetryDecisionSource::LegacyHeuristic,
    })
}

fn legacy_string_retry(tool_name: &str, error: &str) -> Option<RetryStrategy> {
    let err_lower = error.to_lowercase();
    if err_lower.contains("timeout")
        || err_lower.contains("timed out")
        || err_lower.contains("connection")
        || err_lower.contains("network")
        || err_lower.contains("temporarily unavailable")
        || err_lower.contains("rate limit")
        || err_lower.contains("too many requests")
    {
        return Some(RetryStrategy::WaitAndRetry { delay_ms: 1000 });
    }
    if err_lower.contains("permission denied")
        || err_lower.contains("access denied")
        || err_lower.contains("unauthorized")
    {
        return Some(RetryStrategy::RetryOnce);
    }
    if tool_name == "Bash"
        && (err_lower.contains("resource temporarily unavailable")
            || err_lower.contains("try again")
            || err_lower.contains("device or resource busy"))
    {
        return Some(RetryStrategy::WaitAndRetry { delay_ms: 500 });
    }
    if matches!(tool_name, "WebFetch" | "WebSearch")
        && (err_lower.contains("dns")
            || err_lower.contains("resolve")
            || err_lower.contains("certificate")
            || err_lower.contains("ssl"))
    {
        return Some(RetryStrategy::RetryOnce);
    }
    if err_lower.contains("i/o error")
        || err_lower.contains("io error")
        || err_lower.contains("os error")
        || err_lower.contains("broken pipe")
        || err_lower.contains("connection reset")
        || err_lower.contains("resource temporarily unavailable")
    {
        return Some(RetryStrategy::WaitAndRetry { delay_ms: 400 });
    }
    None
}

/// Apply cap and deterministic jitter spread from `jitter_numerator` / 100 of base.
#[must_use]
pub(crate) fn backoff_delay_ms(base: u64, attempt: u32, cap: u64, jitter_numerator: u8) -> u64 {
    let b = base.min(cap).max(1);
    let jitter = (b * jitter_numerator as u64) / 100;
    let slot = (attempt as u64 % 8).saturating_add(1);
    (b + (jitter * slot) / 8).min(cap)
}
