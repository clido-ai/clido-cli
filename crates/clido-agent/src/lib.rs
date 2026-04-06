//! Agent loop and execution.

pub mod agent_loop;
pub mod prompts;
pub mod provider_prompts;
pub mod sub_agent;

pub use agent_loop::{
    session_lines_to_messages, try_session_lines_to_messages, AgentLoop, AskUser, EventEmitter,
    PermGrant, PermRequest,
};
pub use agent_loop::metrics::{AgentMetrics, NoopAgentMetrics, TracingAgentMetrics};
pub use sub_agent::SubAgent;
