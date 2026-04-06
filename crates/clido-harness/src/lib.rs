//! Structured harness: JSON tasks (append-only order, fail→pass only via verification),
//! NDJSON progress log, optional git snapshot for handoff.

mod error;
mod state;
mod store;

pub use error::HarnessError;
pub use state::{
    AcceptanceResult, HarnessMeta, HarnessState, HarnessTask, StoredVerification, TaskPassState,
    VerificationPayload,
};
pub use store::{
    append_progress, git_log_snippet, progress_path, read_progress_tail, read_state,
    reconcile_order, tasks_path, touch_meta_timestamp, write_state,
};

pub type Result<T> = std::result::Result<T, HarnessError>;
