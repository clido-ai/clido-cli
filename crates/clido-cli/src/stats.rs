//! clido stats — session statistics.

use clido_storage::{list_sessions, SessionLine, SessionReader, SessionSummary};
use std::env;

pub fn run_stats(session_id: Option<&str>, json: bool) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let sessions: Vec<SessionSummary> = if let Some(id) = session_id {
        // Load specific session
        match SessionReader::load(&workspace_root, id) {
            Ok(lines) => {
                // Build a fake summary from the lines
                let mut msgs = 0u32;
                let mut cost = 0.0f64;
                let mut preview = String::new();
                let mut start_time = String::new();
                for line in &lines {
                    match line {
                        SessionLine::Meta { start_time: st, .. } => {
                            start_time = st.clone();
                        }
                        SessionLine::UserMessage { content, .. } => {
                            msgs += 1;
                            if preview.is_empty() {
                                if let Some(t) = content
                                    .first()
                                    .and_then(|c| c.get("text"))
                                    .and_then(|v| v.as_str())
                                {
                                    preview = t.chars().take(60).collect();
                                }
                            }
                        }
                        SessionLine::Result {
                            total_cost_usd,
                            num_turns,
                            ..
                        } => {
                            cost = *total_cost_usd;
                            msgs = *num_turns; // prefer Result line if present
                        }
                        _ => {}
                    }
                }
                vec![SessionSummary {
                    session_id: id.to_string(),
                    project_path: workspace_root.display().to_string(),
                    start_time,
                    num_turns: msgs,
                    total_cost_usd: cost,
                    preview,
                    title: None,
                }]
            }
            Err(e) => return Err(anyhow::anyhow!("session not found: {}", e)),
        }
    } else {
        list_sessions(&workspace_root)?
    };

    if json {
        let arr: Vec<serde_json::Value> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "session_id": s.session_id,
                    "start_time": s.start_time,
                    "num_turns": s.num_turns,
                    "total_cost_usd": s.total_cost_usd,
                    "preview": s.preview,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }
        println!("{:<36}  {:>4}  {:>8}  preview", "session", "msgs", "cost");
        println!("{}", "-".repeat(80));
        for s in &sessions {
            let date = if s.start_time.len() >= 16 {
                format!("{} {}", &s.start_time[5..10], &s.start_time[11..16])
            } else {
                s.start_time.clone()
            };
            println!(
                "{:<36}  {:>4}  ${:>7.4}  {}",
                format!("{}  {}", &s.session_id[..s.session_id.len().min(8)], date),
                s.num_turns,
                s.total_cost_usd,
                s.preview.chars().take(40).collect::<String>()
            );
        }
        let total_cost: f64 = sessions.iter().map(|s| s.total_cost_usd).sum();
        let total_turns: u32 = sessions.iter().map(|s| s.num_turns).sum();
        println!("{}", "-".repeat(80));
        println!(
            "Total: {} sessions  {} msgs  ${:.4}",
            sessions.len(),
            total_turns,
            total_cost
        );
    }
    Ok(())
}
