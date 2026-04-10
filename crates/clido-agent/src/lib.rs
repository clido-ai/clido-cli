//! Agent loop and execution.

pub mod agent_loop;
pub mod exploration;
pub mod orchestrator;
pub mod prompts;
pub mod provider_prompts;
pub mod sub_agent;

pub use agent_loop::metrics::{AgentMetrics, NoopAgentMetrics, TracingAgentMetrics};
pub use agent_loop::{
    session_lines_to_messages, try_session_lines_to_messages, AgentLoop, AskUser, EventEmitter,
    PermGrant, PermRequest,
};
pub use exploration::{
    ExplorationResult, ExplorationTask, Finding, FindingKind, TaskComplexity, TaskSplitter,
};
pub use orchestrator::{
    ExplorationOrchestrator, MultiAgentCostTracker, OrchestratorConfig, OrchestratorStats,
};
pub use sub_agent::SubAgent;
