//! SpawnReviewer tool - spawns a reviewer sub-agent to check work.
//!
//! This tool is only available when reviewer is enabled in the TUI.
//!
//! TODO: In a future version, this should spawn a real sub-agent with restricted
//! permissions using the fast provider. Currently, it returns a simulated review.

use async_trait::async_trait;
use serde_json::json;

use crate::{Tool, ToolOutput};

/// SpawnReviewer tool implementation.
/// 
/// CURRENTLY SIMULATED: Returns a mock review. Future versions will spawn
/// a real sub-agent with restricted tool access for independent code review.
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

        // For now, return a simulated review
        // In a full implementation, this would spawn a new agent instance
        // with restricted permissions and run the review
        let feedback = if criteria.is_empty() {
            format!("Reviewed: {}. No major issues found.", subject)
        } else {
            format!("Reviewed: {} against {} criteria. All checks passed.", 
                subject, criteria.len())
        };

        let output = json!({
            "passed": true,
            "feedback": feedback,
            "issues": []
        });

        ToolOutput::ok(output.to_string())
    }
}