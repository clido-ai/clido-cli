//! SubAgent: isolated agent instance with separate history and session.

use crate::agent_loop::AgentLoop;
use clido_core::{AgentConfig, Result};
use clido_providers::ModelProvider;
use clido_tools::ToolRegistry;
use std::sync::Arc;

/// A SubAgent wraps an AgentLoop with its own fresh history (no parent context pollution).
pub struct SubAgent {
    inner: AgentLoop,
}

impl SubAgent {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        registry: ToolRegistry,
        config: AgentConfig,
    ) -> Self {
        Self {
            inner: AgentLoop::new(provider, registry, config, None),
        }
    }

    /// Run the sub-agent on a prompt, returning text output.
    /// History is isolated: parent messages do NOT leak in.
    pub async fn run(&mut self, prompt: &str) -> Result<String> {
        self.inner.run(prompt, None, None, None).await
    }

    /// Cost of this sub-agent's run.
    pub fn cost_usd(&self) -> f64 {
        self.inner.cumulative_cost_usd
    }
}
