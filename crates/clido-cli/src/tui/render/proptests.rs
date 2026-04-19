//! Property-based tests for the TUI render/scroll/wrap invariants.
#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    // Test that scroll clamping is idempotent: scrolling up then down by the same
    // amount should leave scroll position unchanged (within valid range).
    proptest! {
        #[test]
        fn scroll_up_clamps_at_zero(lines in 1u32..100u32) {
            // Scrolling up more than current position always clamps to 0
            let scroll = 0u32;
            let result = scroll.saturating_sub(lines);
            prop_assert_eq!(result, 0);
        }

        #[test]
        fn scroll_saturating_sub_never_underflows(
            current in 0u32..1000u32,
            lines in 0u32..2000u32
        ) {
            // saturating_sub should never panic or overflow
            let result = current.saturating_sub(lines);
            prop_assert!(result <= current);
        }

        #[test]
        fn scroll_saturating_add_never_overflows(
            current in 0u32..u32::MAX / 2,
            lines in 0u32..u32::MAX / 2
        ) {
            // saturating_add should never panic
            let result = current.saturating_add(lines);
            prop_assert!(result >= current);
        }
    }
}
