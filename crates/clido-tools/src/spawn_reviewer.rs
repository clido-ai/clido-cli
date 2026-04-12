//! SpawnReviewer tool - spawns a reviewer sub-agent to check work.
//!
//! This tool is only available when reviewer is enabled in the TUI.
//! It sends a review request to the fast provider (or main provider if no fast provider is configured).

use async_trait::async_trait;
use serde_json::json;

use crate::{Tool, ToolOutput};

/// SpawnReviewer tool implementation.
pub struct SpawnReviewerTool {
    /// Whether reviewer is enabled.
    pub reviewer_enabled: bool,
}

impl SpawnReviewerTool {
    pub fn new(reviewer_enabled: bool) -> Self {
        Self { reviewer_enabled }
    }
}

#[async_trait]
impl Tool for SpawnReviewerTool {
    fn name(&self) -> &str {
        "SpawnReviewer"
    }

    fn description(&self) -> &str {
        "Spawn a reviewer sub-agent to check the quality of work. \
         The reviewer will analyze the changes and provide feedback \
         on correctness, completeness, and potential issues. \
         Only available when reviewer mode is enabled."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "What to review (e.g., 'the code changes in src/main.rs')"
                },
                "criteria": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Specific criteria the reviewer should check"
                }
            },
            "required": ["subject"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        // Check if reviewer is enabled
        if !self.reviewer_enabled {
            return ToolOutput::err(
                "Reviewer is disabled. Enable with /reviewer on".to_string()
            );
        }

        let subject = input.get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or("work");

        let criteria: Vec<String> = input.get("criteria")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect())
            .unwrap_or_default();

        // Build review prompt
        let review_prompt = if criteria.is_empty() {
            format!(
                "Please review the following work and provide feedback on correctness, \
                completeness, and potential issues:\n\n{}\n\n\
                Provide your review in this format:\n\
                - Overall assessment (PASS or NEEDS_IMPROVEMENT)\n\
                - Specific issues found (if any)\n\
                - Recommendations for improvement",
                subject
            )
        } else {
            format!(
                "Please review the following work against these criteria:\n\n\
                Subject: {}\n\n\
                Criteria:\n{}\n\n\
                Provide your review in this format:\n\
                - Overall assessment (PASS or NEEDS_IMPROVEMENT)\n\
                - Specific issues found (if any)\n\
                - Recommendations for improvement",
                subject,
                criteria.iter().map(|c| format!("- {}", c)).collect::<Vec<_>>().join("\n")
            )
        };

        // Return the review prompt as content
        // The actual review will be done by the agent using the fast provider
        let output = json!({
            "review_request": review_prompt,
            "subject": subject,
            "criteria": criteria,
            "note": "The agent should now use the fast provider to get a review for this work"
        });

        ToolOutput::ok(output.to_string())
    }
}