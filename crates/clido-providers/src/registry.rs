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
            ("HTTP-Referer", "https://github.com/clido"),
            ("X-Title", "Clido"),
        ],
        is_local: false,
        is_anthropic: false,
    },
    ProviderDef {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude models — console.anthropic.com",
        base_url: "https://api.anthropic.com",
        api_key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-sonnet-4-5",
        extra_headers: &[],
        is_local: false,
        is_anthropic: true,
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
    },
    ProviderDef {
        id: "minimax",
        name: "MiniMax",
        description: "MiniMax-M2.7 coding model — minimax.io",
        base_url: "https://api.minimax.io/v1",
        api_key_env: "MINIMAX_API_KEY",
        default_model: "MiniMax-M1",
        extra_headers: &[],
        is_local: false,
        is_anthropic: false,
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
    },
];
