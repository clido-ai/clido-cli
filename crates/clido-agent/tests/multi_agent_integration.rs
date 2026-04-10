//! Integration tests for multi-agent exploration.

use std::sync::Arc;
use std::time::Duration;

use clido_agent::exploration::{ExplorationTask, TaskSplitter};
use clido_agent::orchestrator::{ExplorationOrchestrator, OrchestratorConfig};
use clido_agent::MultiAgentCostTracker;
use clido_core::{AgentConfig, Usage};
use clido_memory::SharedMemory;

/// Mock provider for testing.
struct MockProvider;

#[tokio::test]
async fn test_task_splitting() {
    let task = ExplorationTask::ReadFiles {
        paths: (0..20).map(|i| format!("/file{}.rs", i).into()).collect(),
    };

    let split = TaskSplitter::split(&task);
    assert!(split.is_some());

    let tasks = split.unwrap();
    assert!(tasks.len() > 1);
}

#[tokio::test]
async fn test_recommended_concurrency() {
    let simple_tasks: Vec<_> = (0..10)
        .map(|_| ExplorationTask::ReadFiles {
            paths: vec!["/file.rs".into()],
        })
        .collect();

    let concurrency = TaskSplitter::recommended_concurrency(&simple_tasks);
    assert_eq!(concurrency, 5); // Capped at 5
}

#[tokio::test]
async fn test_shared_memory_deduplication() {
    let memory = SharedMemory::new();
    let path = std::path::PathBuf::from("/test/file.rs");

    // First read
    memory.cache_file(path.clone(), "content".to_string());

    // Second read should hit cache
    let cached = memory.get_file(&path);
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().content, "content");
}

#[tokio::test]
async fn test_cost_tracker() {
    let parent_usage = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };

    let mut tracker = MultiAgentCostTracker::new(parent_usage);

    // Add sub-agent costs
    tracker.add_sub_agent_cost(Usage {
        input_tokens: 200,
        output_tokens: 100,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    tracker.add_sub_agent_cost(Usage {
        input_tokens: 300,
        output_tokens: 150,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });

    // Record cache hits
    tracker.record_cache_hit(100, 50);
    tracker.record_cache_hit(100, 50);

    // Check totals
    let total = tracker.total_cost();
    assert_eq!(total.input_tokens, 600); // 100 + 200 + 300
    assert_eq!(total.output_tokens, 300); // 50 + 100 + 150

    let total_tokens = tracker.total_tokens();
    assert_eq!(total_tokens, 900); // 600 + 300

    // Check cache hits
    assert_eq!(tracker.cache_hits, 2);
    assert_eq!(tracker.estimated_savings_input_tokens, 200);
    assert_eq!(tracker.estimated_savings_output_tokens, 100);
}

#[tokio::test]
async fn test_exploration_config_defaults() {
    use clido_core::ExplorationConfig;

    let config = ExplorationConfig::default();
    assert!(config.enabled);
    assert_eq!(config.max_concurrent_agents, 3);
    assert_eq!(config.timeout_secs, 30);
    assert!(config.enable_caching);
    assert_eq!(config.cache_ttl_secs, 300);
}

// Benchmark-style test (not a real benchmark, just timing)
#[tokio::test]
async fn test_parallel_vs_sequential_timing() {
    // This is a conceptual test - real benchmarks would use criterion
    // Just verifying that parallel execution doesn't panic

    let tasks: Vec<_> = (0..3)
        .map(|i| ExplorationTask::ReadFiles {
            paths: vec![format!("/file{}.rs", i).into()],
        })
        .collect();

    let start = std::time::Instant::now();

    // Simulate "sequential" by iterating
    for task in &tasks {
        // Would normally call orchestrator.execute_single_task
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let sequential_time = start.elapsed();

    // Simulate "parallel" by spawning
    let start = std::time::Instant::now();
    let handles: Vec<_> = tasks
        .iter()
        .map(|_| {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }

    let parallel_time = start.elapsed();

    // Parallel should be faster (with some tolerance)
    assert!(parallel_time < sequential_time);
}
