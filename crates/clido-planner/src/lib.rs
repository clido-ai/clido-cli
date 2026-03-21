//! Task planner: decomposes prompts into a DAG of tasks and executes them
//! in dependency order with optional parallelism.
//!
//! Activated only when `--planner` is passed to the CLI; never auto-enabled.
//! Falls back to the reactive agent loop whenever the plan is invalid or
//! low-quality.

pub mod executor;
pub mod graph;
pub mod parser;

pub use executor::{PlanExecutor, PlanResult, TaskResult, TaskRunner};
pub use graph::{GraphError, TaskGraph, TaskId, TaskNode};
pub use parser::{parse_plan, PlanParseError};
