//! Multi-source model metadata fetching.
//!
//! Fetches model metadata from:
//! 1. models.dev API (secondary source)
//! 2. Provider's own /models endpoint (live availability)
//!
//! Results are merged and cached locally.

use crate::model_cache::ModelCache;
use crate::provider::{ModelMetadata, ModelsSnapshot, ProviderModels};
use std::collections::HashMap;
use std::path::Path;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Environment variable to override the models.dev URL.
pub fn models_dev_url() -> String {
    std::env::var("CLIDO_MODELS_DEV_URL").unwrap_or_else(|_| MODELS_DEV_URL.to_string())
}

/// Environment variable to disable remote model fetching.
pub fn disable_models_fetch() -> bool {
    std::env::var("CLIDO_DISABLE_MODELS_FETCH")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Fetches and merges model metadata from multiple sources.
pub struct ModelFetcher {
    cache: ModelCache,
    client: reqwest::Client,
}

impl ModelFetcher {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            cache: ModelCache::new(config_dir),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Load models: cache first, then fetch from APIs if stale.
    /// Returns cached data immediately if fresh, otherwise fetches in background.
    pub async fn load(&self) -> ModelsSnapshot {
        // 1. Return fresh cache if available
        if let Some(fresh) = self.cache.fresh_snapshot() {
            return fresh;
        }

        // 2. Try to fetch from models.dev
        if !disable_models_fetch() {
            if let Ok(snapshot) = self.fetch_models_dev().await {
                let mut snapshot = snapshot;
                ModelCache::stamp_snapshot(&mut snapshot);
                self.cache.write(&snapshot).ok();
                return snapshot;
            }
        }

        // 3. Return stale cache as last resort
        self.cache.read().unwrap_or_default()
    }

    /// Fetch models from models.dev.
    async fn fetch_models_dev(&self) -> Result<ModelsSnapshot, String> {
        let url = models_dev_url();
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("models.dev network error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("models.dev returned {}", resp.status()));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("models.dev parse error: {}", e))?;

        // Parse the models.dev response format
        // Expected: { "providers": { "<id>": { "name", "models": { "<id>": { ... } } } } }
        let mut providers = HashMap::new();

        if let Some(providers_obj) = json.get("providers").and_then(|v| v.as_object()) {
            for (provider_id, provider_data) in providers_obj {
                let name = provider_data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(provider_id)
                    .to_string();

                let base_url = provider_data
                    .get("api")
                    .or_else(|| provider_data.get("base_url"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let api_key_env = provider_data
                    .get("env")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let mut models = HashMap::new();
                if let Some(models_obj) = provider_data.get("models").and_then(|v| v.as_object()) {
                    for (model_id, model_data) in models_obj {
                        let model = parse_models_dev_model(model_id, model_data);
                        models.insert(model_id.clone(), model);
                    }
                }

                providers.insert(
                    provider_id.clone(),
                    ProviderModels {
                        id: provider_id.clone(),
                        name: Some(name),
                        base_url,
                        api_key_env,
                        is_subscription: None,
                        models,
                    },
                );
            }
        }

        Ok(ModelsSnapshot {
            providers,
            fetched_at: None,
        })
    }

    /// Get metadata for a specific provider, merging cache with live data.
    pub async fn get_provider_models(
        &self,
        provider_id: &str,
    ) -> Option<Vec<ModelMetadata>> {
        let snapshot = self.load().await;
        snapshot.providers.get(provider_id).map(|p| {
            p.models.values().cloned().collect()
        })
    }

    /// Enrich a list of live ModelEntry with cached metadata.
    /// Matches by model ID and overlays name, pricing, context, capabilities.
    pub async fn enrich_models(
        &self,
        live_models: Vec<ModelMetadata>,
        provider_id: &str,
    ) -> Vec<ModelMetadata> {
        let snapshot = self.load().await;
        let cached = snapshot
            .providers
            .get(provider_id)
            .map(|p| &p.models);

        live_models
            .into_iter()
            .map(|live| {
                if let Some(cached_model) = cached.and_then(|m| m.get(&live.id)) {
                    ModelMetadata {
                        name: cached_model.name.clone().or(live.name),
                        context_window: cached_model.context_window.or(live.context_window),
                        pricing: cached_model.pricing.clone().or(live.pricing.clone()),
                        capabilities: if live.capabilities.reasoning
                            || live.capabilities.tool_call
                            || live.capabilities.vision
                            || live.capabilities.temperature
                        {
                            live.capabilities
                        } else {
                            cached_model.capabilities.clone()
                        },
                        status: cached_model.status.clone(),
                        release_date: cached_model.release_date.clone().or(live.release_date),
                        ..live
                    }
                } else {
                    live
                }
            })
            .collect()
    }

    /// Background refresh — fetches and caches without blocking.
    pub async fn refresh(&self) {
        if disable_models_fetch() {
            return;
        }
        if let Ok(mut snapshot) = self.fetch_models_dev().await {
            ModelCache::stamp_snapshot(&mut snapshot);
            self.cache.write(&snapshot).ok();
        }
    }
}

/// Parse a single model entry from models.dev format.
fn parse_models_dev_model(model_id: &str, data: &serde_json::Value) -> ModelMetadata {
    let name = data
        .get("name")
        .and_then(|v| v.as_str())
        .map(String::from);

    let context_window = data
        .get("limit")
        .and_then(|v| v.get("context"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    let pricing = data.get("cost").map(|cost| {
        crate::provider::ModelPricing {
            input_per_mtok: cost.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0),
            output_per_mtok: cost.get("output").and_then(|v| v.as_f64()).unwrap_or(0.0),
            cache_read: cost.get("cache_read").and_then(|v| v.as_f64()),
            cache_write: cost.get("cache_write").and_then(|v| v.as_f64()),
        }
    });

    let capabilities = crate::provider::ModelCapabilities {
        reasoning: data.get("reasoning").and_then(|v| v.as_bool()).unwrap_or(false),
        tool_call: data.get("tool_call").and_then(|v| v.as_bool()).unwrap_or(true),
        vision: data
            .get("modalities")
            .and_then(|v| v.get("input"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_str() == Some("image")))
            .unwrap_or(false),
        temperature: data.get("temperature").and_then(|v| v.as_bool()).unwrap_or(false),
    };

    let status = data
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "beta" => crate::provider::ModelStatus::Beta,
            "deprecated" => crate::provider::ModelStatus::Deprecated,
            _ => crate::provider::ModelStatus::Active,
        })
        .unwrap_or_default();

    let release_date = data
        .get("release_date")
        .and_then(|v| v.as_str())
        .map(String::from);

    ModelMetadata {
        id: model_id.to_string(),
        name,
        context_window,
        pricing,
        capabilities,
        status,
        release_date,
        available: true,
    }
}
