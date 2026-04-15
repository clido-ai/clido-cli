//! Session JSONL: SessionLine variants, SessionWriter, SessionReader, list_sessions.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Seek, SeekFrom, Write};
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
    Title {
        title: String,
    },
}

/// Replace a token that starts at `start` with a redacted placeholder.
/// Scans forward from `start` consuming alphanumeric, `-`, `_`, `.`, `+`, `/`
/// characters (base64/URL-safe alphabet used by most API keys).
fn redact_token(s: &str, start: usize) -> String {
    let prefix_end = start;
    let mut end = prefix_end;
    let bytes = s.as_bytes();
    while end < bytes.len() {
        let b = bytes[end];
        if b.is_ascii_alphanumeric()
            || b == b'-'
            || b == b'_'
            || b == b'.'
            || b == b'+'
            || b == b'/'
        {
            end += 1;
        } else {
            break;
        }
    }
    if end > prefix_end {
        format!("{}[REDACTED]{}", &s[..prefix_end], &s[end..])
    } else {
        s.to_string()
    }
}

/// Redact known API key patterns from a JSON string before persisting to disk.
///
/// Handles: Anthropic (`sk-ant-`), OpenRouter (`sk-or-`), OpenAI (`sk-proj-`),
/// generic `sk-` keys (for providers that share this prefix), and AWS access
/// keys (`AKIA`).
/// This operates on the raw JSON string so it works across all field types without
/// needing to know the schema.
pub fn redact_secrets(s: &str) -> String {
    const PREFIXES: &[&str] = &["sk-ant-", "sk-or-", "sk-proj-", "sk-", "AKIA"];

    let mut result = s.to_string();
    for prefix in PREFIXES {
        let mut search_from = 0;
        loop {
            match result[search_from..].find(prefix) {
                None => break,
                Some(rel) => {
                    // Avoid double-redacting provider-specific keys when scanning generic `sk-`.
                    if *prefix == "sk-" {
                        let after = &result[search_from + rel + prefix.len()..];
                        if after.starts_with("ant-")
                            || after.starts_with("or-")
                            || after.starts_with("proj-")
                        {
                            search_from = search_from + rel + prefix.len();
                            if search_from >= result.len() {
                                break;
                            }
                            continue;
                        }
                    }
                    let abs = search_from + rel + prefix.len();
                    result = redact_token(&result, abs);
                    // Advance past the prefix to avoid infinite loop.
                    search_from = search_from + rel + prefix.len();
                    if search_from >= result.len() {
                        break;
                    }
                }
            }
        }
    }
    result
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
    pub created_at: String,
    pub last_edited: String,
    pub num_turns: u32,
    pub total_cost_usd: f64,
    pub preview: String,
    pub title: Option<String>,
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
    ///
    /// Secret patterns (API keys, AWS credentials) are redacted before writing
    /// so that credentials captured in tool inputs/outputs don't persist to disk.
    pub fn write_line(&mut self, line: &SessionLine) -> anyhow::Result<()> {
        let json = serde_json::to_string(line)?;
        let redacted = redact_secrets(&json);
        self.file.write_all(redacted.as_bytes())?;
        self.file.write_all(b"\n")?;
        // Flush immediately so sessions appear in /sessions without delay.
        self.file.flush()?;
        Ok(())
    }

    /// Write a session line and log any I/O error to stderr (non-fatal).
    /// Preferred over `let _ = write_line(...)` to avoid silent data loss.
    ///
    /// Agent rollback uses [`Self::end_offset`] captured before a burst of `log_write_line` calls;
    /// if a write fails here, the on-disk file may not include that line while memory might — the
    /// error is still logged so operators can notice drift.
    pub fn log_write_line(&mut self, line: &SessionLine) {
        if let Err(e) = self.write_line(line) {
            eprintln!("[clido] session write error: {e}");
        }
    }

    /// Byte length of the session file after flushing — call **before** appending lines you may
    /// need to roll back if the agent turn fails on disk.
    pub fn end_offset(&mut self) -> std::io::Result<u64> {
        self.file.flush()?;
        Ok(self.file.metadata()?.len())
    }

    /// Truncate the session file to `byte_len` bytes (must be ≤ current length). Used when the
    /// agent rolls back an in-memory turn so JSONL stays aligned with `AgentLoop.history`.
    pub fn truncate_to(&mut self, byte_len: u64) -> std::io::Result<()> {
        self.file.flush()?;
        let len = self.file.metadata()?.len();
        if byte_len > len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("truncate_to: byte_len {byte_len} exceeds file length {len}"),
            ));
        }
        self.file.set_len(byte_len)?;
        self.file.seek(SeekFrom::Start(byte_len))?;
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
    /// Load all lines from a session file using streaming I/O to avoid loading
    /// large sessions entirely into memory. Fails if schema_version > supported.
    pub fn load(project_path: &Path, session_id: &str) -> anyhow::Result<Vec<SessionLine>> {
        let path = paths::session_file_path(project_path, session_id)?;
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        let mut lines = Vec::new();
        for (i, raw) in reader.lines().enumerate() {
            let line_str = raw.map_err(|e| anyhow::anyhow!("line {}: read error: {}", i + 1, e))?;
            if line_str.trim().is_empty() {
                continue;
            }
            let line: SessionLine = serde_json::from_str(&line_str)
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
        let Ok(content) = std::fs::read(&r.path) else {
            stale.push(r.path.clone());
            continue;
        };
        let hash = Sha256::digest(&content);
        let current_hash = hex::encode(hash);
        if mtime_nanos != Some(r.mtime_nanos) || current_hash != r.content_hash {
            stale.push(r.path.clone());
        }
    }
    stale
}

/// Remove session files older than `max_age_days` days from the session directory.
/// Called at TUI startup to prevent unbounded disk growth over months of use.
/// Only affects `.jsonl` files; ignores any other files in the directory.
pub fn prune_old_sessions(project_path: &Path, max_age_days: u64) -> anyhow::Result<u32> {
    let dir = paths::session_dir_for_project(project_path)?;
    if !dir.exists() {
        return Ok(0);
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(max_age_days * 86_400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0u32;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff && std::fs::remove_file(&path).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
    }
    Ok(removed)
}

/// Delete a single session file by its ID.
pub fn delete_session(project_path: &Path, session_id: &str) -> anyhow::Result<()> {
    let path = paths::session_file_path(project_path, session_id)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Find a **brand-new** session file (meta line present, no user messages yet) modified
/// within the last `max_age` duration.
///
/// This dedupes parallel startup / recovery races that would otherwise create multiple
/// empty session files. We intentionally **do not** reuse sessions that already have user
/// turns — a normal cold start should get a fresh UUID, not append to someone else's chat.
pub fn find_recent_session(project_path: &Path, max_age: std::time::Duration) -> Option<String> {
    let dir = paths::session_dir_for_project(project_path).ok()?;
    if !dir.exists() {
        return None;
    }
    let now = std::time::SystemTime::now();
    let cutoff = now
        .checked_sub(max_age)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let mut most_recent: Option<(std::time::SystemTime, String)> = None;

    for entry in std::fs::read_dir(&dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    if modified >= cutoff {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            if let Ok(lines) = SessionReader::load(project_path, stem) {
                                let has_user = lines
                                    .iter()
                                    .any(|l| matches!(l, SessionLine::UserMessage { .. }));
                                let has_meta =
                                    lines.iter().any(|l| matches!(l, SessionLine::Meta { .. }));
                                if has_meta
                                    && !has_user
                                    && most_recent
                                        .as_ref()
                                        .map(|(t, _)| modified > *t)
                                        .unwrap_or(true)
                                {
                                    most_recent = Some((modified, stem.to_string()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    most_recent.map(|(_, id)| id)
}

/// Delete all sessions that have no user messages (empty sessions).
/// Returns the number of deleted sessions.
pub fn delete_empty_sessions(project_path: &Path) -> anyhow::Result<usize> {
    let dir = paths::session_dir_for_project(project_path)?;
    if !dir.exists() {
        return Ok(0);
    }
    let mut deleted = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(lines) = SessionReader::load(project_path, stem) {
                    let has_user = lines
                        .iter()
                        .any(|l| matches!(l, SessionLine::UserMessage { .. }));
                    if !has_user {
                        // Delete empty session
                        let _ = std::fs::remove_file(&path);
                        deleted += 1;
                    }
                }
            }
        }
    }
    Ok(deleted)
}

/// List sessions for a project (last edited first). Reads session dir and parses first line for meta.
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
                    let first: Option<&SessionLine> = lines.first();
                    if let Some(SessionLine::Meta {
                        session_id: _,
                        start_time,
                        project_path: proj,
                        ..
                    }) = first
                    {
                        let (num_turns, total_cost_usd, preview, title) = summarize_lines(&lines);
                        // Skip sessions with no user messages (launched but nothing sent).
                        if num_turns == 0 {
                            continue;
                        }
                        // Get last modified time from filesystem as last_edited
                        let last_edited = path
                            .metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
                            .flatten()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| start_time.clone());
                        // Use the filename (stem) as the session ID, not the meta session_id.
                        // This ensures consistency when resuming sessions.
                        summaries.push(SessionSummary {
                            session_id: stem.to_string(),
                            project_path: proj.clone(),
                            start_time: start_time.clone(),
                            created_at: start_time.clone(),
                            last_edited: last_edited.clone(),
                            num_turns,
                            total_cost_usd,
                            preview,
                            title,
                        });
                    }
                }
            }
        }
    }
    // Sort by last_edited (most recent first)
    summaries.sort_by(|a, b| b.last_edited.cmp(&a.last_edited));
    Ok(summaries)
}

fn summarize_lines(lines: &[SessionLine]) -> (u32, f64, String, Option<String>) {
    let mut result_turns: Option<u32> = None;
    let mut result_cost: Option<f64> = None;
    let mut user_turn_count = 0u32;
    let mut preview = String::new();
    let mut preview_set = false;
    let mut title: Option<String> = None;
    for line in lines {
        match line {
            SessionLine::UserMessage { content, .. } => {
                user_turn_count += 1;
                if !preview_set {
                    let first_text = content
                        .first()
                        .and_then(|c: &serde_json::Value| c.get("text"));
                    if let Some(s) = first_text.and_then(|v: &serde_json::Value| v.as_str()) {
                        let trimmed = s.trim();
                        preview = trimmed.chars().take(80).collect::<String>();
                        if trimmed.chars().count() > 80 {
                            preview.push('…');
                        }
                        preview_set = true;
                    }
                }
            }
            SessionLine::Result {
                num_turns: n,
                total_cost_usd: c,
                ..
            } => {
                result_turns = Some(*n);
                result_cost = Some(*c);
            }
            SessionLine::Title { title: t } => {
                title = Some(t.clone());
            }
            _ => {}
        }
    }
    let num_turns = result_turns.unwrap_or(user_turn_count);
    let total_cost_usd = result_cost.unwrap_or(0.0);
    (num_turns, total_cost_usd, preview, title)
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
    fn session_writer_end_offset_and_truncate_to() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "trunc-test";
        let mut w = SessionWriter::create(project_path, id).unwrap();
        let after_meta = w.end_offset().unwrap();
        w.write_line(&SessionLine::UserMessage {
            role: "user".into(),
            content: vec![serde_json::json!({"type": "text", "text": "a"})],
        })
        .unwrap();
        let after_user = w.end_offset().unwrap();
        assert!(after_user > after_meta);
        w.truncate_to(after_meta).unwrap();
        drop(w);
        let lines = SessionReader::load(project_path, id).unwrap();
        assert_eq!(lines.len(), 1, "user line should be gone");
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

    #[test]
    fn stale_file_records_skips_non_path_results() {
        let lines = vec![
            SessionLine::ToolResult {
                tool_use_id: "y".into(),
                content: "output".into(),
                is_error: false,
                duration_ms: None,
                path: None, // no path
                content_hash: None,
                mtime_nanos: None,
            },
            SessionLine::UserMessage {
                role: "user".into(),
                content: vec![],
            },
        ];
        let records = SessionReader::stale_file_records(&lines);
        assert!(records.is_empty());
    }

    #[test]
    fn session_writer_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "append-test";
        {
            let mut w = SessionWriter::create(project_path, id).unwrap();
            w.write_line(&SessionLine::UserMessage {
                role: "user".into(),
                content: vec![serde_json::json!({"type": "text", "text": "hello"})],
            })
            .unwrap();
        }
        // Now append
        {
            let mut w = SessionWriter::append(project_path, id).unwrap();
            w.write_line(&SessionLine::AssistantMessage {
                content: vec![serde_json::json!({"type": "text", "text": "world"})],
            })
            .unwrap();
        }
        let lines = SessionReader::load(project_path, id).unwrap();
        assert!(lines.len() >= 3); // meta + user + assistant
        assert!(lines
            .iter()
            .any(|l| matches!(l, SessionLine::AssistantMessage { .. })));
    }

    #[test]
    fn summarize_lines_with_result_line() {
        let lines = [
            SessionLine::Meta {
                session_id: "s1".into(),
                schema_version: SCHEMA_VERSION,
                start_time: "2026-01-01T00:00:00Z".into(),
                project_path: "/tmp/proj".into(),
            },
            SessionLine::UserMessage {
                role: "user".into(),
                content: vec![serde_json::json!({"type": "text", "text": "What is 2+2?"})],
            },
            SessionLine::Result {
                exit_status: "success".into(),
                total_cost_usd: 0.05,
                num_turns: 1,
                duration_ms: 1234,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "summarize-test";
        {
            let mut w = SessionWriter::create(project_path, id).unwrap();
            for line in &lines[1..] {
                w.write_line(line).unwrap();
            }
        }
        let loaded = SessionReader::load(project_path, id).unwrap();
        let sessions = list_sessions(project_path).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, id);
        assert!((sessions[0].total_cost_usd - 0.05).abs() < 0.001);
        assert_eq!(sessions[0].num_turns, 1);
        assert!(sessions[0].preview.contains("2+2"));
        let _ = loaded;
    }

    #[test]
    fn list_sessions_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = list_sessions(dir.path()).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_nonexistent_dir() {
        let result = list_sessions(std::path::Path::new("/this/path/does/not/exist/xyz"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn stale_paths_with_nonexistent_file() {
        let records = vec![StaleFileRecord {
            path: "/nonexistent/file/xyz.txt".into(),
            content_hash: "abc".into(),
            mtime_nanos: 0,
        }];
        let stale = stale_paths(&records);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], "/nonexistent/file/xyz.txt");
    }

    #[test]
    fn stale_paths_with_matching_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test_stale.txt");
        std::fs::write(&file_path, "hello").unwrap();
        // Get current mtime and hash
        use sha2::{Digest, Sha256};
        let content = std::fs::read_to_string(&file_path).unwrap();
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        let meta = std::fs::metadata(&file_path).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let records = vec![StaleFileRecord {
            path: file_path.to_string_lossy().to_string(),
            content_hash: hash,
            mtime_nanos: mtime,
        }];
        let stale = stale_paths(&records);
        assert!(
            stale.is_empty(),
            "file should not be stale when hash and mtime match"
        );
    }

    #[test]
    fn session_line_result_roundtrip() {
        let line = SessionLine::Result {
            exit_status: "success".into(),
            total_cost_usd: 0.01,
            num_turns: 5,
            duration_ms: 3000,
        };
        let json = serde_json::to_string(&line).unwrap();
        let back: SessionLine = serde_json::from_str(&json).unwrap();
        match back {
            SessionLine::Result {
                exit_status,
                num_turns,
                total_cost_usd,
                duration_ms,
            } => {
                assert_eq!(exit_status, "success");
                assert_eq!(num_turns, 5);
                assert!((total_cost_usd - 0.01).abs() < 0.001);
                assert_eq!(duration_ms, 3000);
            }
            _ => panic!("expected Result variant"),
        }
    }

    #[test]
    fn session_line_system_roundtrip() {
        let line = SessionLine::System {
            subtype: "compact".into(),
            message: Some("history compacted".into()),
        };
        let json = serde_json::to_string(&line).unwrap();
        let back: SessionLine = serde_json::from_str(&json).unwrap();
        match back {
            SessionLine::System { subtype, message } => {
                assert_eq!(subtype, "compact");
                assert_eq!(message.as_deref(), Some("history compacted"));
            }
            _ => panic!("expected System variant"),
        }
    }

    /// Line 142: empty lines in session file are skipped.
    #[test]
    fn session_reader_skips_empty_lines() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "empty-line-test";
        // Create session manually with blank lines
        let mut w = SessionWriter::create(project_path, id).unwrap();
        // Write a user message, then manually add blank lines to file
        w.write_line(&SessionLine::UserMessage {
            role: "user".into(),
            content: vec![serde_json::json!({"type": "text", "text": "hello"})],
        })
        .unwrap();
        drop(w);
        // Append a blank line directly
        let path = paths::session_file_path(project_path, id).unwrap();
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap();
        writeln!(file, "   ").unwrap();
        drop(file);
        // Loading should succeed and not include the blank lines
        let lines = SessionReader::load(project_path, id).unwrap();
        assert!(lines.len() >= 2); // meta + user message (blank lines skipped)
    }

    /// Line 148: schema_version too new returns error.
    #[test]
    fn session_reader_rejects_future_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "future-schema-test";
        // Write a session file with schema_version = SCHEMA_VERSION + 1
        let future_meta = SessionLine::Meta {
            session_id: id.into(),
            schema_version: SCHEMA_VERSION + 1,
            start_time: "2026-01-01T00:00:00Z".into(),
            project_path: project_path.to_string_lossy().to_string(),
        };
        let path = paths::session_file_path(project_path, id).unwrap();
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        use std::io::Write as _;
        writeln!(f, "{}", serde_json::to_string(&future_meta).unwrap()).unwrap();
        drop(f);
        let result = SessionReader::load(project_path, id);
        assert!(result.is_err(), "expected error for future schema version");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("newer than supported"), "got: {}", msg);
    }

    /// stale_paths supports binary files by hashing raw bytes.
    #[cfg(unix)]
    #[test]
    fn stale_paths_binary_file_with_matching_hash_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        // Non-UTF8 binary bytes.
        let file_path = dir.path().join("binary.bin");
        let bytes = vec![0xFF, 0xFE, 0x00, 0x01, 0xAA];
        std::fs::write(&file_path, &bytes).unwrap();
        let meta = std::fs::metadata(&file_path).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let hash = hex::encode(Sha256::digest(&bytes));
        let records = vec![StaleFileRecord {
            path: file_path.to_string_lossy().to_string(),
            content_hash: hash,
            mtime_nanos: mtime,
        }];
        let stale = stale_paths(&records);
        assert!(stale.is_empty(), "matching binary file should not be stale");
    }

    /// Line 205: stale_paths detects hash mismatch even when mtime matches.
    #[test]
    fn stale_paths_hash_mismatch_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("changed.txt");
        std::fs::write(&file_path, "current content").unwrap();
        let meta = std::fs::metadata(&file_path).unwrap();
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let records = vec![StaleFileRecord {
            path: file_path.to_string_lossy().to_string(),
            content_hash: "wrong_hash_that_does_not_match".into(),
            mtime_nanos: mtime, // same mtime but wrong hash
        }];
        let stale = stale_paths(&records);
        assert_eq!(stale.len(), 1, "hash mismatch should report stale");
    }

    /// Line 237: sessions with no user turns are skipped in list_sessions.
    #[test]
    fn list_sessions_skips_zero_turn_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "empty-session";
        // Create a session with no user messages (just meta)
        let _w = SessionWriter::create(project_path, id).unwrap();
        drop(_w);
        let sessions = list_sessions(project_path).unwrap();
        assert!(sessions.is_empty(), "zero-turn sessions should be skipped");
    }

    /// Line 274: preview is truncated with ellipsis for text > 80 chars.
    #[test]
    fn summarize_lines_preview_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "long-preview-test";
        let long_text = "a".repeat(90); // 90 chars
        let mut w = SessionWriter::create(project_path, id).unwrap();
        w.write_line(&SessionLine::UserMessage {
            role: "user".into(),
            content: vec![serde_json::json!({"type": "text", "text": long_text})],
        })
        .unwrap();
        drop(w);
        let sessions = list_sessions(project_path).unwrap();
        assert_eq!(sessions.len(), 1);
        // Preview should be truncated with ellipsis
        assert!(
            sessions[0].preview.contains('…'),
            "expected ellipsis in truncated preview"
        );
        assert!(sessions[0].preview.len() <= 100); // should be shorter than 90 chars
    }

    #[test]
    fn session_writer_write_impl() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();
        let id = "write-impl-test";
        let mut w = SessionWriter::create(project_path, id).unwrap();
        // Test the Write trait impl
        use std::io::Write;
        w.write_all(b"raw bytes").unwrap();
        w.flush().unwrap();
    }

    #[test]
    fn redact_secrets_replaces_anthropic_key() {
        let s = r#"{"command":"echo sk-ant-api03-abcXYZ123"}"#;
        let r = redact_secrets(s);
        assert!(!r.contains("abcXYZ123"), "API key value should be redacted");
        assert!(
            r.contains("sk-ant-"),
            "prefix should remain for readability"
        );
        assert!(r.contains("[REDACTED]"));
    }

    #[test]
    fn redact_secrets_replaces_aws_key() {
        let s = r#"{"output":"AKIAIOSFODNN7EXAMPLE rest of text"}"#;
        let r = redact_secrets(s);
        assert!(
            !r.contains("IOSFODNN7EXAMPLE"),
            "AWS key should be redacted"
        );
        assert!(r.contains("[REDACTED]"));
    }

    #[test]
    fn redact_secrets_replaces_openrouter_key() {
        let s = r#"{"key":"sk-or-v1-deadbeefcafe1234"}"#;
        let r = redact_secrets(s);
        assert!(!r.contains("deadbeefcafe1234"));
        assert!(r.contains("[REDACTED]"));
    }

    #[test]
    fn redact_secrets_replaces_generic_sk_key() {
        let s = r#"{"token":"sk-legacy-abcdef123456"}"#;
        let r = redact_secrets(s);
        assert!(!r.contains("legacy-abcdef123456"));
        assert!(r.contains("sk-[REDACTED]"));
    }

    #[test]
    fn redact_secrets_does_not_double_redact_provider_specific_sk_key() {
        let s = r#"{"token":"sk-ant-api03-secretvalue123"}"#;
        let r = redact_secrets(s);
        assert_eq!(r.matches("[REDACTED]").count(), 1);
        assert!(r.contains("sk-ant-[REDACTED]"));
    }

    #[test]
    fn redact_secrets_noop_on_clean_input() {
        let s = r#"{"tool_name":"Read","input":{"file_path":"src/main.rs"}}"#;
        let r = redact_secrets(s);
        assert_eq!(r, s, "clean input should be unchanged");
    }

    #[test]
    fn redact_secrets_multiple_keys_in_one_string() {
        let s = "sk-ant-key1 and sk-or-key2";
        let r = redact_secrets(s);
        assert!(!r.contains("key1"));
        assert!(!r.contains("key2"));
        assert_eq!(r.matches("[REDACTED]").count(), 2);
    }
}
