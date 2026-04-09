//! Architect→Editor planning pipeline.

use clido_core::{AgentConfig, ContentBlock, Message, Role};
use clido_providers::ModelProvider;
use tracing::{debug, warn};

use crate::prompts::architect_user_prompt;

/// Use the utility provider to generate a plan for complex prompts.
/// Returns None if the prompt is too simple or planning fails.
pub(crate) async fn architect_plan(
    user_input: &str,
    config: &AgentConfig,
    provider: &dyn ModelProvider,
) -> Option<String> {
    // Only invoke architect for non-trivial prompts (>50 chars, not simple questions)
    if user_input.len() < 50 {
        return None;
    }
    let lower = user_input.to_lowercase();
    // Skip for simple queries that don't need planning
    if lower.starts_with("what ")
        || lower.starts_with("how ")
        || lower.starts_with("why ")
        || lower.starts_with("explain ")
        || lower.starts_with("show ")
    {
        return None;
    }

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: architect_user_prompt(user_input),
        }],
    }];

    match provider.complete(&messages, &[], config).await {
        Ok(response) => {
            let plan = response
                .content
                .iter()
                .find_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            if plan.is_empty() {
                return None;
            }

            debug!(
                "Architect plan generated ({} chars, model={})",
                plan.len(),
                config.model
            );
            Some(plan)
        }
        Err(e) => {
            warn!(
                "Architect planning failed (falling back to direct execution): {}",
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clido_core::{
        AgentConfig, ContentBlock, Message, ModelResponse, PermissionMode, Result, StopReason,
        ToolSchema, Usage,
    };
    use clido_providers::{ModelEntry, ModelProvider};
    use futures::Stream;
    use std::pin::Pin;

    fn test_cfg() -> AgentConfig {
        AgentConfig {
            model: "test-model".into(),
            permission_mode: PermissionMode::AcceptAll,
            ..Default::default()
        }
    }

    struct FixedResponseProvider {
        text: String,
        err: bool,
    }

    #[async_trait]
    impl ModelProvider for FixedResponseProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> Result<ModelResponse> {
            if self.err {
                Err(anyhow::anyhow!("simulated planner failure").into())
            } else {
                Ok(ModelResponse {
                    id: "1".into(),
                    model: "m".into(),
                    content: vec![ContentBlock::Text {
                        text: self.text.clone(),
                    }],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
            }
        }

        async fn complete_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolSchema],
            _config: &AgentConfig,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<clido_providers::StreamEvent>> + Send>>>
        {
            unimplemented!()
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelEntry>, String> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn architect_skips_short_prompts() {
        let p = FixedResponseProvider {
            text: "plan".into(),
            err: false,
        };
        assert!(architect_plan("short", &test_cfg(), &p).await.is_none());
    }

    #[tokio::test]
    async fn architect_skips_simple_question_prefixes() {
        let p = FixedResponseProvider {
            text: "x".into(),
            err: false,
        };
        let cfg = test_cfg();
        for prefix in ["what ", "how ", "why ", "explain ", "show "] {
            let body = format!("{prefix}{}", "y".repeat(60));
            assert!(
                architect_plan(&body, &cfg, &p).await.is_none(),
                "prefix {prefix:?}"
            );
        }
    }

    #[tokio::test]
    async fn architect_returns_plan_when_model_replies() {
        let p = FixedResponseProvider {
            text: "STEP 1 — do the thing".into(),
            err: false,
        };
        let body = "x".repeat(60);
        let out = architect_plan(&body, &test_cfg(), &p).await;
        assert_eq!(out.as_deref(), Some("STEP 1 — do the thing"));
    }

    #[tokio::test]
    async fn architect_none_when_plan_text_empty() {
        let p = FixedResponseProvider {
            text: "".into(),
            err: false,
        };
        let body = "x".repeat(60);
        assert!(architect_plan(&body, &test_cfg(), &p).await.is_none());
    }

    #[tokio::test]
    async fn architect_none_on_provider_error() {
        let p = FixedResponseProvider {
            text: "ignored".into(),
            err: true,
        };
        let body = "x".repeat(60);
        assert!(architect_plan(&body, &test_cfg(), &p).await.is_none());
    }
}
