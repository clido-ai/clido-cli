//! Load pricing.toml and compute cost from Usage.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::Usage;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModelPricingEntry {
    pub name: String,
    pub provider: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_creation_per_mtok: Option<f64>,
    #[serde(default)]
    pub cache_read_per_mtok: Option<f64>,
    #[serde(default)]
    pub context_window: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricingToml {
    #[serde(default)]
    pub model: HashMap<String, ModelPricingEntry>,
}

/// Pricing table keyed by model id.
#[derive(Debug, Clone, Default)]
pub struct PricingTable {
    pub models: HashMap<String, ModelPricingEntry>,
}

const STALENESS_DAYS: u64 = 90;

fn pricing_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "clido").map(|d| d.config_dir().join("pricing.toml"))
}

/// Load pricing from config dir. If file is older than 90 days, log a warning.
/// Returns default (empty) table if file absent; uses default per-model cost when model not in table.
pub fn load_pricing() -> (PricingTable, Option<std::path::PathBuf>) {
    let path = match pricing_path() {
        Some(p) => p,
        None => return (PricingTable::default(), None),
    };
    if !path.exists() {
        return (PricingTable::default(), None);
    }
    let table = match std::fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<PricingToml>(&s) {
            Ok(t) => PricingTable { models: t.model },
            Err(_) => PricingTable::default(),
        },
        Err(_) => PricingTable::default(),
    };

    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            if let (Ok(now_dur), Ok(mod_dur)) = (
                SystemTime::now().duration_since(UNIX_EPOCH),
                modified.duration_since(UNIX_EPOCH),
            ) {
                let age_secs = now_dur.as_secs().saturating_sub(mod_dur.as_secs());
                if age_secs > STALENESS_DAYS * 86400 {
                    tracing::warn!(
                        "pricing.toml is older than {} days; consider running clido update-pricing (or doctor)",
                        STALENESS_DAYS
                    );
                }
            }
        }
    }

    (table, Some(path))
}

/// Compute cost in USD for the given usage and model. Uses pricing table if model found; else fallback defaults.
pub fn compute_cost_usd(usage: &Usage, model_id: &str, table: &PricingTable) -> f64 {
    let (input_per_mtok, output_per_mtok, cache_creation, cache_read) = table
        .models
        .get(model_id)
        .map(|e| {
            (
                e.input_per_mtok,
                e.output_per_mtok,
                e.cache_creation_per_mtok.unwrap_or(e.input_per_mtok * 1.25),
                e.cache_read_per_mtok.unwrap_or(e.input_per_mtok * 0.10),
            )
        })
        .unwrap_or((3.0, 15.0, 3.75, 0.3)); // fallback USD per million tokens

    let input = (usage.input_tokens as f64 / 1_000_000.0) * input_per_mtok;
    let output = (usage.output_tokens as f64 / 1_000_000.0) * output_per_mtok;
    let cache_creation_cost = usage
        .cache_creation_input_tokens
        .map(|t| (t as f64 / 1_000_000.0) * cache_creation)
        .unwrap_or(0.0);
    let cache_read_cost = usage
        .cache_read_input_tokens
        .map(|t| (t as f64 / 1_000_000.0) * cache_read)
        .unwrap_or(0.0);

    input + output + cache_creation_cost + cache_read_cost
}
