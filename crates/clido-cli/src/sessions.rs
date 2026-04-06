//! Session list and show commands.

use clido_agent::try_session_lines_to_messages;
use clido_storage::{list_sessions, SessionReader, SessionWriter};
use std::env;
use std::io::{self, IsTerminal};

pub fn is_stdin_tty() -> bool {
    io::stdin().is_terminal()
}

pub async fn run_sessions_list() -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sessions = list_sessions(&cwd)?;
    if sessions.is_empty() {
        println!("No sessions yet. Run 'clido <prompt>' to start one.");
        return Ok(());
    }
    for s in sessions {
        let (head, tail) = if s.session_id.len() > 12 {
            (&s.session_id[..8], &s.session_id[s.session_id.len() - 4..])
        } else {
            (s.session_id.as_str(), "")
        };
        let short_id = format!("{}...{}", head, tail);
        println!(
            "{}  {}  turns: {}  cost: ${:.4}  {}",
            short_id, s.start_time, s.num_turns, s.total_cost_usd, s.preview
        );
    }
    Ok(())
}

pub async fn run_sessions_show(id: &str) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let lines = SessionReader::load(&cwd, id)?;
    for line in lines {
        println!("{}", serde_json::to_string(&line)?);
    }
    Ok(())
}

pub async fn run_sessions_verify(id: &str) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let lines = SessionReader::load(&cwd, id)?;
    let msgs = try_session_lines_to_messages(&lines)
        .map_err(|e| anyhow::anyhow!("session {id}: strict load failed — {e}"))?;
    println!(
        "OK: session {} strict-loads as {} message(s).",
        id,
        msgs.len()
    );
    Ok(())
}

pub async fn run_sessions_fork(id: &str) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let lines = SessionReader::load(&cwd, id)?;
    let new_id = uuid::Uuid::new_v4().to_string();
    let mut writer = SessionWriter::create(&cwd, &new_id)?;
    for line in &lines {
        writer.write_line(line)?;
    }
    println!("{}", new_id);
    Ok(())
}
