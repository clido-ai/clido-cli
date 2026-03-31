//! Shared backoff / retry helpers used by both the Anthropic and OpenAI providers.

use std::time::Duration;

// ── Max-attempt constants ────────────────────────────────────────────────────

pub const MAX_RATE_LIMIT_ATTEMPTS: u32 = 6;
pub const MAX_SERVER_ERROR_ATTEMPTS: u32 = 5;
pub const MAX_NETWORK_ATTEMPTS: u32 = 4;

// ── Backoff calculations ─────────────────────────────────────────────────────

/// Backoff for rate limits: 15s, 30s, 60s, 90s, 120s, …
pub fn rate_limit_backoff_secs(attempt: u32) -> u64 {
    let base: u64 = 15 * (1u64 << (attempt - 1).min(3));
    base.min(120)
}

/// Backoff for server errors: 1s, 2s, 4s, 8s, 16s.
pub fn server_error_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(4)).min(16)
}

/// Backoff for network errors: 1s, 2s, 4s.
pub fn network_backoff_secs(attempt: u32) -> u64 {
    (1u64 << (attempt - 1).min(2)).min(4)
}

// ── Retry-After header parsing ───────────────────────────────────────────────

/// Like [`parse_retry_after`] but returns raw seconds (useful when the caller
/// needs the numeric value for comparisons and message formatting).
pub fn parse_retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Parse `Retry-After` header value (integer seconds only; HTTP-date not supported).
/// Caps at 5 minutes to avoid waiting forever on a bad header value.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    parse_retry_after_secs(headers).map(|secs| Duration::from_secs(secs.min(300)))
}

// ── Subscription / quota detection ───────────────────────────────────────────

/// Heuristic: does this rate-limit look like a subscription/quota limit rather
/// than a transient burst limit?
///
/// Long retry-after (>5 min) or specific keywords in the error body indicate a
/// quota/subscription limit that won't reset soon.
pub fn is_subscription_limit(retry_after_secs: Option<u64>, body: &str) -> bool {
    retry_after_secs.is_some_and(|s| s > 300)
        || body.contains("quota")
        || body.contains("subscription")
        || body.contains("limit exceeded")
        || body.contains("allowance")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_backoff_increases_and_caps() {
        assert_eq!(rate_limit_backoff_secs(1), 15);
        assert_eq!(rate_limit_backoff_secs(2), 30);
        assert_eq!(rate_limit_backoff_secs(3), 60);
        assert_eq!(rate_limit_backoff_secs(4), 120);
        assert_eq!(rate_limit_backoff_secs(5), 120); // capped
        assert_eq!(rate_limit_backoff_secs(10), 120); // still capped
    }

    #[test]
    fn server_error_backoff_exponential() {
        assert_eq!(server_error_backoff_secs(1), 1);
        assert_eq!(server_error_backoff_secs(2), 2);
        assert_eq!(server_error_backoff_secs(3), 4);
        assert_eq!(server_error_backoff_secs(4), 8);
        assert_eq!(server_error_backoff_secs(5), 16);
        assert_eq!(server_error_backoff_secs(6), 16); // capped
    }

    #[test]
    fn network_backoff_exponential() {
        assert_eq!(network_backoff_secs(1), 1);
        assert_eq!(network_backoff_secs(2), 2);
        assert_eq!(network_backoff_secs(3), 4);
        assert_eq!(network_backoff_secs(4), 4); // capped
        assert_eq!(network_backoff_secs(10), 4); // still capped
    }
}
