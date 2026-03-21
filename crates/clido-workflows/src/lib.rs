//! Declarative YAML workflow engine: load, validate, template, execute (step runner abstraction).

pub mod context;
pub mod executor;
pub mod loader;
pub mod template;
pub mod types;

pub use context::{StepResult, WorkflowContext};
pub use executor::{
    run as run_workflow, StepRunRequest, StepRunResult, WorkflowStepRunner, WorkflowSummary,
};
pub use loader::{load, preflight, required_tools_and_profiles, validate, PreflightCheck, PreflightResult, PreflightStatus};
pub use template::render;
pub use types::{
    BackoffKind, InputDef, OnErrorPolicy, OutputConfig, OutputDef, PrereqEntry, PrerequisitesDef,
    RetryConfig, StepDef, WorkflowDef,
};
