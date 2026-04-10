//! Global rate limiter for multi-agent exploration.
//!
//! Provides per-provider rate limiting with semaphore-based concurrency control
//! and exponential backoff for 429 responses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

/// Rate limit configuration for a provider.
#[derive(Clone, Copy, Debug)]
pub struct RateLimitConfig {
    /// Maximum requests per minute.
    pub requests_per_minute: u32,
    /// Maximum concurrent requests.
    pub max_concurrent: usize,
    /// Exponential backoff base duration (seconds).
    pub backoff_base_secs: u64,
    /// Maximum backoff duration (seconds).
    pub max_backoff_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 60,
            max_concurrent: 10,
            backoff_base_secs: 1,
            max_backoff_secs: 60,
        }
    }
}

impl RateLimitConfig {
    /// Configuration for OpenAI.
    pub fn openai() -> Self {
        Self {
            requests_per_minute: 60,
            max_concurrent: 10,
            backoff_base_secs: 1,
            max_backoff_secs: 60,
        }
    }

    /// Configuration for Anthropic.
    pub fn anthropic() -> Self {
        Self {
            requests_per_minute: 40,
            max_concurrent: 5,
            backoff_base_secs: 1,
            max_backoff_secs: 60,
        }
    }

    /// Configuration for local/Ollama (no limits).
    pub fn local() -> Self {
        Self {
            requests_per_minute: u32::MAX,
            max_concurrent: 100,
            backoff_base_secs: 0,
            max_backoff_secs: 0,
        }
    }
}

/// Sliding window entry for rate limiting.
#[derive(Debug)]
struct WindowEntry {
    count: u32,
    window_start: Instant,
}

/// Global rate limiter for a specific provider.
#[derive(Debug)]
pub struct RateLimiter {
    provider_id: String,
    config: RateLimitConfig,
    /// Semaphore for concurrent request control.
    semaphore: Arc<Semaphore>,
    /// Sliding window for request rate tracking.
    window: Arc<Mutex<WindowEntry>>,
    /// Current backoff duration (exponential).
    current_backoff: Arc<Mutex<Duration>>,
}

impl RateLimiter {
    /// Create a new rate limiter for a provider.
    pub fn new(provider_id: impl Into<String>, config: RateLimitConfig) -> Self {
        let config = config;
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        Self {
            provider_id: provider_id.into(),
            config,
            semaphore,
            window: Arc::new(Mutex::new(WindowEntry {
                count: 0,
                window_start: Instant::now(),
            })),
            current_backoff: Arc::new(Mutex::new(Duration::from_secs(0))),
        }
    }

    /// Acquire permission to make a request.
    ///
    /// This will:
    /// 1. Wait for semaphore (concurrency control)
    /// 2. Check rate limit (sliding window)
    /// 3. Apply backoff if rate limited
    ///
    /// Returns a permit that should be held for the duration of the request.
    pub async fn acquire(&self) -> Result<RateLimitPermit<'_>, RateLimitError> {
        // Step 1: Acquire semaphore for concurrency control
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| RateLimitError::SemaphoreClosed)?;

        // Step 2: Check and update sliding window
        let wait_duration = self.check_and_update_window().await?;

        // Step 3: If at limit, wait
        if wait_duration > Duration::from_secs(0) {
            tokio::time::sleep(wait_duration).await;
        }

        // Step 4: Apply any active backoff
        let backoff = *self.current_backoff.lock().await;
        if backoff > Duration::from_secs(0) {
            tokio::time::sleep(backoff).await;
        }

        Ok(RateLimitPermit {
            _permit,
            limiter: self,
        })
    }

    /// Check rate limit and return wait duration if at limit.
    async fn check_and_update_window(&self) -> Result<Duration, RateLimitError> {
        let mut window = self.window.lock().await;
        let now = Instant::now();
        let window_duration = Duration::from_secs(60);

        // Reset window if expired
        if now.duration_since(window.window_start) >= window_duration {
            window.count = 0;
            window.window_start = now;
        }

        // Check if at limit
        if window.count >= self.config.requests_per_minute {
            let elapsed = now.duration_since(window.window_start);
            let wait = window_duration.saturating_sub(elapsed);
            return Ok(wait);
        }

        // Increment count
        window.count += 1;
        Ok(Duration::from_secs(0))
    }

    /// Notify that a 429 (rate limited) response was received.
    ///
    /// This will increase the backoff duration exponentially.
    pub async fn on_rate_limited(&self) {
        let mut backoff = self.current_backoff.lock().await;
        let new_backoff = if *backoff == Duration::from_secs(0) {
            Duration::from_secs(self.config.backoff_base_secs)
        } else {
            (*backoff * 2).min(Duration::from_secs(self.config.max_backoff_secs))
        };
        *backoff = new_backoff;

        tracing::warn!(
            provider = %self.provider_id,
            backoff_secs = new_backoff.as_secs(),
            "Rate limited, increasing backoff"
        );
    }

    /// Notify that a successful request was made.
    ///
    /// This will gradually reduce the backoff duration.
    pub async fn on_success(&self) {
        let mut backoff = self.current_backoff.lock().await;
        if *backoff > Duration::from_secs(0) {
            // Reduce by half, but not below 0
            *backoff = *backoff / 2;
        }
    }

    /// Get current statistics for monitoring.
    pub async fn stats(&self) -> RateLimiterStats {
        let window = self.window.lock().await;
        let backoff = self.current_backoff.lock().await;

        RateLimiterStats {
            provider_id: self.provider_id.clone(),
            requests_in_current_window: window.count,
            requests_per_minute_limit: self.config.requests_per_minute,
            available_permits: self.semaphore.available_permits(),
            max_concurrent: self.config.max_concurrent,
            current_backoff_secs: backoff.as_secs(),
        }
    }
}

/// Permit held during a rate-limited request.
///
/// When dropped, the semaphore permit is released.
pub struct RateLimitPermit<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
    limiter: &'a RateLimiter,
}

impl<'a> RateLimitPermit<'a> {
    /// Mark this request as successful.
    pub async fn success(self) {
        self.limiter.on_success().await;
    }

    /// Mark this request as rate limited (429).
    pub async fn rate_limited(self) {
        self.limiter.on_rate_limited().await;
    }
}

/// Rate limiter statistics for monitoring.
#[derive(Clone, Debug)]
pub struct RateLimiterStats {
    pub provider_id: String,
    pub requests_in_current_window: u32,
    pub requests_per_minute_limit: u32,
    pub available_permits: usize,
    pub max_concurrent: usize,
    pub current_backoff_secs: u64,
}

/// Rate limit error types.
#[derive(Debug)]
pub enum RateLimitError {
    SemaphoreClosed,
    RateLimitExceeded,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitError::SemaphoreClosed => write!(f, "Semaphore closed"),
            RateLimitError::RateLimitExceeded => write!(f, "Rate limit exceeded"),
        }
    }
}

impl std::error::Error for RateLimitError {}

/// Registry of rate limiters for multiple providers.
#[derive(Debug, Default, Clone)]
pub struct RateLimiterRegistry {
    limiters: Arc<Mutex<HashMap<String, Arc<RateLimiter>>>>,
}

impl RateLimiterRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            limiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create a rate limiter for a provider.
    pub async fn get_or_create(
        &self,
        provider_id: impl Into<String>,
        config: RateLimitConfig,
    ) -> Arc<RateLimiter> {
        let provider_id = provider_id.into();
        let mut limiters = self.limiters.lock().await;

        if let Some(limiter) = limiters.get(&provider_id) {
            return limiter.clone();
        }

        let limiter = Arc::new(RateLimiter::new(&provider_id, config));
        limiters.insert(provider_id, limiter.clone());
        limiter
    }

    /// Get a rate limiter if it exists.
    pub async fn get(&self, provider_id: &str) -> Option<Arc<RateLimiter>> {
        let limiters = self.limiters.lock().await;
        limiters.get(provider_id).cloned()
    }

    /// Get statistics for all providers.
    pub async fn all_stats(&self) -> Vec<RateLimiterStats> {
        let limiters = self.limiters.lock().await;
        let mut stats = Vec::new();
        for limiter in limiters.values() {
            stats.push(limiter.stats().await);
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new("test", RateLimitConfig {
            requests_per_minute: 10,
            max_concurrent: 2,
            backoff_base_secs: 1,
            max_backoff_secs: 10,
        });

        // Should acquire successfully
        let permit = limiter.acquire().await.unwrap();
        permit.success().await;

        let stats = limiter.stats().await;
        assert_eq!(stats.requests_in_current_window, 1);
    }

    #[tokio::test]
    async fn test_concurrent_limit() {
        let limiter = RateLimiter::new("test", RateLimitConfig {
            requests_per_minute: 100,
            max_concurrent: 2,
            backoff_base_secs: 1,
            max_backoff_secs: 10,
        });

        // Acquire 2 permits (max)
        let _permit1 = limiter.acquire().await.unwrap();
        let _permit2 = limiter.acquire().await.unwrap();

        // Third should wait (but we can't easily test that without timeout)
        let stats = limiter.stats().await;
        assert_eq!(stats.available_permits, 0);
    }

    #[tokio::test]
    async fn test_backoff_increases() {
        let limiter = RateLimiter::new("test", RateLimitConfig {
            requests_per_minute: 100,
            max_concurrent: 10,
            backoff_base_secs: 1,
            max_backoff_secs: 10,
        });

        // Simulate rate limited responses
        let permit = limiter.acquire().await.unwrap();
        permit.rate_limited().await;

        let stats = limiter.stats().await;
        assert_eq!(stats.current_backoff_secs, 1);

        let permit = limiter.acquire().await.unwrap();
        permit.rate_limited().await;

        let stats = limiter.stats().await;
        assert_eq!(stats.current_backoff_secs, 2);
    }

    #[tokio::test]
    async fn test_backoff_decreases_on_success() {
        let limiter = RateLimiter::new("test", RateLimitConfig {
            requests_per_minute: 100,
            max_concurrent: 10,
            backoff_base_secs: 1,
            max_backoff_secs: 10,
        });

        // Set backoff to 4 seconds
        let permit = limiter.acquire().await.unwrap();
        permit.rate_limited().await;
        let permit = limiter.acquire().await.unwrap();
        permit.rate_limited().await;
        let permit = limiter.acquire().await.unwrap();
        permit.rate_limited().await;

        let stats = limiter.stats().await;
        assert_eq!(stats.current_backoff_secs, 4);

        // Success should halve it
        let permit = limiter.acquire().await.unwrap();
        permit.success().await;

        let stats = limiter.stats().await;
        assert_eq!(stats.current_backoff_secs, 2);
    }

    #[tokio::test]
    async fn test_registry() {
        let registry = RateLimiterRegistry::new();

        let limiter1 = registry
            .get_or_create("openai", RateLimitConfig::openai())
            .await;
        let limiter2 = registry
            .get_or_create("openai", RateLimitConfig::anthropic()) // Different config, should be ignored
            .await;

        // Should return same instance
        assert!(Arc::ptr_eq(&limiter1, &limiter2));
    }
}
