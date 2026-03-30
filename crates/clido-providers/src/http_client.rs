//! Shared HTTP client construction for all providers.

use std::time::Duration;

/// Default total request timeout (connect + send + body read).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Default TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Build a pre-configured `reqwest::Client` with standard timeouts.
///
/// `user_agent` is used as-is; callers are expected to resolve aliases/env
/// overrides before calling this function.
pub fn build_http_client(user_agent: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(user_agent)
        .build()
        .expect("failed to build reqwest::Client — TLS backend unavailable")
}
