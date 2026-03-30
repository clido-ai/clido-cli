//! Desktop notification and terminal bell for task completion.
//!
//! This module is compiled only when the `desktop-notify` feature is enabled.
//! The `notify_done` function is the sole public entry point.

/// Minimum task duration in seconds before a notification is fired.
pub const MIN_ELAPSED_SECS: u64 = 10;

/// Fire a desktop notification and terminal bell when a task completes.
///
/// Silently no-ops for tasks shorter than `MIN_ELAPSED_SECS` seconds to avoid
/// notification spam for quick responses. The terminal bell fires unconditionally
/// (no feature flag needed); the OS desktop notification requires the
/// `desktop-notify` Cargo feature.
///
/// Falls back silently on any failure — notifications are non-fatal.
pub fn notify_done(session_id: &str, elapsed_secs: u64, cost_usd: f64) {
    if !should_notify(elapsed_secs) {
        return;
    }

    // Terminal bell — always fires regardless of desktop-notify feature.
    eprint!("\x07");

    #[cfg(feature = "desktop-notify")]
    {
        let summary = "clido done";
        let body = format!(
            "Session {} · {}s · ${:.4}",
            session_id, elapsed_secs, cost_usd
        );
        use notify_rust::Notification;
        let _ = Notification::new().summary(summary).body(&body).show();
        let _ = session_id; // suppress unused warning when log line below is removed
        let _ = cost_usd;
    }

    // When desktop-notify is off, suppress unused-variable warnings.
    #[cfg(not(feature = "desktop-notify"))]
    {
        let _ = session_id;
        let _ = cost_usd;
    }
}

/// Returns true if a notification should fire for the given elapsed time.
/// Extracted for testability without needing the `desktop-notify` feature.
pub fn should_notify(elapsed_secs: u64) -> bool {
    elapsed_secs >= MIN_ELAPSED_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that elapsed < MIN_ELAPSED_SECS returns early without panicking.
    /// We can't directly assert "no notification was sent" in a unit test without
    /// mocking the OS layer, but we can confirm the gate logic works by checking
    /// that `should_notify` returns false for short tasks.
    #[test]
    fn test_min_duration_gate_suppresses_short_task() {
        assert!(!should_notify(9));
        assert!(!should_notify(0));
    }

    #[test]
    fn test_min_duration_gate_passes_long_task() {
        assert!(should_notify(10));
        assert!(should_notify(15));
        assert!(should_notify(60));
    }

    #[test]
    fn test_min_duration_gate_boundary() {
        // Exactly at the boundary: 10 seconds should fire.
        assert!(should_notify(MIN_ELAPSED_SECS));
        // One below: should not fire.
        assert!(!should_notify(MIN_ELAPSED_SECS - 1));
    }
}
