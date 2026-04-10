//! ExploreParallel tool for multi-agent exploration.
//!
//! Spawns parallel sub-agents to explore multiple files/directories
//! simultaneously, significantly speeding up codebase analysis.

use async_trait::async_trait;
use clido_core::{Tool, ToolOutput, ToolSchema};
use serde::{Deserialize, Serialize};

use crate::exploration::{ExplorationTask, TaskSplitter};
use crate::orchestrator::{ExplorationOrchestrator, OrchestratorConfig};

/// Input for the ExploreParallel tool.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExploreParallelInput {
    /// List of exploration tasks to execute in parallel.
    pub tasks: Vec<ExplorationTaskInput>,
    /// Optional: maximum number of concurrent sub-agents (default: 3).
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    /// Optional: timeout per task in seconds (default: 30).
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Individual exploration task input.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExplorationTaskInput {
    /// Explore a directory.
    ExploreDirectory {
        path: String,
        #[serde(default)]
        depth: u8,
    },
    /// Find files matching a pattern.
    FindFiles {
        pattern: String,
        #[serde(default = "default_max_results")]
        max_results: usize,
    },
    /// Search for a symbol.
    SearchSymbol {
        symbol: String,
        #[serde(default)]
        file_types: Vec<String>,
    },
    /// Read specific files.
    ReadFiles {
        paths: Vec<String>,
    },
    /// Grep search.
    GrepSearch {
        pattern: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// Get codebase overview.
    CodebaseOverview {
        #[serde(default = "default_max_files")]
        max_files: usize,
    },
}

fn default_max_results() -> usize {
    50
}

fn default_max_files() -> usize {
    30
}

impl From<ExplorationTaskInput> for ExplorationTask {
    fn from(input: ExplorationTaskInput) -> Self {
        match input {
            ExplorationTaskInput::ExploreDirectory { path, depth } => {
                ExplorationTask::ExploreDirectory {
                    path: path.into(),
                    depth,
                }
            }
            ExplorationTaskInput::FindFiles { pattern, max_results } => {
                ExplorationTask::FindFiles { pattern, max_results }
            }
            ExplorationTaskInput::SearchSymbol { symbol, file_types } => {
                ExplorationTask::SearchSymbol { symbol, file_types }
            }
            ExplorationTaskInput::ReadFiles { paths } => {
                ExplorationTask::ReadFiles {
                    paths: paths.into_iter().map(Into::into).collect(),
                }
            }
            ExplorationTaskInput::GrepSearch { pattern, path } => {
                ExplorationTask::GrepSearch {
                    pattern,
                    path: path.map(Into::into),
                }
            }
            ExplorationTaskInput::CodebaseOverview { max_files } => {
                ExplorationTask::CodebaseOverview { max_files }
            }
        }
    }
}

/// Output from the ExploreParallel tool.
#[derive(Clone, Debug, Serialize)]
pub struct ExploreParallelOutput {
    /// Number of tasks executed.
    pub tasks_executed: usize,
    /// Number of successful tasks.
    pub tasks_successful: usize,
    /// Number of failed tasks.
    pub tasks_failed: usize,
    /// Combined summary from all tasks.
    pub combined_summary: String,
    /// Total files read across all tasks.
    pub total_files_read: usize,
    /// Total cost across all sub-agents.
    pub total_cost_usd: f64,
    /// Time saved vs sequential execution (estimated).
    pub time_saved_seconds: u64,
    /// Individual task results.
    pub results: Vec<TaskResult>,
}

/// Individual task result.
#[derive(Clone, Debug, Serialize)]
pub struct TaskResult {
    /// Task description.
    pub task: String,
    /// Whether the task succeeded.
    pub success: bool,
    /// Summary of findings.
    pub summary: String,
    /// Files read.
    pub files_read: usize,
    /// Cost in USD.
    pub cost_usd: f64,
    /// Duration in seconds.
    pub duration_seconds: f64,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The ExploreParallel tool.
pub struct ExploreParallelTool {
    /// Orchestrator for managing sub-agents.
    orchestrator: ExplorationOrchestrator,
}

impl ExploreParallelTool {
    /// Create a new ExploreParallel tool.
    pub fn new(orchestrator: ExplorationOrchestrator) -> Self {
        Self { orchestrator }
    }
}

#[async_trait]
impl Tool for ExploreParallelTool {
    fn name(&self) -> &str {
        "ExploreParallel"
    }

    fn description(&self) -> &str {
        "Spawn parallel sub-agents to explore multiple files or directories simultaneously. \
         This is significantly faster than sequential exploration for large codebases. \
         Use this when you need to understand multiple parts of the codebase at once."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "description": "List of exploration tasks to execute in parallel",
                        "items": {
                            "type": "object",
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "explore_directory" },
                                        "path": { "type": "string" },
                                        "depth": { "type": "integer", "default": 3 }
                                    },
                                    "required": ["type", "path"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "find_files" },
                                        "pattern": { "type": "string" },
                                        "max_results": { "type": "integer", "default": 50 }
                                    },
                                    "required": ["type", "pattern"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "search_symbol" },
                                        "symbol": { "type": "string" },
                                        "file_types": { "type": "array", "items": { "type": "string" } }
                                    },
                                    "required": ["type", "symbol"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "read_files" },
                                        "paths": { "type": "array", "items": { "type": "string" } }
                                    },
                                    "required": ["type", "paths"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "grep_search" },
                                        "pattern": { "type": "string" },
                                        "path": { "type": "string" }
                                    },
                                    "required": ["type", "pattern"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "codebase_overview" },
                                        "max_files": { "type": "integer", "default": 30 }
                                    },
                                    "required": ["type"]
                                }
                            ]
                        }
                    },
                    "max_concurrent": {
                        "type": "integer",
                        "description": "Maximum number of concurrent sub-agents (default: 3)",
                        "minimum": 1,
                        "maximum": 10
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Timeout per task in seconds (default: 30)",
                        "minimum": 5,
                        "maximum": 120
                    }
                },
                "required": ["tasks"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> anyhow::Result<ToolOutput> {
        let input: ExploreParallelInput = serde_json::from_value(input)?;

        // Convert input tasks to exploration tasks
        let tasks: Vec<ExplorationTask> = input
            .tasks
            .into_iter()
            .map(Into::into)
            .collect();

        if tasks.is_empty() {
            return Ok(ToolOutput::ok("No tasks to execute"));
        }

        // Configure orchestrator
        let config = OrchestratorConfig {
            max_concurrent: input.max_concurrent.unwrap_or(3),
            task_timeout: std::time::Duration::from_secs(
                input.timeout_seconds.unwrap_or(30)
            ),
            ..Default::default()
        };

        // TODO: Update orchestrator config
        // For now, use the existing orchestrator

        tracing::info!(
            task_count = tasks.len(),
            max_concurrent = config.max_concurrent,
            "Starting parallel exploration"
        );

        // Execute tasks in parallel
        let results = self.orchestrator.execute_parallel(tasks.clone()).await;

        // Build output
        let mut output = ExploreParallelOutput {
            tasks_executed: results.len(),
            tasks_successful: 0,
            tasks_failed: 0,
            combined_summary: String::new(),
            total_files_read: 0,
            total_cost_usd: 0.0,
            time_saved_seconds: 0, // TODO: Calculate based on sequential estimate
            results: Vec::new(),
        };

        let mut summaries = Vec::new();

        for (idx, result) in results.iter().enumerate() {
            if result.success {
                output.tasks_successful += 1;
            } else {
                output.tasks_failed += 1;
            }

            output.total_files_read += result.files_read.len();
            output.total_cost_usd += result.usage.total_cost_usd.unwrap_or(0.0);

            summaries.push(format!(
                "Task {} ({}): {}",
                idx + 1,
                if result.success { "✓" } else { "✗" },
                result.summary
            ));

            output.results.push(TaskResult {
                task: tasks.get(idx).map(|t| t.description()).unwrap_or_default(),
                success: result.success,
                summary: result.summary.clone(),
                files_read: result.files_read.len(),
                cost_usd: result.usage.total_cost_usd.unwrap_or(0.0),
                duration_seconds: result.duration.as_secs_f64(),
                error: result.error.clone(),
            });
        }

        output.combined_summary = summaries.join("\n\n");

        // Calculate estimated time saved
        // Assume sequential would take sum of durations, parallel takes max duration
        let sequential_time: u64 = results
            .iter()
            .map(|r| r.duration.as_secs())
            .sum();
        let parallel_time: u64 = results
            .iter()
            .map(|r| r.duration.as_secs())
            .max()
            .unwrap_or(0);
        output.time_saved_seconds = sequential_time.saturating_sub(parallel_time);

        Ok(ToolOutput::ok(serde_json::to_string(&output)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_conversion() {
        let input = ExplorationTaskInput::ExploreDirectory {
            path: "/src".to_string(),
            depth: 3,
        };
        let task: ExplorationTask = input.into();
        assert!(matches!(task, ExplorationTask::ExploreDirectory { .. }));
    }

    #[test]
    fn test_schema_generation() {
        let tool = ExploreParallelTool {
            orchestrator: todo!(), // Would need mock
        };
        let schema = tool.schema();
        assert_eq!(schema.name, "ExploreParallel");
    }
}
