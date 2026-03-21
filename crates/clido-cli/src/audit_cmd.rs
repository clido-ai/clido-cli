//! clido audit — view audit log.

use clido_storage::AuditEntry;
use std::{env, io::BufRead};

pub fn run_audit(
    tail: Option<usize>,
    session: Option<&str>,
    tool: Option<&str>,
    since: Option<&str>,
    json: bool,
) -> Result<(), anyhow::Error> {
    let workspace_root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let audit_path = clido_storage::audit_log_path(&workspace_root)?;

    if !audit_path.exists() {
        if json {
            println!("[]");
        } else {
            println!("No audit log found.");
        }
        return Ok(());
    }

    let file = std::fs::File::open(&audit_path)?;
    let reader = std::io::BufReader::new(file);
    let mut entries: Vec<AuditEntry> = reader
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();

    // Apply filters
    if let Some(sess) = session {
        entries.retain(|e| e.session_id.contains(sess));
    }
    if let Some(t) = tool {
        entries.retain(|e| e.tool_name.to_lowercase().contains(&t.to_lowercase()));
    }
    if let Some(s) = since {
        entries.retain(|e| e.timestamp.as_str() >= s);
    }
    if let Some(n) = tail {
        let start = entries.len().saturating_sub(n);
        entries = entries[start..].to_vec();
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        if entries.is_empty() {
            println!("No audit entries match.");
            return Ok(());
        }
        println!("{:<20}  {:<12}  {:>6}  {}", "time", "tool", "ms", "input");
        println!("{}", "-".repeat(72));
        for e in &entries {
            let time = if e.timestamp.len() >= 16 {
                format!("{} {}", &e.timestamp[5..10], &e.timestamp[11..16])
            } else {
                e.timestamp.clone()
            };
            let status = if e.is_error { "✗" } else { "✓" };
            let input = e.input_summary.chars().take(36).collect::<String>();
            println!(
                "{} {:<19}  {:<12}  {:>6}  {}",
                status, time, e.tool_name, e.duration_ms, input
            );
        }
    }
    Ok(())
}
