//! Session JSONL: SessionLine variants, SessionWriter, SessionReader, list_sessions.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

use crate::paths;

pub const SCHEMA_VERSION: u32 = 1;

/// One line in a session JSONL file (discriminated by `type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionLine {
    Meta {
        session_id: String,
        schema_version: u32,
        start_time: String,
        project_path: String,
    },
    UserMessage {
        role: String,
        content: Vec<serde_json::Value>,
    },
    AssistantMessage {
        content: Vec<serde_json::Value>,
    },
    ToolCall {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_hash: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mtime_nanos: Option<u64>,
    },
    System {
        subtype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Result {
        exit_status: String,
        total_cost_usd: f64,
        num_turns: u32,
        duration_ms: u64,
    },
}

/// Stale-file record from a ToolResult (path, content_hash, mtime_nanos).
#[derive(Debug, Clone)]
pub struct StaleFileRecord {
    pub path: String,
    pub content_hash: String,
    pub mtime_nanos: u64,
}

/// Summary for sessions list.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_path: String,
    pub start_time: String,
    pub num_turns: u32,
    pub total_cost_usd: f64,
    pub preview: String,
}

/// Append-only session file writer.
pub struct SessionWriter {
    file: std::fs::File,
}

impl SessionWriter {
    /// Create a new session file and write the meta line. Creates parent dirs.
    pub fn create(project_path: &Path, session_id: &str) -> anyhow::Result<Self> {
        let path = paths::session_file_path(project_path, session_id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let start_time = chrono::Utc::now().to_rfc3339();
        let meta = SessionLine::Meta {
            session_id: session_id.to_string(),
            schema_version: SCHEMA_VERSION,
            start_time: start_time.clone(),
            project_path: project_path.display().to_string(),
        };
        serde_json::to_writer(&mut file, &meta)?;
        file.write_all(b"\n")?;
        Ok(Self { file })
    }

    /// Open an existing session file for appending (resume).
    pub fn append(project_path: &Path, session_id: &str) -> anyhow::Result<Self> {
        let path = paths::session_file_path(project_path, session_id)?;
        let file = std::fs::OpenOptions::new().append(true).open(&path)?;
        Ok(Self { file })
    }

    /// Append a session line (newline-delimited JSON).
    pub fn write_line(&mut self, line: &SessionLine) -> anyhow::Result<()> {
        serde_json::to_writer(&mut self.file, line)?;
        self.file.write_all(b"\n")?;
        Ok(())
    }
}

impl std::io::Write for SessionWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Load a session file for resume.
pub struct SessionReader;

impl SessionReader {
    /// Load all lines from a session file. Fails if schema_version > supported.
    pub fn load(project_path: &Path, session_id: &str) -> anyhow::Result<Vec<SessionLine>> {
        let path = paths::session_file_path(project_path, session_id)?;
        let content = std::fs::read_to_string(&path)?;
        let mut lines = Vec::new();
        for (i, line_str) in content.lines().enumerate() {
            if line_str.trim().is_empty() {
                continue;
            }
            let line: SessionLine = serde_json::from_str(line_str)
                .map_err(|e| anyhow::anyhow!("line {}: {}", i + 1, e))?;
            if let SessionLine::Meta { schema_version, .. } = &line {
                if *schema_version > SCHEMA_VERSION {
                    anyhow::bail!(
                        "Session schema version {} is newer than supported {}",
                        schema_version,
                        SCHEMA_VERSION
                    );
                }
            }
            lines.push(line);
        }
        Ok(lines)
    }

    /// Collect stale-file records from ToolResult lines (path, content_hash, mtime_nanos).
    pub fn stale_file_records(lines: &[SessionLine]) -> Vec<StaleFileRecord> {
        lines
            .iter()
            .filter_map(|l| {
                if let SessionLine::ToolResult {
                    path: Some(path),
                    content_hash: Some(content_hash),
                    mtime_nanos: Some(mtime_nanos),
                    ..
                } = l
                {
                    Some(StaleFileRecord {
                        path: path.clone(),
                        content_hash: content_hash.clone(),
                        mtime_nanos: *mtime_nanos,
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Returns paths that have been modified since the session (content hash or mtime differs).
pub fn stale_paths(records: &[StaleFileRecord]) -> Vec<String> {
    let mut stale = Vec::new();
    for r in records {
        let Ok(meta) = std::fs::metadata(&r.path) else {
            stale.push(r.path.clone());
            continue;
        };
        let mtime_nanos = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_nanos() as u64)
        });
        let Ok(content) = std::fs::read_to_string(&r.path) else {
            stale.push(r.path.clone());
            continue;
        };
        let hash = Sha256::digest(content.as_bytes());
        let current_hash = hex::encode(hash);
        if mtime_nanos != Some(r.mtime_nanos) || current_hash != r.content_hash {
            stale.push(r.path.clone());
        }
    }
    stale
}

/// List sessions for a project (newest first). Reads session dir and parses first line for meta.
pub fn list_sessions(project_path: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let dir = paths::session_dir_for_project(project_path)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let load_result: Result<Vec<SessionLine>, anyhow::Error> =
                    SessionReader::load(project_path, stem);
                if let Ok(lines) = load_result {
                    if let Some(SessionLine::Meta {
                        session_id,
                        start_time,
                        project_path: proj,
                        ..
                    }) = lines.first()
                    {
                        let (num_turns, total_cost_usd, preview) = summarize_lines(&lines);
                        summaries.push(SessionSummary {
                            session_id: session_id.clone(),
                            project_path: proj.clone(),
                            start_time: start_time.clone(),
                            num_turns,
                            total_cost_usd,
                            preview,
                        });
                    }
                }
            }
        }
    }
    summaries.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    Ok(summaries)
}

fn summarize_lines(lines: &[SessionLine]) -> (u32, f64, String) {
    let mut num_turns = 0u32;
    let mut total_cost_usd = 0.0;
    let mut preview = String::new();
    for line in lines {
        match line {
            SessionLine::UserMessage { content, .. } => {
                let first_text = content
                    .first()
                    .and_then(|c: &serde_json::Value| c.get("text"));
                if let Some(s) = first_text.and_then(|v: &serde_json::Value| v.as_str()) {
                    preview = s.chars().take(50).collect::<String>();
                    if s.len() > 50 {
                        preview.push('…');
                    }
                }
            }
            SessionLine::Result {
                num_turns: n,
                total_cost_usd: c,
                ..
            } => {
                num_turns = *n;
                total_cost_usd = *c;
            }
            _ => {}
        }
    }
    (num_turns, total_cost_usd, preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_writer_create_and_write_line() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "test-session-1";
        let mut w = SessionWriter::create(project_path, id).unwrap();
        w.write_line(&SessionLine::UserMessage {
            role: "user".to_string(),
            content: vec![serde_json::json!({"type": "text", "text": "hello"})],
        })
        .unwrap();
        w.flush().unwrap();
        drop(w);
        let lines = SessionReader::load(project_path, id).unwrap();
        assert!(lines.len() >= 2);
        assert!(matches!(lines[0], SessionLine::Meta { .. }));
    }

    #[test]
    fn stale_file_records_from_tool_results() {
        let lines = vec![SessionLine::ToolResult {
            tool_use_id: "x".into(),
            content: "ok".into(),
            is_error: false,
            duration_ms: None,
            path: Some("/tmp/a".into()),
            content_hash: Some("abc".into()),
            mtime_nanos: Some(123),
        }];
        let records = SessionReader::stale_file_records(&lines);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, "/tmp/a");
        assert_eq!(records[0].content_hash, "abc");
    }
}
