//! Storage: session JSONL, paths (XDG data dir).

mod paths;
mod session;

pub use paths::{data_dir, session_dir_for_project, session_file_path, workflow_run_path};
pub use session::{
    list_sessions, stale_paths, SessionLine, SessionReader, SessionSummary, SessionWriter,
    StaleFileRecord, SCHEMA_VERSION,
};
