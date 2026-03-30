//! Per-provider system prompt suffixes.
//!
//! Different LLM providers respond best to different prompt styles.
//! Rather than maintaining completely separate prompts, we append a
//! small provider-specific suffix to the shared base system prompt.

/// Broad provider family used to select prompt style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFamily {
    Anthropic,
    OpenAI,
    Generic,
}

impl ProviderFamily {
    /// Detect the family from a provider identifier (e.g. `"anthropic"`, `"openai"`).
    pub fn from_provider_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("anthropic") || lower.contains("claude") {
            Self::Anthropic
        } else if lower.contains("openai") || lower.contains("gpt") {
            Self::OpenAI
        } else {
            Self::Generic
        }
    }

    /// Detect the family from a model identifier (e.g. `"claude-sonnet-4-5"`, `"gpt-4.1"`).
    pub fn from_model_name(model: &str) -> Self {
        let lower = model.to_lowercase();
        if lower.contains("claude") {
            Self::Anthropic
        } else if lower.contains("gpt")
            || lower.starts_with("o1")
            || lower.starts_with("o3")
            || lower.starts_with("o4")
        {
            Self::OpenAI
        } else {
            Self::Generic
        }
    }

    /// Detect the family by checking the provider name first, then falling
    /// back to model name heuristics.
    pub fn detect(provider: &str, model: &str) -> Self {
        let by_provider = Self::from_provider_name(provider);
        if by_provider != Self::Generic {
            return by_provider;
        }
        Self::from_model_name(model)
    }
}

/// Return provider-specific instructions to append to the system prompt.
///
/// These are stylistic hints that help the model respond in its strongest
/// format. Returns an empty string for [`ProviderFamily::Generic`].
pub fn provider_specific_instructions(family: ProviderFamily) -> &'static str {
    match family {
        ProviderFamily::Anthropic => {
            "\n\n\
## Response Style (Anthropic-optimized)\n\
- Use XML tags for structured output when appropriate (e.g., <analysis>, <plan>, <code>)\n\
- Think step-by-step using <thinking> tags when facing complex problems\n\
- Be direct and avoid unnecessary preamble"
        }
        ProviderFamily::OpenAI => {
            "\n\n\
## Response Style (OpenAI-optimized)\n\
- Use markdown formatting for structured output\n\
- For complex problems, break down your approach before implementing\n\
- Be concise and action-oriented"
        }
        ProviderFamily::Generic => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_anthropic_by_provider() {
        assert_eq!(
            ProviderFamily::from_provider_name("anthropic"),
            ProviderFamily::Anthropic,
        );
        assert_eq!(
            ProviderFamily::from_provider_name("Anthropic"),
            ProviderFamily::Anthropic,
        );
    }

    #[test]
    fn detect_openai_by_provider() {
        assert_eq!(
            ProviderFamily::from_provider_name("openai"),
            ProviderFamily::OpenAI,
        );
    }

    #[test]
    fn detect_anthropic_by_model() {
        assert_eq!(
            ProviderFamily::from_model_name("claude-sonnet-4-5"),
            ProviderFamily::Anthropic,
        );
        assert_eq!(
            ProviderFamily::from_model_name("claude-3-opus-20240229"),
            ProviderFamily::Anthropic,
        );
    }

    #[test]
    fn detect_openai_by_model() {
        assert_eq!(
            ProviderFamily::from_model_name("gpt-4.1"),
            ProviderFamily::OpenAI,
        );
        assert_eq!(
            ProviderFamily::from_model_name("o3-mini"),
            ProviderFamily::OpenAI,
        );
        assert_eq!(
            ProviderFamily::from_model_name("o4-mini"),
            ProviderFamily::OpenAI,
        );
    }

    #[test]
    fn detect_generic_fallback() {
        assert_eq!(
            ProviderFamily::from_model_name("gemini-pro"),
            ProviderFamily::Generic,
        );
        assert_eq!(
            ProviderFamily::from_provider_name("groq"),
            ProviderFamily::Generic,
        );
    }

    #[test]
    fn detect_prefers_provider_over_model() {
        // Provider says anthropic → Anthropic even if model looks like openai
        assert_eq!(
            ProviderFamily::detect("anthropic", "gpt-4"),
            ProviderFamily::Anthropic,
        );
    }

    #[test]
    fn detect_falls_back_to_model() {
        assert_eq!(
            ProviderFamily::detect("openrouter", "claude-sonnet-4-5"),
            ProviderFamily::Anthropic,
        );
    }

    #[test]
    fn generic_suffix_is_empty() {
        assert!(provider_specific_instructions(ProviderFamily::Generic).is_empty());
    }

    #[test]
    fn anthropic_suffix_is_nonempty() {
        let s = provider_specific_instructions(ProviderFamily::Anthropic);
        assert!(s.contains("Anthropic-optimized"));
        assert!(s.contains("XML tags"));
    }

    #[test]
    fn openai_suffix_is_nonempty() {
        let s = provider_specific_instructions(ProviderFamily::OpenAI);
        assert!(s.contains("OpenAI-optimized"));
        assert!(s.contains("markdown"));
    }
}
