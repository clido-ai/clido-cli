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

pub async fn run_search(query: &str, session_id: Option<&str>) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let id = match session_id {
        Some(s) => s.to_string(),
        None => {
            // Find most recent session
            let sessions = list_sessions(&cwd)?;
            sessions
                .first()
                .map(|s| s.session_id.clone())
                .ok_or_else(|| anyhow::anyhow!("No sessions found"))?
        }
    };

    let lines = SessionReader::load(&cwd, &id)?;
    let query_lower = query.to_lowercase();

    for line in &lines {
        let text = match line {
            clido_storage::SessionLine::UserMessage { content, .. } => content
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" "),
            clido_storage::SessionLine::AssistantMessage { content, .. } => content
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };

        if text.to_lowercase().contains(&query_lower) {
            println!("{}", text);
        }
    }
    Ok(())
}

pub async fn run_export(
    session_id: Option<&str>,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let id = match session_id {
        Some(s) => s.to_string(),
        None => {
            let sessions = list_sessions(&cwd)?;
            sessions
                .first()
                .map(|s| s.session_id.clone())
                .ok_or_else(|| anyhow::anyhow!("No sessions found"))?
        }
    };

    let lines = SessionReader::load(&cwd, &id)?;
    let mut markdown = format!("# Session {}\n\n", id);

    for line in &lines {
        match line {
            clido_storage::SessionLine::UserMessage { content, .. } => {
                let text = content
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                markdown.push_str(&format!("## User\n\n{}\n\n", text));
            }
            clido_storage::SessionLine::AssistantMessage { content, .. } => {
                let text = content
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                markdown.push_str(&format!("## Assistant\n\n{}\n\n", text));
            }
            _ => {}
        }
    }

    match output {
        Some(path) => {
            std::fs::write(path, markdown)?;
            println!("Exported to {}", path.display());
        }
        None => {
            println!("{}", markdown);
        }
    }
    Ok(())
}

pub async fn run_compact(_session_id: Option<&str>) -> anyhow::Result<()> {
    println!("Compacting session... (placeholder - requires agent context)");
    Ok(())
}

pub async fn run_copy(all: bool, session_id: Option<&str>) -> anyhow::Result<()> {
    let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let id = match session_id {
        Some(s) => s.to_string(),
        None => {
            let sessions = list_sessions(&cwd)?;
            sessions
                .first()
                .map(|s| s.session_id.clone())
                .ok_or_else(|| anyhow::anyhow!("No sessions found"))?
        }
    };

    let lines = SessionReader::load(&cwd, &id)?;
    let mut text = String::new();

    for line in &lines {
        match line {
            clido_storage::SessionLine::UserMessage { content, .. } if all => {
                let t = content
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                text.push_str(&format!("User: {}\n", t));
            }
            clido_storage::SessionLine::AssistantMessage { content, .. } => {
                let t = content
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                text.push_str(&format!("Assistant: {}\n", t));
            }
            _ => {}
        }
    }

    // Copy to clipboard
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
        if let Some(stdin) = child.stdin.take() {
            use std::io::Write;
            let mut stdin = stdin;
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
    }
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .or_else(|_| {
                Command::new("xclip")
                    .arg("-selection")
                    .arg("clipboard")
                    .stdin(Stdio::piped())
                    .spawn()
            })?;
        if let Some(stdin) = child.stdin.take() {
            use std::io::Write;
            let mut stdin = stdin;
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
    }

    println!("Copied to clipboard");
    Ok(())
}

pub async fn run_todo(_session_id: Option<&str>) -> anyhow::Result<()> {
    println!("Todo list: (placeholder - requires agent context)");
    Ok(())
}
