//! Shared types, errors, and config for Clido.

pub mod config;
pub mod config_loader;
pub mod error;
pub mod pricing;
pub mod types;

pub use config::{AgentConfig, PermissionMode, ProviderConfig, ProviderType};
pub use config_loader::{agent_config_from_loaded, load_config, LoadedConfig, ProfileEntry};
pub use error::{ClidoError, Result};
pub use pricing::{compute_cost_usd, load_pricing, PricingTable};
pub use types::{ContentBlock, Message, ModelResponse, Role, StopReason, ToolSchema, Usage};
