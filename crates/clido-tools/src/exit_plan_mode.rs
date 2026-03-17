//! ExitPlanMode tool: no parameters; when executed, the agent loop switches from PlanOnly to Default.

use super::{Tool, ToolOutput};
use async_trait::async_trait;

/// Tool that signals the agent to switch from plan-only to default (interactive) permission mode.
#[derive(Debug, Clone, Copy)]
pub struct ExitPlanModeTool;

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn description(&self) -> &str {
        "Switch from plan-only mode to agent mode. State-changing tools (Write, Edit, Bash) will then be allowed (with approval in default mode). Call this when you are ready to execute the plan."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolOutput {
        ToolOutput::ok(
            "Switched to agent mode. State-changing tools are now available.".to_string(),
        )
    }
}
