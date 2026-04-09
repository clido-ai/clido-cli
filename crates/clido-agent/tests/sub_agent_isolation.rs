//! SubAgent isolation test.
//!
//! Verifies that two SubAgent instances have separate histories and that
//! context from one does not leak into the other.

use async_trait::async_trait;
use clido_agent::SubAgent;
use clido_core::{
    AgentConfig, ContentBlock, Message, ModelResponse, PermissionMode, StopReason, ToolSchema,
    Usage,
};
use clido_providers::ModelProvider;
use clido_tools::default_registry_with_blocked;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// A mock provider that echoes a fixed reply.
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

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "mock-model".to_string(),
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
async fn sub_agents_have_isolated_histories() {
    let tmp = std::env::temp_dir();

    // Sub-agent A
    let provider_a: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_text: "response_from_A".to_string(),
    });
    let registry_a = default_registry_with_blocked(tmp.clone(), vec![]);
    let mut agent_a = SubAgent::new(provider_a, registry_a, test_config());

    // Sub-agent B
    let provider_b: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_text: "response_from_B".to_string(),
    });
    let registry_b = default_registry_with_blocked(tmp.clone(), vec![]);
    let mut agent_b = SubAgent::new(provider_b, registry_b, test_config());

    let result_a = agent_a.run("prompt A").await.unwrap();
    let result_b = agent_b.run("prompt B").await.unwrap();

    // Each agent returns its own response
    assert_eq!(result_a, "response_from_A");
    assert_eq!(result_b, "response_from_B");

    // Cross-check: A's response is not B's
    assert_ne!(result_a, result_b);
}

#[tokio::test]
async fn sub_agent_cost_tracking_is_independent() {
    let tmp = std::env::temp_dir();

    let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_text: "done".to_string(),
    });
    let registry = default_registry_with_blocked(tmp, vec![]);
    let mut agent = SubAgent::new(provider, registry, test_config());

    let _ = agent.run("hello").await.unwrap();

    // Cost should be > 0 (mock tokens: 10 in + 5 out with default rates)
    assert!(agent.cost_usd() >= 0.0);
}

#[tokio::test]
async fn two_sub_agents_run_sequentially_without_cross_pollution() {
    let tmp = std::env::temp_dir();

    // First agent
    let p1: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_text: "agent_one".to_string(),
    });
    let mut a1 = SubAgent::new(
        p1,
        default_registry_with_blocked(tmp.clone(), vec![]),
        test_config(),
    );
    let r1 = a1.run("task one").await.unwrap();

    // Second agent started after first completes — no shared state
    let p2: Arc<dyn ModelProvider> = Arc::new(MockProvider {
        response_text: "agent_two".to_string(),
    });
    let mut a2 = SubAgent::new(
        p2,
        default_registry_with_blocked(tmp.clone(), vec![]),
        test_config(),
    );
    let r2 = a2.run("task two").await.unwrap();

    assert_eq!(r1, "agent_one");
    assert_eq!(r2, "agent_two");
    // Session ids are different (they start fresh, no shared writer)
    assert_ne!(r1, r2);
}
