//! Compute session cost from Usage and model pricing metadata.
//!
//! Pricing comes from ModelMetadata (fetched from models.dev or provider APIs).
//! No pricing.toml file is used.

use crate::types::Usage;

/// Compute cost in USD for the given usage and model pricing.
/// If pricing is unavailable (None), returns 0.0 — no fake rates are used.
pub fn compute_cost_usd(usage: &Usage, pricing: Option<&ModelPricingRef>) -> f64 {
    let (input_per_mtok, output_per_mtok, cache_creation, cache_read) = match pricing {
        Some(p) => (
            Some(p.input_per_mtok),
            Some(p.output_per_mtok),
            p.cache_write,
            p.cache_read,
        ),
        None => (None, None, None, None),
    };

    let input = input_per_mtok
        .map(|r| (usage.input_tokens as f64 / 1_000_000.0) * r)
        .unwrap_or(0.0);
    let output = output_per_mtok
        .map(|r| (usage.output_tokens as f64 / 1_000_000.0) * r)
        .unwrap_or(0.0);
    let cache_creation_cost = usage
        .cache_creation_input_tokens
        .and_then(|t| cache_creation.map(|r| (t as f64 / 1_000_000.0) * r))
        .unwrap_or(0.0);
    let cache_read_cost = usage
        .cache_read_input_tokens
        .and_then(|t| cache_read.map(|r| (t as f64 / 1_000_000.0) * r))
        .unwrap_or(0.0);

    input + output + cache_creation_cost + cache_read_cost
}

/// Legacy stub — pricing.toml has been removed. Always returns an empty table.
/// Kept for backward compatibility with agent loop signatures.
#[derive(Debug, Clone, Default)]
pub struct PricingTable;

impl PricingTable {
    pub fn models(&self) -> std::collections::HashMap<&str, &ModelPricingRef> {
        std::collections::HashMap::new()
    }
}

/// Reference to model pricing data, owned to avoid lifetime issues.
#[derive(Debug, Clone)]
pub struct ModelPricingRef {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    fn make_usage_with_cache(input: u64, output: u64, create: u64, read: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: Some(create),
            cache_read_input_tokens: Some(read),
        }
    }

    #[test]
    fn cost_no_pricing_returns_zero() {
        // When pricing data is unavailable, cost should be 0.0 (no fake rates).
        let usage = make_usage(1_000_000, 1_000_000);
        assert_eq!(compute_cost_usd(&usage, None), 0.0);
    }

    #[test]
    fn cost_zero_tokens() {
        let usage = make_usage(0, 0);
        assert_eq!(compute_cost_usd(&usage, None), 0.0);
    }

    #[test]
    fn cost_from_pricing_ref() {
        let pricing = ModelPricingRef {
            input_per_mtok: 0.25,
            output_per_mtok: 1.25,
            cache_read: None,
            cache_write: None,
        };
        let usage = make_usage(1_000_000, 1_000_000);
        let cost = compute_cost_usd(&usage, Some(&pricing));
        assert!((cost - 1.50).abs() < 0.001, "expected $1.50, got {}", cost);
    }

    #[test]
    fn cost_no_pricing_cache_tokens_also_zero() {
        // Without pricing data, cache tokens also contribute $0.
        let usage = make_usage_with_cache(0, 0, 1_000_000, 1_000_000);
        assert_eq!(compute_cost_usd(&usage, None), 0.0);
    }

    #[test]
    fn cost_with_explicit_cache_rates() {
        let pricing = ModelPricingRef {
            input_per_mtok: 10.0,
            output_per_mtok: 30.0,
            cache_read: Some(1.0),
            cache_write: Some(5.0),
        };
        let usage = make_usage_with_cache(0, 0, 2_000_000, 500_000);
        let cost = compute_cost_usd(&usage, Some(&pricing));
        let expected = 2.0 * 5.0 + 0.5 * 1.0;
        assert!(
            (cost - expected).abs() < 0.001,
            "expected {}, got {}",
            expected,
            cost
        );
    }
}
