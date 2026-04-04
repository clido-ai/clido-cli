//! Slash command registry: data-driven command definitions with metadata.
//!
//! Replaces the inline `SLASH_COMMAND_SECTIONS` constant in tui.rs.
//! Commands are described declaratively; the autocomplete system and `/help`
//! output derive from this single source of truth.

/// A single slash command definition.
#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    /// The command string including `/`, e.g. "/model".
    pub name: &'static str,
    /// Section header for grouping in help output.
    pub section: &'static str,
    /// Short description shown in autocomplete and /help.
    pub description: &'static str,
    /// Usage hint (shown when command is misused), e.g. "/model [name]".
    pub usage: Option<&'static str>,
    /// Whether this command accepts arguments.
    pub takes_args: bool,
    /// Whether this command requires the agent to be idle (not busy).
    pub requires_idle: bool,
}

/// All available slash commands, grouped by section.
///
/// This is the single source of truth for command metadata.
/// `execute_slash()` in tui.rs handles the actual logic via match.
pub static COMMANDS: &[SlashCommand] = &[
    // ── Session ──────────────────────────────────────────────────
    SlashCommand {
        name: "/clear",
        section: "Session",
        description: "clear the conversation",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/help",
        section: "Session",
        description: "show key bindings and all slash commands",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/keys",
        section: "Session",
        description: "show keyboard shortcuts (overlay)",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/quit",
        section: "Session",
        description: "exit clido",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/sessions",
        section: "Session",
        description: "list & resume recent sessions",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/session",
        section: "Session",
        description: "show current session ID",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/search",
        section: "Session",
        description: "search conversation history",
        usage: Some("/search <query>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/export",
        section: "Session",
        description: "save this conversation to a markdown file",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/init",
        section: "Session",
        description: "reconfigure the current profile — opens in-TUI profile editor",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/note",
        section: "Session",
        description: "add a side note visible in the session but not sent to the agent",
        usage: Some("/note <text>"),
        takes_args: true,
        requires_idle: false,
    },
    // ── Settings ─────────────────────────────────────────────────
    SlashCommand {
        name: "/config",
        section: "Settings",
        description: "show all settings — provider, model, roles, agent, context",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/configure",
        section: "Settings",
        description: "change settings with natural language",
        usage: Some("/configure <intent>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/settings",
        section: "Settings",
        description: "open settings editor — manage roles and default model",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/enhance",
        section: "Settings",
        description: "enhance your prompt — review and edit before sending",
        usage: Some("/enhance <prompt>"),
        takes_args: true,
        requires_idle: false,
    },
    // ── Git ──────────────────────────────────────────────────────
    SlashCommand {
        name: "/ship",
        section: "Git",
        description: "stage → commit → push",
        usage: Some("/ship [message]"),
        takes_args: true,
        requires_idle: true,
    },
    SlashCommand {
        name: "/save",
        section: "Git",
        description: "stage → commit locally, no push",
        usage: Some("/save [message]"),
        takes_args: true,
        requires_idle: true,
    },
    SlashCommand {
        name: "/pr",
        section: "Git",
        description: "create a pull request",
        usage: Some("/pr [title]"),
        takes_args: true,
        requires_idle: true,
    },
    SlashCommand {
        name: "/branch",
        section: "Git",
        description: "create + switch to a new branch",
        usage: Some("/branch <name>"),
        takes_args: true,
        requires_idle: true,
    },
    SlashCommand {
        name: "/undo",
        section: "Git",
        description: "undo last commit safely (confirm before reset)",
        usage: None,
        takes_args: false,
        requires_idle: true,
    },
    SlashCommand {
        name: "/rollback",
        section: "Git",
        description: "restore to a checkpoint",
        usage: Some("/rollback [id]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/sync",
        section: "Git",
        description: "pull --rebase from upstream, resolve conflicts if needed",
        usage: None,
        takes_args: false,
        requires_idle: true,
    },
    // ── Model ────────────────────────────────────────────────────
    SlashCommand {
        name: "/models",
        section: "Model",
        description: "open interactive model picker (search, filter, favorites)",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/model",
        section: "Model",
        description: "show or switch model",
        usage: Some("/model [model-name]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/fast",
        section: "Model",
        description: "switch to fast (cheap) model for this session",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/smart",
        section: "Model",
        description: "switch to smart (powerful) model for this session",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/fav",
        section: "Model",
        description: "mark or unmark current model as a favorite",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/reviewer",
        section: "Model",
        description: "show or toggle reviewer",
        usage: Some("/reviewer [on|off]"),
        takes_args: true,
        requires_idle: false,
    },
    // ── Context ──────────────────────────────────────────────────
    SlashCommand {
        name: "/cost",
        section: "Context",
        description: "show total cost for this session",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/tokens",
        section: "Context",
        description: "show token usage for this session",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/compact",
        section: "Context",
        description: "compress context window now (summarises history)",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/memory",
        section: "Context",
        description: "search saved memories",
        usage: Some("/memory [query]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/todo",
        section: "Context",
        description: "show the agent's current task list",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    // ── Plan ─────────────────────────────────────────────────────
    SlashCommand {
        name: "/plan",
        section: "Plan",
        description: "show current plan, or /plan <task> to have agent plan first",
        usage: Some("/plan [task|edit|save|list|view|text|raw]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/plan edit",
        section: "Plan",
        description: "open plan editor for the current plan",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/plan save",
        section: "Plan",
        description: "save current plan to .clido/plans/",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/plan list",
        section: "Plan",
        description: "list all saved plans",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    // ── Workflow ─────────────────────────────────────────────────
    SlashCommand {
        name: "/workflow",
        section: "Workflow",
        description: "list workflows, or /workflow new <desc> to create one with AI guidance",
        usage: Some("/workflow [new|list|show|edit|save|run] [name|description]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow new",
        section: "Workflow",
        description: "create a new workflow with guided AI assistance",
        usage: Some("/workflow new <description of what the workflow should do>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow list",
        section: "Workflow",
        description: "list all saved workflows",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow show",
        section: "Workflow",
        description: "display a workflow's YAML in the chat",
        usage: Some("/workflow show <name>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow edit",
        section: "Workflow",
        description: "open a workflow in the text editor (or last draft from chat)",
        usage: Some("/workflow edit [name]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow save",
        section: "Workflow",
        description: "save the last workflow YAML from chat to .clido/workflows/",
        usage: Some("/workflow save [name]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workflow run",
        section: "Workflow",
        description: "run a saved workflow",
        usage: Some("/workflow run <name>"),
        takes_args: true,
        requires_idle: false,
    },
    // ── Project ──────────────────────────────────────────────────
    SlashCommand {
        name: "/agents",
        section: "Project",
        description: "show current agent configuration (main + fast provider)",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/profiles",
        section: "Project",
        description: "list all profiles with active model per slot",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/profile",
        section: "Project",
        description: "open profile picker — switch, create, or edit profiles",
        usage: Some("/profile [name|new|edit [name]]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/profile new",
        section: "Project",
        description: "create a new profile — opens an in-TUI wizard",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/profile edit",
        section: "Project",
        description: "edit a profile in the TUI overview",
        usage: Some("/profile edit [name]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/profile delete",
        section: "Project",
        description: "delete a profile (cannot delete active profile)",
        usage: Some("/profile delete <name>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/check",
        section: "Project",
        description: "run diagnostics on current project",
        usage: None,
        takes_args: false,
        requires_idle: true,
    },
    SlashCommand {
        name: "/rules",
        section: "Project",
        description: "show active project rules files (CLIDO.md)",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/image",
        section: "Project",
        description: "attach an image to the next message",
        usage: Some("/image <path>"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/workdir",
        section: "Project",
        description: "show or set working directory",
        usage: Some("/workdir [path]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/stop",
        section: "Project",
        description: "interrupt current run without sending a message",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    SlashCommand {
        name: "/copy",
        section: "Project",
        description: "copy to clipboard",
        usage: Some("/copy [all|<n>]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/notify",
        section: "Project",
        description: "toggle desktop notifications on/off",
        usage: Some("/notify [on|off]"),
        takes_args: true,
        requires_idle: false,
    },
    SlashCommand {
        name: "/index",
        section: "Project",
        description: "show codebase index stats",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
    // ── Update ───────────────────────────────────────────────────
    SlashCommand {
        name: "/update",
        section: "Update",
        description: "check for updates and install the latest version",
        usage: None,
        takes_args: false,
        requires_idle: false,
    },
];

// ── Registry helpers ──────────────────────────────────────────────────────────

/// Unique section names in display order.
pub fn sections() -> Vec<&'static str> {
    let mut seen = Vec::new();
    for cmd in COMMANDS {
        if !seen.contains(&cmd.section) {
            seen.push(cmd.section);
        }
    }
    seen
}

/// Commands grouped by section: `(section_label, &[SlashCommand])`.
pub fn commands_by_section() -> Vec<(&'static str, Vec<&'static SlashCommand>)> {
    let secs = sections();
    secs.into_iter()
        .map(|s| {
            let cmds: Vec<&SlashCommand> = COMMANDS.iter().filter(|c| c.section == s).collect();
            (s, cmds)
        })
        .collect()
}

/// Flat list of (name, description) — drop-in replacement for old
/// `slash_commands()` in tui.rs.
pub fn flat_commands() -> Vec<(&'static str, &'static str)> {
    COMMANDS.iter().map(|c| (c.name, c.description)).collect()
}

/// Autocomplete: return commands whose name starts with `prefix`.
pub fn completions(prefix: &str) -> Vec<&'static SlashCommand> {
    if !prefix.starts_with('/') {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(prefix))
        .collect()
}

/// Look up a command by exact name match.
pub fn find(name: &str) -> Option<&'static SlashCommand> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// Build the grouped help rows used by the autocomplete popup.
/// Returns `(section_header_or_empty, cmd_name, description)` triples.
pub fn completion_rows(prefix: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let matches = completions(prefix);
    if matches.is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::new();
    let mut last_section = "";
    for cmd in &matches {
        if cmd.section != last_section {
            last_section = cmd.section;
            rows.push((cmd.section, "", ""));
        }
        rows.push(("", cmd.name, cmd.description));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_commands_start_with_slash() {
        for cmd in COMMANDS {
            assert!(
                cmd.name.starts_with('/'),
                "Command '{}' doesn't start with /",
                cmd.name
            );
        }
    }

    #[test]
    fn no_duplicate_names() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        names.sort();
        for w in names.windows(2) {
            assert_ne!(w[0], w[1], "Duplicate command: {}", w[0]);
        }
    }

    #[test]
    fn all_commands_have_description() {
        for cmd in COMMANDS {
            assert!(
                !cmd.description.is_empty(),
                "Command '{}' has empty description",
                cmd.name
            );
        }
    }

    #[test]
    fn sections_are_stable() {
        let secs = sections();
        assert!(secs.contains(&"Session"));
        assert!(secs.contains(&"Git"));
        assert!(secs.contains(&"Model"));
    }

    #[test]
    fn completions_filter_by_prefix() {
        let matches = completions("/mod");
        assert!(matches.iter().any(|c| c.name == "/model"));
        assert!(matches.iter().any(|c| c.name == "/models"));
        assert!(matches.iter().all(|c| c.name.starts_with("/mod")));
    }

    #[test]
    fn completions_empty_for_non_slash() {
        assert!(completions("model").is_empty());
    }

    #[test]
    fn find_returns_correct_command() {
        let cmd = find("/clear").unwrap();
        assert_eq!(cmd.section, "Session");
        assert!(!cmd.takes_args);
    }

    #[test]
    fn find_returns_none_for_unknown() {
        assert!(find("/nonexistent").is_none());
    }

    #[test]
    fn flat_commands_matches_registry_size() {
        assert_eq!(flat_commands().len(), COMMANDS.len());
    }

    #[test]
    fn commands_by_section_groups_correctly() {
        let grouped = commands_by_section();
        let total: usize = grouped.iter().map(|(_, cmds)| cmds.len()).sum();
        assert_eq!(total, COMMANDS.len());
    }

    #[test]
    fn completion_rows_include_headers() {
        let rows = completion_rows("/plan");
        // Should have at least one section header row
        assert!(rows.iter().any(|(s, _, _)| !s.is_empty()));
        // Should have command rows
        assert!(rows.iter().any(|(_, n, _)| *n == "/plan"));
    }

    #[test]
    fn completions_exact_match_help() {
        let matches = completions("/help");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "/help");
    }

    #[test]
    fn completions_prefix_h_returns_all_h_commands() {
        let matches = completions("/h");
        assert!(!matches.is_empty());
        for cmd in &matches {
            assert!(
                cmd.name.starts_with("/h"),
                "Expected /h prefix, got {}",
                cmd.name
            );
        }
        // /help should be among results
        assert!(matches.iter().any(|c| c.name == "/help"));
    }

    #[test]
    fn completions_no_match_returns_empty() {
        assert!(completions("/xyz").is_empty());
    }

    #[test]
    fn completions_empty_input_returns_empty() {
        assert!(completions("").is_empty());
    }

    #[test]
    fn completions_model_matches_model_and_models() {
        let matches = completions("/model");
        let names: Vec<&str> = matches.iter().map(|c| c.name).collect();
        assert!(names.contains(&"/model"), "should contain /model");
        assert!(names.contains(&"/models"), "should contain /models");
    }
}
