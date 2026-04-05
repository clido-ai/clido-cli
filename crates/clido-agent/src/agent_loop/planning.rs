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
