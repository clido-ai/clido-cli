//! Orchestrator for multi-agent exploration.
//!
//! Manages parallel execution of exploration tasks across multiple sub-agents,
//! coordinating shared memory, rate limiting, and result aggregation.

use std::sync::Arc;
use std::time::Duration;

use clido_core::{AgentConfig, Usage};
use clido_memory::SharedMemory;
use clido_providers::{ModelProvider, RateLimiter, RateLimiterRegistry, RateLimitConfig};
use clido_tools::ToolRegistry;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::exploration::{
    ExplorationResult, ExplorationTask, TaskComplexity, TaskSplitter,
};
use crate::sub_agent::SubAgent;

/// Configuration for the exploration orchestrator.
#[derive(Clone, Debug)]
pub struct OrchestratorConfig {
    /// Maximum number of concurrent sub-agents.
    pub max_concurrent: usize,
    /// Timeout for each sub-agent task.
    pub task_timeout: Duration,
    /// Whether to enable result caching.
    pub enable_caching: bool,
    /// Rate limit configuration for sub-agents.
    pub rate_limit_config: RateLimitConfig,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            task_timeout: Duration::from_secs(30),
            enable_caching: true,
            rate_limit_config: RateLimitConfig::default(),
        }
    }
}

/// Orchestrator for parallel exploration tasks.
pub struct ExplorationOrchestrator {
    config: OrchestratorConfig,
    /// Semaphore to control concurrent sub-agents.
    concurrency_semaphore: Arc<Semaphore>,
    /// Shared memory for file deduplication.
    shared_memory: SharedMemory,
    /// Rate limiter registry for API calls.
    rate_limiter_registry: RateLimiterRegistry,
    /// Parent agent configuration (inherited by sub-agents).
    parent_config: AgentConfig,
    /// Model provider (shared with sub-agents).
    provider: Arc<dyn ModelProvider>,
}

impl ExplorationOrchestrator {
    /// Create a new orchestrator.
    pub fn new(
        config: OrchestratorConfig,
        parent_config: AgentConfig,
        provider: Arc<dyn ModelProvider>,
    ) -> Self {
        let concurrency_semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        let shared_memory = if config.enable_caching {
            SharedMemory::new()
        } else {
            SharedMemory::with_config(0, Duration::from_secs(0), Duration::from_secs(0))
        };

        Self {
            config,
            concurrency_semaphore,
            shared_memory,
            rate_limiter_registry: RateLimiterRegistry::new(),
            parent_config,
            provider,
        }
    }

    /// Execute multiple exploration tasks in parallel.
    ///
    /// This method:
    /// 1. Splits complex tasks if needed
    /// 2. Spawns sub-agents up to max_concurrent
    /// 3. Waits for all to complete
    /// 4. Merges and deduplicates results
    pub async fn execute_parallel(
        &self,
        tasks: Vec<ExplorationTask>,
    ) -> Vec<ExplorationResult> {
        if tasks.is_empty() {
            return Vec::new();
        }

        // Step 1: Split complex tasks
        let mut expanded_tasks = Vec::new();
        for task in tasks {
            if let Some(split) = TaskSplitter::split(&task) {
                expanded_tasks.extend(split);
            } else {
                expanded_tasks.push(task);
            }
        }

        // Step 2: Determine optimal concurrency
        let recommended = TaskSplitter::recommended_concurrency(&expanded_tasks);
        let concurrency = recommended.min(self.config.max_concurrent);

        tracing::info!(
            total_tasks = expanded_tasks.len(),
            concurrency,
            "Starting parallel exploration"
        );

        // Step 3: Execute tasks with controlled concurrency
        let semaphore = self.concurrency_semaphore.clone();
        let mut handles = Vec::new();

        for (idx, task) in expanded_tasks.into_iter().enumerate() {
            let permit = semaphore.clone().acquire_owned().await.ok();
            if permit.is_none() {
                tracing::error!("Failed to acquire concurrency permit");
                continue;
            }

            let handle = tokio::spawn({
                let shared_memory = self.shared_memory.clone();
                let rate_limiter_registry = self.rate_limiter_registry.clone();
                let parent_config = self.parent_config.clone();
                let provider = self.provider.clone();
                let config = self.config.clone();
                async move {
                    let _permit = permit; // Hold permit until task completes
                    Self::execute_single_task_static(
                        config,
                        parent_config,
                        provider,
                        shared_memory,
                        rate_limiter_registry,
                        task,
                    )
                    .await
                }
            });

            handles.push((idx, handle));
        }

        // Step 4: Collect results
        let mut results = Vec::new();
        for (idx, handle) in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::error!(task_idx = idx, error = %e, "Task panicked");
                    // Create error result
                    results.push(ExplorationResult {
                        task_id: Uuid::new_v4(),
                        task: ExplorationTask::CodebaseOverview { max_files: 0 }, // Placeholder
                        files_read: Vec::new(),
                        findings: Vec::new(),
                        summary: format!("Task failed: {}", e),
                        usage: Usage::default(),
                        duration: Duration::from_secs(0),
                        success: false,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        // Step 5: Merge and deduplicate
        let merged = self.merge_results(results);

        tracing::info!(
            total_results = merged.len(),
            "Parallel exploration completed"
        );

        merged
    }

    /// Execute a single exploration task (static version for spawn).
    async fn execute_single_task_static(
        config: OrchestratorConfig,
        parent_config: AgentConfig,
        provider: Arc<dyn ModelProvider>,
        _shared_memory: SharedMemory,
        rate_limiter_registry: RateLimiterRegistry,
        task: ExplorationTask,
    ) -> ExplorationResult {
        let task_id = Uuid::new_v4();
        let start = std::time::Instant::now();

        tracing::debug!(task_id = %task_id, task = %task.description(), "Starting task");

        // Create sub-agent with restricted tools
        let _sub_agent = SubAgent::new(
            provider,
            ToolRegistry::default(), // TODO: Pass restricted tool registry
            parent_config,
        );

        // Get rate limiter for this provider
        let rate_limiter = rate_limiter_registry
            .get_or_create("default", config.rate_limit_config)
            .await;

        // Acquire rate limit permit
        let _permit = match rate_limiter.acquire().await {
            Ok(permit) => permit,
            Err(e) => {
                return ExplorationResult {
                    task_id,
                    task: task.clone(),
                    files_read: Vec::new(),
                    findings: Vec::new(),
                    summary: format!("Rate limit error: {}", e),
                    usage: Usage::default(),
                    duration: start.elapsed(),
                    success: false,
                    error: Some(e.to_string()),
                };
            }
        };

        // Execute with timeout
        let result = tokio::time::timeout(
            config.task_timeout,
            Self::run_exploration_static(_sub_agent, task.clone()),
        )
        .await;

        let duration = start.elapsed();

        match result {
            Ok(Ok(exploration_result)) => {
                tracing::debug!(task_id = %task_id, "Task completed successfully");
                exploration_result
            }
            Ok(Err(e)) => {
                tracing::error!(task_id = %task_id, error = %e, "Task failed");
                ExplorationResult {
                    task_id,
                    task,
                    files_read: Vec::new(),
                    findings: Vec::new(),
                    summary: format!("Exploration error: {}", e),
                    usage: Usage::default(),
                    duration,
                    success: false,
                    error: Some(e.to_string()),
                }
            }
            Err(_) => {
                tracing::error!(task_id = %task_id, "Task timed out");
                ExplorationResult {
                    task_id,
                    task,
                    files_read: Vec::new(),
                    findings: Vec::new(),
                    summary: "Task timed out".to_string(),
                    usage: Usage::default(),
                    duration,
                    success: false,
                    error: Some("Timeout".to_string()),
                }
            }
        }
    }

    /// Execute a single exploration task.
    async fn execute_single_task(&self, task: ExplorationTask) -> ExplorationResult {
        Self::execute_single_task_static(
            self.config.clone(),
            self.parent_config.clone(),
            self.provider.clone(),
            self.shared_memory.clone(),
            self.rate_limiter_registry.clone(),
            task,
        )
        .await
    }

    /// Run the actual exploration using a sub-agent (static version).
    async fn run_exploration_static(
        _sub_agent: SubAgent,
        task: ExplorationTask,
    ) -> anyhow::Result<ExplorationResult> {
        // TODO: Implement actual exploration logic
        // This would:
        // 1. Convert task to a prompt for the sub-agent
        // 2. Run the sub-agent
        // 3. Parse results
        // 4. Return ExplorationResult

        // Placeholder implementation
        Ok(ExplorationResult {
            task_id: Uuid::new_v4(),
            task,
            files_read: Vec::new(),
            findings: Vec::new(),
            summary: "Placeholder result".to_string(),
            usage: Usage::default(),
            duration: Duration::from_secs(0),
            success: true,
            error: None,
        })
    }

    /// Merge and deduplicate results from multiple sub-agents.
    fn merge_results(&self, results: Vec<ExplorationResult>) -> Vec<ExplorationResult> {
        // TODO: Implement proper merging logic
        // - Deduplicate findings
        // - Combine summaries
        // - Aggregate usage
        // - Sort by relevance

        results
    }

    /// Get current statistics for monitoring.
    pub async fn stats(&self) -> OrchestratorStats {
        let rate_limiter_stats = self.rate_limiter_registry.all_stats().await;
        let memory_stats = self.shared_memory.stats();

        OrchestratorStats {
            max_concurrent: self.config.max_concurrent,
            available_permits: self.concurrency_semaphore.available_permits(),
            rate_limiter_stats,
            memory_stats,
        }
    }
}

/// Statistics for monitoring the orchestrator.
#[derive(Debug)]
pub struct OrchestratorStats {
    pub max_concurrent: usize,
    pub available_permits: usize,
    pub rate_limiter_stats: Vec<clido_providers::RateLimiterStats>,
    pub memory_stats: clido_memory::CacheStats,
    /// Total cost across all completed tasks.
    pub total_cost_usd: f64,
    /// Number of tasks completed.
    pub tasks_completed: usize,
    /// Estimated cost savings from caching.
    pub cache_savings_usd: f64,
}

/// Multi-agent cost tracker.
#[derive(Clone, Debug, Default)]
pub struct MultiAgentCostTracker {
    /// Parent agent cost.
    pub parent_cost: Usage,
    /// Costs per sub-agent.
    pub sub_agent_costs: Vec<Usage>,
    /// Cache hits (avoided API calls).
    pub cache_hits: usize,
    /// Estimated savings from caching.
    pub estimated_savings_usd: f64,
}

impl MultiAgentCostTracker {
    /// Create a new cost tracker.
    pub fn new(parent_cost: Usage) -> Self {
        Self {
            parent_cost,
            sub_agent_costs: Vec::new(),
            cache_hits: 0,
            estimated_savings_usd: 0.0,
        }
    }

    /// Add a sub-agent's cost.
    pub fn add_sub_agent_cost(&mut self, cost: Usage) {
        self.sub_agent_costs.push(cost);
    }

    /// Record a cache hit.
    pub fn record_cache_hit(&mut self, estimated_cost_usd: f64) {
        self.cache_hits += 1;
        self.estimated_savings_usd += estimated_cost_usd;
    }

    /// Get total cost (parent + all sub-agents).
    pub fn total_cost(&self) -> Usage {
        let mut total = self.parent_cost.clone();
        for cost in &self.sub_agent_costs {
            total.input_tokens += cost.input_tokens;
            total.output_tokens += cost.output_tokens;
            if let Some(parent_cost) = total.total_cost_usd {
                if let Some(sub_cost) = cost.total_cost_usd {
                    total.total_cost_usd = Some(parent_cost + sub_cost);
                }
            }
        }
        total
    }

    /// Get total cost in USD.
    pub fn total_cost_usd(&self) -> f64 {
        self.total_cost().total_cost_usd.unwrap_or(0.0)
    }

    /// Get summary string for display.
    pub fn summary(&self) -> String {
        let total = self.total_cost_usd();
        let parent = self.parent_cost.total_cost_usd.unwrap_or(0.0);
        let sub_agents: f64 = self.sub_agent_costs.iter()
            .map(|c| c.total_cost_usd.unwrap_or(0.0))
            .sum();
        
        format!(
            "Total: ${:.4} (parent: ${:.4}, sub-agents: ${:.4}, cache savings: ${:.4})",
            total, parent, sub_agents, self.estimated_savings_usd
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_config_default() {
        let config = OrchestratorConfig::default();
        assert_eq!(config.max_concurrent, 3);
        assert_eq!(config.task_timeout, Duration::from_secs(30));
        assert!(config.enable_caching);
    }

    // More tests would require mocking the ModelProvider
}
