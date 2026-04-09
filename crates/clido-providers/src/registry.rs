//! Central registry of all supported AI model providers.
//!
//! Adding a new OpenAI-compatible provider requires only a single `ProviderDef`
//! entry in [`PROVIDER_REGISTRY`]. All other parts of the codebase derive their
//! information (env-var names, base URLs, display names, etc.) from this slice.

/// Metadata for a single AI model provider.
pub struct ProviderDef {
    /// Short machine-readable identifier used in config files (e.g. `"groq"`).
    pub id: &'static str,
    /// Human-readable display name (e.g. `"Groq"`).
    pub name: &'static str,
    /// One-line description shown in setup UI (e.g. `"Fast inference — groq.com"`).
    pub description: &'static str,
    /// API base URL. For local/alibabacloud providers this is the default that
    /// can be overridden at runtime via `base_url` in config.
    pub base_url: &'static str,
    /// Conventional environment variable that holds the API key. Empty string
    /// for local providers that don't need a key.
    pub api_key_env: &'static str,
    /// Sensible default model ID for first-run setup.
    pub default_model: &'static str,
    /// Extra HTTP headers required by this provider (e.g. OpenRouter referrer).
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// True for providers that run locally and need no API key (e.g. Ollama).
    pub is_local: bool,
    /// True for the Anthropic provider, which uses its own SDK rather than the
    /// OpenAI-compatible `OpenAICompatProvider`.
    pub is_anthropic: bool,
    /// True for providers billed via a flat subscription rather than per-token.
    /// Budget tracking is not applicable for these providers.
    pub is_subscription: bool,
    /// Hardcoded fallback model IDs shown in the picker when the live API
    /// fetch returns no models (e.g. plan-specific base URL not yet set).
    pub fallback_models: &'static [&'static str],
    /// Context window sizes (in thousands of tokens) for a subset of fallback
    /// models where the value is known. Entries not listed here get `context_k: None`.
    /// Format: `&[("model-id", context_k_u32)]`.
    pub fallback_model_context_k: &'static [(&'static str, u32)],
    /// Whether this provider requires a custom base URL that varies per user/plan.
    /// When true, the creation wizard prompts for base_url before the model fetch.
    pub needs_base_url: bool,
}

/// Check whether a provider (by id) uses subscription billing.
pub fn is_subscription_provider(provider_id: &str) -> bool {
    PROVIDER_REGISTRY
        .iter()
        .find(|d| d.id == provider_id)
        .map(|d| d.is_subscription)
        .unwrap_or(false)
}

/// All supported providers in the canonical display order.
///
/// The index of each entry is stable — user configs reference providers by
/// their string `id`, but the setup wizard uses integer indices that must not
/// shift between releases.
pub static PROVIDER_REGISTRY: &[ProviderDef] = &[
    ProviderDef {
        id: "openrouter",
        name: "OpenRouter",
        description: "access any model — openrouter.ai",
        base_url: "https://openrouter.ai/api/v1",
        api_key_env: "OPENROUTER_API_KEY",
        default_model: "anthropic/claude-sonnet-4-5",
        extra_headers: &[
            ("HTTP-Referer", "https://github.com/clido-ai/clido-cli"),
            ("X-Title", "Clido"),
        ],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude models — console.anthropic.com",
        base_url: "https://api.anthropic.com",
        api_key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-sonnet-4-6",
        extra_headers: &[],
        is_local: false,
        is_anthropic: true,
        is_subscription: false,
        fallback_models: &[
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "openai",
        name: "OpenAI",
        description: "GPT & o-series — platform.openai.com",
        base_url: "https://api.openai.com/v1",
        api_key_env: "OPENAI_API_KEY",
        default_model: "gpt-4o",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["gpt-4o", "gpt-4o-mini", "o1", "o3-mini"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "mistral",
        name: "Mistral",
        description: "Mistral models — console.mistral.ai",
        base_url: "https://api.mistral.ai/v1",
        api_key_env: "MISTRAL_API_KEY",
        default_model: "mistral-large-latest",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "mistral-large-latest",
            "mistral-small-latest",
            "codestral-latest",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "minimax",
        name: "MiniMax",
        description: "MiniMax models — minimax.io",
        base_url: "https://api.minimax.io/v1",
        api_key_env: "MINIMAX_API_KEY",
        default_model: "MiniMax-M1",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["MiniMax-M1", "MiniMax-Text-01"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "kimi",
        name: "Kimi",
        description: "Moonshot AI models — platform.moonshot.ai",
        base_url: "https://api.moonshot.ai/v1",
        api_key_env: "MOONSHOT_API_KEY",
        default_model: "moonshot-v1-8k",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["moonshot-v1-8k", "moonshot-v1-32k", "moonshot-v1-128k"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "kimi-code",
        name: "Kimi Code",
        description: "coding model — api.kimi.com",
        base_url: "https://api.kimi.com/coding/v1",
        api_key_env: "KIMI_CODE_API_KEY",
        default_model: "kimi-for-coding",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: true,
        fallback_models: &["kimi-for-coding"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "alibabacloud",
        name: "Alibaba Cloud",
        description: "Qwen models — dashscope.aliyuncs.com",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        api_key_env: "DASHSCOPE_API_KEY",
        default_model: "qwen-max",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "qwen-max",
            "qwen-plus",
            "qwen-turbo",
            "qwen2.5-72b-instruct",
            "qwen2.5-coder-32b-instruct",
            "qwq-32b",
        ],
        fallback_model_context_k: &[],
        needs_base_url: true,
    },
    ProviderDef {
        id: "alibabacloud-code",
        name: "Alibaba Cloud  (coding plan)",
        description: "Multi-vendor coding models — coding-intl.dashscope.aliyuncs.com",
        base_url: "https://coding-intl.dashscope.aliyuncs.com/v1",
        api_key_env: "DASHSCOPE_CODE_API_KEY",
        default_model: "qwen3.6-plus",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: true,
        fallback_models: &[
            "qwen3.6-plus",
            "qwen3.5-plus",
            "qwen3-max-2026-01-23",
            "qwen3-coder-next",
            "qwen3-coder-plus",
            "glm-5",
            "glm-5.1",
            "glm-4.7",
            "kimi-k2.5",
            "MiniMax-M2.5",
        ],
        fallback_model_context_k: &[
            ("qwen3.6-plus", 1000),
            ("qwen3.5-plus", 128),
            ("qwen3-max-2026-01-23", 128),
            ("qwen3-coder-next", 128),
            ("qwen3-coder-plus", 128),
            ("glm-5", 128),
            ("glm-5.1", 128),
            ("glm-4.7", 128),
            ("kimi-k2.5", 128),
            ("MiniMax-M2.5", 1000),
        ],
        needs_base_url: false,
    },
    ProviderDef {
        id: "deepseek",
        name: "DeepSeek",
        description: "DeepSeek models — api.deepseek.com",
        base_url: "https://api.deepseek.com/v1",
        api_key_env: "DEEPSEEK_API_KEY",
        default_model: "deepseek-chat",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["deepseek-chat", "deepseek-reasoner"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "groq",
        name: "Groq",
        description: "Fast inference — groq.com",
        base_url: "https://api.groq.com/openai/v1",
        api_key_env: "GROQ_API_KEY",
        default_model: "llama-3.3-70b-versatile",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "gemma2-9b-it",
            "mixtral-8x7b-32768",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "cerebras",
        name: "Cerebras",
        description: "Fast inference — cerebras.ai",
        base_url: "https://api.cerebras.ai/v1",
        api_key_env: "CEREBRAS_API_KEY",
        default_model: "llama3.1-70b",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["llama3.1-70b", "llama3.1-8b"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "togetherai",
        name: "Together AI",
        description: "Open models — together.xyz",
        base_url: "https://api.together.xyz/v1",
        api_key_env: "TOGETHER_API_KEY",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo",
            "deepseek-ai/DeepSeek-R1",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "fireworks",
        name: "Fireworks AI",
        description: "Fast open models — fireworks.ai",
        base_url: "https://api.fireworks.ai/inference/v1",
        api_key_env: "FIREWORKS_API_KEY",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "accounts/fireworks/models/llama-v3p3-70b-instruct",
            "accounts/fireworks/models/deepseek-r1",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "xai",
        name: "xAI (Grok)",
        description: "Grok models — x.ai",
        base_url: "https://api.x.ai/v1",
        api_key_env: "XAI_API_KEY",
        default_model: "grok-3-beta",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["grok-3-beta", "grok-3-mini-beta", "grok-2-1212"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "perplexity",
        name: "Perplexity",
        description: "Sonar models — perplexity.ai",
        base_url: "https://api.perplexity.ai",
        api_key_env: "PERPLEXITY_API_KEY",
        default_model: "sonar-pro",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &["sonar-pro", "sonar", "sonar-reasoning-pro"],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "gemini",
        name: "Google Gemini",
        description: "Gemini models — gemini.google.com",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        api_key_env: "GEMINI_API_KEY",
        default_model: "gemini-2.5-flash",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[
            "gemini-2.5-pro-preview-06-05",
            "gemini-2.5-flash",
            "gemini-2.0-flash",
        ],
        fallback_model_context_k: &[],
        needs_base_url: false,
    },
    ProviderDef {
        id: "local",
        name: "Local / Ollama",
        description: "no key needed, runs on your machine",
        base_url: "http://localhost:11434/v1",
        api_key_env: "",
        default_model: "llama3.2",
        extra_headers: &[],
        is_local: true,
        is_anthropic: false,
        is_subscription: false,
        fallback_models: &[],
        fallback_model_context_k: &[],
        needs_base_url: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    // ── Registry invariants ───────────────────────────────────────────

    #[test]
    fn registry_is_not_empty() {
        assert!(
            !PROVIDER_REGISTRY.is_empty(),
            "PROVIDER_REGISTRY must contain at least one provider"
        );
    }

    #[test]
    fn all_provider_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PROVIDER_REGISTRY {
            assert!(seen.insert(p.id), "duplicate provider id: {}", p.id);
        }
    }

    #[test]
    fn all_providers_have_non_empty_id_and_name() {
        for p in PROVIDER_REGISTRY {
            assert!(!p.id.is_empty(), "provider id must not be empty");
            assert!(
                !p.name.is_empty(),
                "provider name must not be empty for {}",
                p.id
            );
        }
    }

    #[test]
    fn all_providers_have_non_empty_base_url() {
        for p in PROVIDER_REGISTRY {
            assert!(
                !p.base_url.is_empty(),
                "base_url must not be empty for {}",
                p.id
            );
        }
    }

    #[test]
    fn all_providers_have_non_empty_default_model() {
        for p in PROVIDER_REGISTRY {
            assert!(
                !p.default_model.is_empty(),
                "default_model must not be empty for {}",
                p.id
            );
        }
    }

    // ── is_subscription flag ──────────────────────────────────────────

    #[test]
    fn kimi_code_is_subscription() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "kimi-code")
            .unwrap();
        assert!(
            p.is_subscription,
            "kimi-code should be a subscription provider"
        );
    }

    #[test]
    fn non_subscription_providers() {
        let expected_non_sub = [
            "openrouter",
            "anthropic",
            "openai",
            "mistral",
            "minimax",
            "kimi",
            "alibabacloud",
            "deepseek",
            "groq",
            "cerebras",
            "togetherai",
            "fireworks",
            "xai",
            "perplexity",
            "gemini",
            "local",
        ];
        for id in expected_non_sub {
            let p = PROVIDER_REGISTRY
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("provider {} not found", id));
            assert!(
                !p.is_subscription,
                "{} should NOT be a subscription provider",
                id
            );
        }
    }

    // ── is_subscription_provider() helper ─────────────────────────────

    #[test]
    fn is_subscription_provider_returns_true_for_kimi_code() {
        assert!(is_subscription_provider("kimi-code"));
    }

    #[test]
    fn is_subscription_provider_returns_false_for_openai() {
        assert!(!is_subscription_provider("openai"));
    }

    #[test]
    fn is_subscription_provider_returns_false_for_unknown() {
        assert!(!is_subscription_provider("nonexistent-provider"));
    }

    #[test]
    fn is_subscription_provider_returns_false_for_empty_string() {
        assert!(!is_subscription_provider(""));
    }

    // ── api_key_env per provider ──────────────────────────────────────

    #[test]
    fn api_key_env_openrouter() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "openrouter")
            .unwrap();
        assert_eq!(p.api_key_env, "OPENROUTER_API_KEY");
    }

    #[test]
    fn api_key_env_anthropic() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "anthropic")
            .unwrap();
        assert_eq!(p.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn api_key_env_openai() {
        let p = PROVIDER_REGISTRY.iter().find(|p| p.id == "openai").unwrap();
        assert_eq!(p.api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn api_key_env_deepseek() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "deepseek")
            .unwrap();
        assert_eq!(p.api_key_env, "DEEPSEEK_API_KEY");
    }

    #[test]
    fn api_key_env_gemini() {
        let p = PROVIDER_REGISTRY.iter().find(|p| p.id == "gemini").unwrap();
        assert_eq!(p.api_key_env, "GEMINI_API_KEY");
    }

    #[test]
    fn api_key_env_groq() {
        let p = PROVIDER_REGISTRY.iter().find(|p| p.id == "groq").unwrap();
        assert_eq!(p.api_key_env, "GROQ_API_KEY");
    }

    #[test]
    fn api_key_env_local_is_empty() {
        let p = PROVIDER_REGISTRY.iter().find(|p| p.id == "local").unwrap();
        assert_eq!(
            p.api_key_env, "",
            "local provider should have no API key env var"
        );
    }

    #[test]
    fn api_key_env_xai() {
        let p = PROVIDER_REGISTRY.iter().find(|p| p.id == "xai").unwrap();
        assert_eq!(p.api_key_env, "XAI_API_KEY");
    }

    #[test]
    fn api_key_env_mistral() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "mistral")
            .unwrap();
        assert_eq!(p.api_key_env, "MISTRAL_API_KEY");
    }

    #[test]
    fn api_key_env_alibabacloud() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "alibabacloud")
            .unwrap();
        assert_eq!(p.api_key_env, "DASHSCOPE_API_KEY");
    }

    // ── is_local / is_anthropic flags ─────────────────────────────────

    #[test]
    fn only_local_provider_is_local() {
        for p in PROVIDER_REGISTRY {
            if p.id == "local" {
                assert!(p.is_local, "local provider should have is_local=true");
            } else {
                assert!(!p.is_local, "{} should not have is_local=true", p.id);
            }
        }
    }

    #[test]
    fn only_anthropic_provider_is_anthropic() {
        for p in PROVIDER_REGISTRY {
            if p.id == "anthropic" {
                assert!(p.is_anthropic, "anthropic should have is_anthropic=true");
            } else {
                assert!(
                    !p.is_anthropic,
                    "{} should not have is_anthropic=true",
                    p.id
                );
            }
        }
    }

    // ── Non-local providers must have an API key env var ──────────────

    #[test]
    fn non_local_providers_have_api_key_env() {
        for p in PROVIDER_REGISTRY {
            if !p.is_local {
                assert!(
                    !p.api_key_env.is_empty(),
                    "non-local provider {} must have an api_key_env",
                    p.id
                );
            }
        }
    }

    // ── Extra headers ─────────────────────────────────────────────────

    #[test]
    fn openrouter_has_extra_headers() {
        let p = PROVIDER_REGISTRY
            .iter()
            .find(|p| p.id == "openrouter")
            .unwrap();
        assert!(
            !p.extra_headers.is_empty(),
            "openrouter should have extra headers"
        );
        let keys: Vec<&str> = p.extra_headers.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"HTTP-Referer"));
        assert!(keys.contains(&"X-Title"));
    }

    #[test]
    fn most_providers_have_no_extra_headers() {
        let with_headers: Vec<&str> = PROVIDER_REGISTRY
            .iter()
            .filter(|p| !p.extra_headers.is_empty())
            .map(|p| p.id)
            .collect();
        // Only openrouter should have extra headers
        assert_eq!(with_headers, vec!["openrouter"]);
    }
}
