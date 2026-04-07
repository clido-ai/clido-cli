//! Minimum spacing between provider completion calls.

use std::time::{Duration, Instant};

/// Sleep if needed so at least `min_interval_ms` passes since `last_end` (previous call completion).
pub(crate) async fn throttle_before_complete(last_end: &mut Option<Instant>, min_interval_ms: u32) {
    if min_interval_ms == 0 {
        return;
    }
    let min_d = Duration::from_millis(min_interval_ms as u64);
    let now = Instant::now();
    if let Some(prev) = *last_end {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < min_d {
            tokio::time::sleep(min_d - elapsed).await;
        }
    }
}

/// Record that a provider call finished (wall clock end).
pub(crate) fn mark_complete_finished(last_end: &mut Option<Instant>) {
    *last_end = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn throttle_zero_interval_is_noop() {
        let mut last = None;
        let t0 = Instant::now();
        throttle_before_complete(&mut last, 0).await;
        assert!(t0.elapsed() < std::time::Duration::from_millis(20));
    }

    #[tokio::test]
    async fn throttle_skips_sleep_without_prior_mark() {
        let mut last = None;
        let t0 = Instant::now();
        throttle_before_complete(&mut last, 200).await;
        assert!(t0.elapsed() < std::time::Duration::from_millis(20));
    }

    #[tokio::test]
    async fn throttle_sleeps_after_recent_completion() {
        let mut last = None;
        mark_complete_finished(&mut last);
        let t0 = Instant::now();
        throttle_before_complete(&mut last, 40).await;
        assert!(t0.elapsed() >= std::time::Duration::from_millis(35));
    }
}
