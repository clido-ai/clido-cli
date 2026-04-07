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

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::ToolFailureKind;

    #[test]
    fn classify_typed_transport_and_rate_limit() {
        let r = classify_retry(
            Some(ToolFailureKind::Transport),
            "Read",
            "anything",
        )
        .unwrap();
        assert_eq!(r.source, RetryDecisionSource::TypedKind);
        assert!(matches!(
            r.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 1000 }
        ));

        let r2 = classify_retry(
            Some(ToolFailureKind::RateLimited),
            "Read",
            "",
        )
        .unwrap();
        assert!(matches!(
            r2.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 1000 }
        ));
    }

    #[test]
    fn classify_typed_timeout_io_and_non_retryable() {
        let t = classify_retry(Some(ToolFailureKind::Timeout), "X", "").unwrap();
        assert!(matches!(
            t.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 800 }
        ));
        let io = classify_retry(Some(ToolFailureKind::Io), "X", "").unwrap();
        assert!(matches!(
            io.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 400 }
        ));
        assert!(classify_retry(
            Some(ToolFailureKind::ValidationInput),
            "Read",
            "timeout in message"
        )
        .is_none());
        assert!(classify_retry(Some(ToolFailureKind::Logical), "Read", "network down").is_none());
    }

    #[test]
    fn classify_unknown_falls_back_to_legacy_substrings() {
        let r = classify_retry(
            Some(ToolFailureKind::Unknown),
            "Read",
            "connection reset by peer",
        )
        .unwrap();
        assert_eq!(r.source, RetryDecisionSource::LegacyHeuristic);
        assert!(matches!(
            r.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 1000 }
        ));

        let r2 = classify_retry(None, "Read", "Permission Denied on file").unwrap();
        assert_eq!(r2.source, RetryDecisionSource::LegacyHeuristic);
        assert!(matches!(r2.strategy, RetryStrategy::RetryOnce));

        let r3 = classify_retry(None, "Bash", "device or resource busy").unwrap();
        assert!(matches!(
            r3.strategy,
            RetryStrategy::WaitAndRetry { delay_ms: 500 }
        ));

        let r4 = classify_retry(
            None,
            "WebFetch",
            "failed to resolve DNS name",
        )
        .unwrap();
        assert!(matches!(r4.strategy, RetryStrategy::RetryOnce));

        assert!(classify_retry(None, "Read", "nothing recognizable here").is_none());
    }

    #[test]
    fn backoff_delay_respects_cap_and_jitter() {
        let d = backoff_delay_ms(100, 0, 500, 10);
        assert!(d >= 1 && d <= 500);
        let d2 = backoff_delay_ms(10_000, 7, 50, 50);
        assert_eq!(d2, 50);
    }
}
