//! Concurrent provider correctness test.
//!
//! Verifies that multiple agent instances using a mock provider can run
//! concurrently without result interleaving or data races.

use async_trait::async_trait;
use clido_agent::AgentLoop;
use clido_core::{
    AgentConfig, ContentBlock, Message, ModelResponse, PermissionMode, StopReason, ToolSchema,
    Usage,
};
use clido_providers::ModelProvider;
use clido_tools::default_registry_with_blocked;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// A mock provider that always returns a fixed text response.
struct MockProvider {
    response_text: String,
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        _config: &AgentConfig,
    ) -> clido_core::Result<ModelResponse> {
        // Simulate a tiny bit of async work without actual I/O.
        tokio::task::yield_now().await;
        Ok(ModelResponse {
            id: "mock-id".to_string(),
            model: "mock-model".to_string(),
            content: vec![ContentBlock::Text {
                text: self.response_text.clone(),
            }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        })
    }

    async fn complete_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolSchema],
        _config: &AgentConfig,
    ) -> clido_core::Result<
        Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
    > {
        unimplemented!("stream not used in this test")
    }
    async fn list_models(&self) -> std::result::Result<Vec<clido_providers::ModelEntry>, String> {
        Ok(vec![])
    }
}

/// Build a minimal AgentConfig for testing.
fn test_config(model: &str) -> AgentConfig {
    AgentConfig {
        model: model.to_string(),
        system_prompt: None,
        max_turns: 3,
        max_budget_usd: None,
        permission_mode: PermissionMode::AcceptAll,
        permission_rules: vec![],
        max_context_tokens: None,
        compaction_threshold: None,
        quiet: false,
        max_parallel_tools: 4,
        use_planner: false,
        use_index: false,
        no_rules: false,
        rules_file: None,
        max_output_tokens: None,
        ..Default::default()
    }
}

#[tokio::test]
async fn concurrent_agents_return_correct_responses() {
    const CONCURRENCY: usize = 5;
    let expected = "Hello from mock provider";

    let tasks: Vec<_> = (0..CONCURRENCY)
        .map(|i| {
            let response_text = expected.to_string();
            tokio::spawn(async move {
                let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider { response_text });
                let tmp = std::env::temp_dir();
                let registry = default_registry_with_blocked(tmp, vec![]);
                let config = test_config("mock-model");
                let mut agent = AgentLoop::new(provider, registry, config, None);
                let result = agent.run("hello", None, None, None).await;
                (i, result)
            })
        })
        .collect();

    for task in tasks {
        let (i, result) = task.await.expect("task panicked");
        let text = result.unwrap_or_else(|e| panic!("agent {} failed: {}", i, e));
        assert_eq!(text, expected, "agent {} returned unexpected response", i);
    }
}

#[tokio::test]
async fn concurrent_cost_tracking_is_independent() {
    // Each agent should have its own cost counter, not shared state.
    const CONCURRENCY: usize = 5;

    let tasks: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            tokio::spawn(async move {
                let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider {
                    response_text: "done".to_string(),
                });
                let tmp = std::env::temp_dir();
                let registry = default_registry_with_blocked(tmp, vec![]);
                let config = test_config("mock-model");
                let mut agent = AgentLoop::new(provider, registry, config, None);
                let _ = agent.run("ping", None, None, None).await;
                // Each agent should have independent token counts
                (
                    agent.cumulative_input_tokens,
                    agent.cumulative_output_tokens,
                )
            })
        })
        .collect();

    for task in tasks {
        let (input_tok, output_tok) = task.await.expect("task panicked");
        // Each agent did exactly one turn with the mock (10 input, 5 output).
        assert_eq!(input_tok, 10, "input tokens should be 10");
        assert_eq!(output_tok, 5, "output tokens should be 5");
    }
}
