//! Agent instruction text embedded at compile time (single source for defaults).

const DEFAULT_AGENT_BASE: &str = include_str!("default_agent_base.txt");
const WORKFLOW_PHASES: &str = include_str!("workflow_phases.txt");
const ARCHITECT_TEMPLATE: &str = include_str!("architect.txt");

/// Default system prompt body when the user does not supply `--system-prompt` or a file.
pub fn bundled_default_system_prompt() -> String {
    format!(
        "{}\n\n{}",
        DEFAULT_AGENT_BASE.trim_end(),
        WORKFLOW_PHASES.trim(),
    )
}

/// User message sent to the utility model for upfront planning.
pub fn architect_user_prompt(task: &str) -> String {
    ARCHITECT_TEMPLATE.replace("{task}", task)
}

/// Prepended to the tool-result user message when any tool failed — forces recovery behavior.
pub fn tool_failure_recovery_nudge(failures: &[(&str, &str)]) -> String {
    if failures.is_empty() {
        return String::new();
    }
    let body: String = failures
        .iter()
        .map(|(name, err)| {
            let preview: String = err.chars().take(220).collect();
            format!("- `{name}`: {preview}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "[Clido — tool recovery required]\n\
         At least one tool failed. You must NOT end this turn as \"done\" until each failure is handled:\n\
         (1) Diagnose the error (path, permissions, invalid arguments, timeout, environment).\n\
         (2) Change strategy (fix JSON, narrower read, different tool, smaller command).\n\
         (3) Retry with corrected inputs, or explain clearly why work is blocked and what the user must do.\n\
         Failed tools:\n{body}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_prompt_contains_workflow_and_clido() {
        let s = bundled_default_system_prompt();
        assert!(s.contains("clido"));
        assert!(s.contains("Execution workflow"));
        assert!(s.contains("Tool failure protocol"));
    }

    #[test]
    fn architect_prompt_includes_task() {
        let p = architect_user_prompt("fix the bug in foo.rs");
        assert!(p.contains("fix the bug"));
        assert!(p.contains("ARCHITECT"));
    }

    #[test]
    fn recovery_nudge_lists_failures() {
        let n = tool_failure_recovery_nudge(&[("Read", "ENOENT")]);
        assert!(n.contains("Read"));
        assert!(n.contains("ENOENT"));
    }
}
