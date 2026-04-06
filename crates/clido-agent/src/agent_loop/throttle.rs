//! Minimum spacing between provider completion calls.

use std::time::{Duration, Instant};

/// Sleep if needed so at least `min_interval_ms` passes since `last_end` (previous call completion).
pub(crate) async fn throttle_before_complete(
    last_end: &mut Option<Instant>,
    min_interval_ms: u32,
) {
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
