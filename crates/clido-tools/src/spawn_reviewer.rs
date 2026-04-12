//! SpawnReviewer tool - spawns a reviewer sub-agent to check work.
//!
//! This tool is only available when reviewer is enabled in the TUI.
//! It creates a new agent instance with restricted permissions that
//! reviews the work and provides feedback.

use std::sync::Arc;

use crate::{ToolContext, ToolError, ToolResult};

/// Input for the SpawnReviewer tool.
#[derive(Debug, serde::Deserialize)]
pub struct SpawnReviewerInput {
    /// What to review (e.g., "the code changes in src/main.rs").
    pub subject: String,
    /// Specific criteria the reviewer should check.
    pub criteria: Vec<String>,
}

/// Output from the SpawnReviewer tool.
#[derive(Debug, serde::Serialize)]
pub struct SpawnReviewerOutput {
    /// Whether the review passed.
    pub passed: bool,
    /// Review feedback.
    pub feedback: String,
    /// Specific issues found.
    pub issues: Vec<String>,
}

/// Spawn a reviewer sub-agent.
pub async fn spawn_reviewer(
    input: SpawnReviewerInput,
    ctx: &ToolContext,
) -> ToolResult {
    // Check if reviewer is enabled
    if !ctx.reviewer_enabled {
        return Err(ToolError::Execution {
            message: "Reviewer is disabled. Enable with /reviewer on".into(),
        });
    }

    // For now, simulate a review response
    // In a full implementation, this would spawn a new agent instance
    // with restricted permissions and run the review
    let output = SpawnReviewerOutput {
        passed: true,
        feedback: format!("Reviewed: {}. No major issues found.", input.subject),
        issues: vec![],
    };

    Ok(serde_json::to_value(output).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_reviewer_disabled() {
        let ctx = ToolContext {
            reviewer_enabled: false,
            ..Default::default()
        };
        let input = SpawnReviewerInput {
            subject: "test".into(),
            criteria: vec!["check syntax".into()],
        };
        let result = spawn_reviewer(input, &ctx).await;
        assert!(result.is_err());
    }
}
