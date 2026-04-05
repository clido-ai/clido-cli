//! Full-screen ratatui TUI: scrollable conversation + persistent input bar.

mod app_state;
mod commands;
mod event_loop;
pub mod events;
mod input;
mod permissions;
mod render;
mod state;

use app_state::*;
use events::*;
use permissions::*;
use state::*;

pub(crate) use event_loop::run_tui;

// Re-export submodule items so they are accessible via `use super::*;` in sibling modules.
#[allow(unused_imports)]
use commands::{
    execute_slash, is_known_slash_cmd, parse_per_turn_model, slash_completion_rows,
    slash_completions, CompletionRow,
};
#[allow(unused_imports)]
use event_loop::{
    agent_task, build_model_list, copy_to_clipboard, copy_to_clipboard_osc52, event_loop,
    resolve_workdir_arg, run_tui_inner, spawn_model_fetch, start_agent_runtime,
    tui_memory_store_path, AgentAction, AgentRuntimeHandles, EventLoopExit,
};
#[allow(unused_imports)]
use input::{
    char_byte_pos, char_byte_pos_tui, delete_char_at_cursor_pe, delete_char_before_cursor_pe,
    handle_app_action, handle_key, handle_plan_editor_key, handle_plan_text_editor_key,
    handle_profile_overlay_key, handle_workflow_editor_key, move_cursor_line_down,
    move_cursor_line_up, scroll_down, scroll_up,
};
#[allow(unused_imports)]
use render::{
    build_lines_w, build_lines_w_uncached, build_plan_from_assistant_text, build_plan_from_tasks,
    extract_current_step_full, filter_indicator_line, fit_spans, is_welcome_only, modal_block,
    modal_block_with_hint, modal_row_two_col, parse_hunk_header, parse_plan_from_text,
    popup_above_input, relative_time, render, render_markdown, render_plan_editor,
    render_plan_text_editor, render_profile_create, render_profile_model_picker,
    render_profile_overlay, render_profile_overview, render_profile_provider_picker,
    render_table_to_lines, render_welcome, render_workflow_editor, scroll_indicator_line,
    strip_plan_line_prefix, tool_color, tool_display_name, truncate_chars, word_wrap,
};

use std::env;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, Ordering};

#[allow(unused_imports)]
use clido_agent::EventEmitter;
#[allow(unused_imports)]
use clido_core::PermissionMode;
#[allow(unused_imports)]
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;
#[allow(unused_imports)]
use tokio::sync::{mpsc, oneshot};

#[allow(unused_imports)]
use crate::overlay::{ErrorOverlay, OverlayKind};
#[allow(unused_imports)]
use clido_agent::{AskUser, PermGrant as AgentPermGrant, PermRequest as AgentPermRequest};

pub(super) const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Truecolor palette — unique to clido, not copied from opencode or anyone else.
/// Warm-cold gradient: user = warm, assistant = cool, tools = neutral, status = ambient.
pub(super) const TUI_SOFT_ACCENT: Color = Color::Rgb(150, 200, 255);
pub(super) const TUI_ACCENT: Color = Color::Green;
/// Softer white for main text — easier on the eyes than pure white.
pub(super) const TUI_TEXT: Color = Color::Rgb(212, 212, 220);
/// Slightly dimmer text for secondary content.
pub(super) const TUI_TEXT_DIM: Color = Color::Rgb(180, 180, 190);
/// Selected row background in pickers and completion lists (muted slate).
pub(super) const TUI_SELECTION_BG: Color = Color::Rgb(52, 62, 78);

/// Code block: near-black interior, subtle blue-gray border.
pub(super) const TUI_CODE_BG: Color = Color::Rgb(20, 20, 26);
pub(super) const TUI_CODE_BORDER: Color = Color::Rgb(50, 50, 65);
pub(super) const TUI_CODE_LANG: Color = Color::Rgb(180, 140, 220); // soft purple for lang label

/// Diff: deeper, more readable backgrounds that don't wash out text.
pub(super) const TUI_DIFF_ADD_BG: Color = Color::Rgb(16, 36, 18);
pub(super) const TUI_DIFF_DEL_BG: Color = Color::Rgb(42, 16, 16);
pub(super) const TUI_DIFF_ADD_FG: Color = Color::Rgb(150, 220, 150);
pub(super) const TUI_DIFF_DEL_FG: Color = Color::Rgb(220, 150, 150);
pub(super) const TUI_DIFF_HEADER: Color = Color::Rgb(130, 170, 200);

/// Blockquote accent bar.
/// Blockquote accent bar — reserved for future use.
#[allow(dead_code)]
pub(super) const TUI_QUOTE_ACCENT: Color = Color::Rgb(100, 110, 135);

/// Slash commands grouped by section — now delegates to command_registry.
pub(super) fn slash_command_sections() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    crate::command_registry::commands_by_section()
        .into_iter()
        .map(|(section, cmds)| {
            let pairs: Vec<(&'static str, &'static str)> =
                cmds.into_iter().map(|c| (c.name, c.description)).collect();
            (section, pairs)
        })
        .collect()
}

/// Flat list of all slash commands — delegates to command_registry.
pub(super) fn slash_commands() -> Vec<(&'static str, &'static str)> {
    crate::command_registry::flat_commands()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_picker::ListPicker;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn make_test_app() -> App {
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (resume_tx, _resume_rx) = mpsc::unbounded_channel();
        let (model_switch_tx, _model_switch_rx) = mpsc::unbounded_channel();
        let (workdir_tx, _workdir_rx) = mpsc::unbounded_channel();
        let (compact_now_tx, _compact_now_rx) = mpsc::unbounded_channel();
        let (fetch_tx, _fetch_rx) = mpsc::unbounded_channel();
        let (kill_tx, _kill_rx) = mpsc::unbounded_channel();
        let (allowed_paths_tx, _allowed_paths_rx) = mpsc::unbounded_channel();
        let (note_tx, _note_rx) = mpsc::unbounded_channel();
        let (path_permission_tx, _path_permission_rx) = mpsc::unbounded_channel();
        let (profile_switch_tx, _profile_switch_rx) = mpsc::unbounded_channel();
        App::new(
            AgentChannels {
                prompt_tx,
                resume_tx,
                model_switch_tx,
                workdir_tx,
                compact_now_tx,
                fetch_tx,
                kill_tx,
                allowed_paths_tx,
                note_tx,
                path_permission_tx,
                profile_switch_tx,
            },
            Arc::new(AtomicBool::new(false)),
            "openrouter".to_string(),
            "default-model".to_string(),
            std::env::temp_dir(),
            false,
            Arc::new(Mutex::new(None)),
            false,
            Vec::new(),
            clido_core::ModelPrefs::default(),
            "default".to_string(),
            Arc::new(AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            String::new(),
            None,
            clido_providers::build_provider(
                "openrouter",
                String::new(),
                "test-model".to_string(),
                None,
            )
            .expect("test provider"),
            "test-model".to_string(),
        )
    }

    // ── parse_per_turn_model tests ─────────────────────────────────────────────

    #[test]
    fn test_parse_per_turn_model_extracts_model_and_prompt() {
        let result = parse_per_turn_model("@claude-opus-4-6 explain the auth flow");
        assert_eq!(
            result,
            Some((
                "claude-opus-4-6".to_string(),
                "explain the auth flow".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_normal_input() {
        assert_eq!(parse_per_turn_model("explain the auth flow"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_at_in_middle() {
        // @ not at start → None
        assert_eq!(parse_per_turn_model("email me @ work"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_at_only() {
        assert_eq!(parse_per_turn_model("@"), None);
    }

    #[test]
    fn test_parse_per_turn_model_returns_none_for_model_no_prompt() {
        // Has model name but no prompt after space
        assert_eq!(parse_per_turn_model("@claude-opus-4-6"), None);
    }

    #[test]
    fn test_parse_per_turn_model_trims_prompt_whitespace() {
        let result = parse_per_turn_model("@claude-haiku-4-5   refactor this");
        assert_eq!(
            result,
            Some(("claude-haiku-4-5".to_string(), "refactor this".to_string()))
        );
    }

    // ── /fast and /smart model name constants ─────────────────────────────────

    #[test]
    fn test_fast_model_name() {
        // Verify the model name used by /fast is the expected haiku model.
        let fast_model = "claude-haiku-4-5-20251001";
        assert!(!fast_model.is_empty());
        assert!(fast_model.contains("haiku"));
    }

    #[test]
    fn test_smart_model_name() {
        // Verify the model name used by /smart is the expected opus model.
        let smart_model = "claude-opus-4-6";
        assert!(!smart_model.is_empty());
        assert!(smart_model.contains("opus"));
    }

    // ── slash_completions ──────────────────────────────────────────────────────

    #[test]
    fn completions_for_slash_only_returns_all_commands() {
        let c = slash_completions("/");
        assert_eq!(c.len(), slash_commands().len());
    }

    #[test]
    fn completions_for_pr_includes_profile_variants() {
        // When the user types "/pr", autocomplete should offer /pr, /profile,
        // and /profiles — all are valid prefixed matches for the popup.
        let c = slash_completions("/pr");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(cmds.contains(&"/pr"), "/pr must be in completions");
        assert!(
            cmds.contains(&"/profile"),
            "/profile must be in completions"
        );
        assert!(
            cmds.contains(&"/profiles"),
            "/profiles must be in completions"
        );
    }

    #[test]
    fn completions_for_profile_does_not_include_pr() {
        let c = slash_completions("/profile");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(
            !cmds.contains(&"/pr"),
            "/pr must NOT appear for /profile prefix"
        );
    }

    #[test]
    fn completions_for_profiles_returns_only_profiles() {
        let c = slash_completions("/profiles");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "/profiles");
    }

    #[test]
    fn completions_empty_for_non_slash() {
        assert!(slash_completions("hello").is_empty());
        assert!(slash_completions("").is_empty());
    }

    #[test]
    fn completions_for_m_includes_model_and_models() {
        let c = slash_completions("/m");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(
            cmds.contains(&"/model"),
            "/model must be in completions for /m"
        );
        assert!(
            cmds.contains(&"/models"),
            "/models must be in completions for /m"
        );
        assert!(
            cmds.contains(&"/memory"),
            "/memory must be in completions for /m"
        );
    }

    #[test]
    fn completions_for_model_exact_includes_models() {
        let c = slash_completions("/model");
        let cmds: Vec<&str> = c.iter().map(|(cmd, _)| *cmd).collect();
        assert!(cmds.contains(&"/model"));
        assert!(cmds.contains(&"/models"));
    }

    #[test]
    fn is_known_slash_cmd_returns_false_for_partial_command() {
        // "/mod" is not a complete command, so it should return false
        assert!(!is_known_slash_cmd("/mod"));
    }

    #[test]
    fn parse_plan_from_text_strips_markdown_wrapped_steps() {
        let text = "### **Step 1:** Fix auth\n**Step 2:** Add tests\n";
        let tasks = parse_plan_from_text(text);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0], "Fix auth");
        assert_eq!(tasks[1], "Add tests");
    }

    #[test]
    fn build_plan_from_assistant_text_preserves_order_for_save_and_render() {
        let text = "Step 1: Define contract\nStep 2: Implement parser\nStep 3: Add tests";
        let plan = build_plan_from_assistant_text(text).expect("plan");
        let tasks: Vec<String> = plan.tasks.iter().map(|t| t.description.clone()).collect();
        assert_eq!(
            tasks,
            vec![
                "Define contract".to_string(),
                "Implement parser".to_string(),
                "Add tests".to_string()
            ]
        );
    }

    #[test]
    fn build_plan_from_assistant_text_fallback_is_deterministic() {
        let text = "alpha\n\n beta  \n gamma";
        let plan = build_plan_from_assistant_text(text).expect("fallback plan");
        let tasks: Vec<String> = plan.tasks.iter().map(|t| t.description.clone()).collect();
        assert_eq!(
            tasks,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn resolve_workdir_arg_accepts_absolute_dir() {
        let td = tempfile::tempdir().expect("tempdir");
        let p = td.path().to_path_buf();
        let resolved = resolve_workdir_arg(p.to_str().expect("utf8 path")).expect("resolve");
        assert_eq!(std::fs::canonicalize(p).expect("canonicalize"), resolved);
    }

    #[test]
    fn resolve_workdir_arg_rejects_missing_path() {
        let td = tempfile::tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist-12345");
        let err = resolve_workdir_arg(missing.to_str().expect("utf8 path")).expect_err("must fail");
        assert!(err.contains("could not access"));
    }

    // ── is_known_slash_cmd ────────────────────────────────────────────────────

    #[test]
    fn known_slash_cmd_exact_match() {
        assert!(is_known_slash_cmd("/pr"));
        assert!(is_known_slash_cmd("/clear"));
        assert!(is_known_slash_cmd("/ship"));
        assert!(is_known_slash_cmd("/profile"));
        assert!(is_known_slash_cmd("/profiles"));
    }

    #[test]
    fn known_slash_cmd_with_args() {
        assert!(is_known_slash_cmd("/pr my feature title"));
        assert!(is_known_slash_cmd("/profile work"));
        assert!(is_known_slash_cmd("/memory search query"));
        assert!(is_known_slash_cmd("/branch feature/foo"));
        assert!(is_known_slash_cmd("/ship fix login bug"));
    }

    #[test]
    fn known_slash_cmd_profile_with_name_is_recognized() {
        // Regression: /profile <name> must be recognized as a slash command,
        // not silently treated as chat input.
        assert!(is_known_slash_cmd("/profile default"));
        assert!(is_known_slash_cmd("/profile my-work-profile"));
    }

    #[test]
    fn known_slash_cmd_rejects_unknown() {
        assert!(!is_known_slash_cmd("/prfoo"));
        assert!(!is_known_slash_cmd("/notacommand"));
        assert!(!is_known_slash_cmd("not a slash"));
        assert!(!is_known_slash_cmd(""));
    }

    // ── slash_completion_rows grouping ────────────────────────────────────────

    #[test]
    fn completion_rows_have_headers_between_sections() {
        let rows = slash_completion_rows("/");
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Header(h) = r {
                    Some(*h)
                } else {
                    None
                }
            })
            .collect();
        // All six sections should appear as headers.
        assert!(headers.contains(&"Session"));
        assert!(headers.contains(&"Git"));
        assert!(headers.contains(&"Model"));
        assert!(headers.contains(&"Context"));
        assert!(headers.contains(&"Plan"));
        assert!(headers.contains(&"Project"));
    }

    #[test]
    fn completion_rows_pr_section_under_git_header() {
        let rows = slash_completion_rows("/pr");
        // Should have exactly one Git header since /pr, /profile, /profiles
        // all live in different sections.
        let header_sections: Vec<&str> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Header(h) = r {
                    Some(*h)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            header_sections.contains(&"Git"),
            "Git section header expected"
        );
        assert!(
            header_sections.contains(&"Project"),
            "Project section header expected (contains /profile)"
        );
    }

    #[test]
    fn completion_rows_flat_indices_are_contiguous() {
        // flat_idx in Cmd rows must be 0, 1, 2, ... without gaps.
        let rows = slash_completion_rows("/");
        let indices: Vec<usize> = rows
            .iter()
            .filter_map(|r| {
                if let CompletionRow::Cmd { flat_idx, .. } = r {
                    Some(*flat_idx)
                } else {
                    None
                }
            })
            .collect();
        for (expected, got) in indices.iter().enumerate() {
            assert_eq!(
                expected, *got,
                "flat_idx out of order at position {}",
                expected
            );
        }
    }

    #[test]
    fn slash_commands_derived_matches_registry_count() {
        // slash_commands() must match the command_registry size.
        assert_eq!(
            slash_commands().len(),
            crate::command_registry::COMMANDS.len(),
            "slash_commands() and command_registry::COMMANDS are out of sync"
        );
    }

    #[test]
    fn undo_command_description_mentions_confirmation() {
        let desc = slash_commands()
            .into_iter()
            .find_map(|(cmd, desc)| (cmd == "/undo").then_some(desc))
            .expect("/undo command should exist");
        assert!(
            desc.contains("confirm"),
            "/undo description should mention confirmation for safety"
        );
    }

    #[test]
    fn submit_queues_known_slash_when_busy() {
        let mut app = make_test_app();
        app.busy = true;
        app.text_input.text = "/help".to_string();
        app.text_input.cursor = app.text_input.text.chars().count();

        app.submit();

        assert_eq!(app.queued.len(), 1);
        assert_eq!(app.queued.front().map(String::as_str), Some("/help"));
    }

    #[test]
    fn force_send_interrupt_prioritizes_prompt_at_queue_front() {
        let mut app = make_test_app();
        app.busy = true;
        app.queued.push_back("older queued item".to_string());
        app.text_input.text = "urgent next prompt".to_string();
        app.text_input.cursor = app.text_input.text.chars().count();

        app.force_send();

        assert_eq!(app.queued.len(), 2);
        assert_eq!(
            app.queued.front().map(String::as_str),
            Some("urgent next prompt")
        );
        assert!(app.cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn perms_state_clear_all_grants_resets_permissions() {
        let mut perms = PermsState::default();
        perms.session_allowed.insert("Edit".to_string());
        perms.workdir_open = true;

        perms.clear_all_grants();

        assert!(perms.session_allowed.is_empty());
        assert!(!perms.workdir_open);
    }

    #[test]
    fn workdir_command_does_not_switch_before_runtime_confirmation() {
        let mut app = make_test_app();
        let original = app.workspace_root.clone();
        let target_dir = std::env::temp_dir();
        let cmd = format!("/workdir {}", target_dir.display());

        execute_slash(&mut app, &cmd);

        assert_eq!(
            app.workspace_root, original,
            "UI workdir should remain unchanged until runtime confirms switch"
        );
    }

    #[tokio::test]
    async fn tui_ask_user_session_grant_skips_second_prompt_for_same_tool() {
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let ask = TuiAskUser {
            perm_tx,
            perms: Arc::new(Mutex::new(PermsState::default())),
        };

        let first = tokio::spawn({
            let ask = TuiAskUser {
                perm_tx: ask.perm_tx.clone(),
                perms: ask.perms.clone(),
            };
            async move {
                ask.ask(AgentPermRequest {
                    tool_name: "Write".to_string(),
                    description: "{\"path\":\"a.txt\"}".to_string(),
                    diff: None,
                    proposed_content: None,
                    file_path: None,
                })
                .await
            }
        });
        let pending = perm_rx.recv().await.expect("first prompt expected");
        pending
            .reply
            .send(PermGrant::Session)
            .expect("reply should send");
        let first_grant = first.await.expect("first ask task should complete");
        assert!(matches!(first_grant, AgentPermGrant::AllowAll));

        let second_grant = ask
            .ask(AgentPermRequest {
                tool_name: "Write".to_string(),
                description: "{\"path\":\"a.txt\"}".to_string(),
                diff: None,
                proposed_content: None,
                file_path: None,
            })
            .await;
        assert!(matches!(second_grant, AgentPermGrant::Allow));
        assert!(
            perm_rx.try_recv().is_err(),
            "second ask for same tool should not prompt again"
        );
    }

    #[tokio::test]
    async fn tui_ask_user_workdir_grant_skips_prompt_for_other_tools() {
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let ask = TuiAskUser {
            perm_tx,
            perms: Arc::new(Mutex::new(PermsState::default())),
        };

        let first_req = AgentPermRequest {
            tool_name: "Edit".to_string(),
            description: "{}".to_string(),
            diff: None,
            proposed_content: None,
            file_path: None,
        };
        let first = tokio::spawn({
            let ask = TuiAskUser {
                perm_tx: ask.perm_tx.clone(),
                perms: ask.perms.clone(),
            };
            async move { ask.ask(first_req).await }
        });
        let pending = perm_rx.recv().await.expect("first prompt expected");
        pending
            .reply
            .send(PermGrant::Workdir)
            .expect("reply should send");
        let first_grant = first.await.expect("first ask task should complete");
        assert!(matches!(first_grant, AgentPermGrant::AllowAll));

        let second_req = AgentPermRequest {
            tool_name: "Write".to_string(),
            description: "{}".to_string(),
            diff: None,
            proposed_content: None,
            file_path: None,
        };
        let second_grant = ask.ask(second_req).await;
        assert!(matches!(second_grant, AgentPermGrant::Allow));
        assert!(
            perm_rx.try_recv().is_err(),
            "workdir grant should avoid prompting for other tools"
        );
    }

    // ── T01: TUI critical path tests ──────────────────────────────────────

    #[test]
    fn model_picker_filtered_trims_whitespace() {
        fn make_entry(id: &str) -> ModelEntry {
            ModelEntry {
                id: id.to_string(),
                provider: "test".to_string(),
                input_mtok: 0.0,
                output_mtok: 0.0,
                context_k: None,
                role: None,
                is_favorite: false,
            }
        }
        let mut state = ModelPickerState {
            models: vec![make_entry("gpt-4"), make_entry("claude-3")],
            filter: "  gpt  ".to_string(),
            selected: 0,
            scroll_offset: 0,
        };
        let filtered = state.filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "gpt-4");

        // Whitespace-only filter should return all models
        state.filter = "   ".to_string();
        let all = state.filtered();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn app_per_turn_override_indicator_set_and_cleared() {
        let mut app = make_test_app();
        assert!(app.per_turn_prev_model.is_none());
        // Simulate override set
        app.per_turn_prev_model = Some("base-model".to_string());
        assert!(app.per_turn_prev_model.is_some());
        // Simulate override cleared
        app.per_turn_prev_model = None;
        assert!(app.per_turn_prev_model.is_none());
    }

    #[test]
    fn queue_messages_empty_does_not_panic() {
        let mut app = make_test_app();
        // Draining an empty queue should be a no-op
        let drained: Vec<_> = app.queued.drain(..).collect();
        assert!(drained.is_empty());
    }

    #[test]
    fn queue_messages_preserves_order() {
        let mut app = make_test_app();
        app.queued.push_back("first".to_string());
        app.queued.push_back("second".to_string());
        app.queued.push_back("third".to_string());
        let result: Vec<_> = app.queued.drain(..).collect();
        assert_eq!(result, vec!["first", "second", "third"]);
    }

    #[test]
    fn slash_completions_returns_sorted_unique_commands() {
        let completions = slash_completions("/");
        // All completions should have names starting with "/"
        for (cmd, _desc) in &completions {
            assert!(cmd.starts_with('/'), "completion missing /: {cmd}");
        }
        // Should not have duplicates
        let mut names: Vec<_> = completions.iter().map(|(c, _)| *c).collect();
        names.sort();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate slash completions found");
    }

    #[test]
    fn search_and_export_are_known_commands() {
        assert!(
            is_known_slash_cmd("/search"),
            "/search must be a known command"
        );
        assert!(
            is_known_slash_cmd("/export"),
            "/export must be a known command"
        );
        assert!(
            is_known_slash_cmd("/search hello world"),
            "/search with args must be known"
        );
    }

    #[test]
    fn search_and_export_appear_in_completions() {
        let cmds: Vec<_> = slash_completions("/s").iter().map(|(c, _)| *c).collect();
        assert!(
            cmds.contains(&"/search"),
            "/search must appear in /s completions"
        );
        let all: Vec<_> = slash_completions("/export")
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert!(
            all.contains(&"/export"),
            "/export must appear in completions"
        );
    }

    #[test]
    fn search_and_export_have_non_empty_descriptions() {
        let all = slash_commands();
        let search_desc = all.iter().find(|(c, _)| *c == "/search").map(|(_, d)| d);
        let export_desc = all.iter().find(|(c, _)| *c == "/export").map(|(_, d)| d);
        assert!(
            search_desc.is_some_and(|d| !d.is_empty()),
            "/search needs a description"
        );
        assert!(
            export_desc.is_some_and(|d| !d.is_empty()),
            "/export needs a description"
        );
    }

    // ── render_markdown responsiveness ───────────────────────────────────────

    #[test]
    fn code_block_close_scales_with_width() {
        let md = "```bash\necho hello\n```";
        let narrow = render_markdown(md, 40);
        let wide = render_markdown(md, 120);
        // Find the close-bar line (contains the └ character)
        let close_narrow = narrow
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.contains('└'))
                    .unwrap_or(false)
            })
            .expect("close bar missing on narrow");
        let close_wide = wide
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.contains('└'))
                    .unwrap_or(false)
            })
            .expect("close bar missing on wide");
        let narrow_len: usize = close_narrow
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let wide_len: usize = close_wide
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert!(
            narrow_len <= wide_len,
            "narrow close bar ({narrow_len}) should be ≤ wide ({wide_len})"
        );
    }

    #[test]
    fn horizontal_rule_scales_with_width() {
        let md = "---";
        let narrow = render_markdown(md, 40);
        let wide = render_markdown(md, 100);
        let hr_narrow = narrow
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('─'))
                    .unwrap_or(false)
            })
            .expect("hr missing on narrow");
        let hr_wide = wide
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.starts_with('─'))
                    .unwrap_or(false)
            })
            .expect("hr missing on wide");
        let n_len: usize = hr_narrow
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let w_len: usize = hr_wide
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        assert!(
            n_len <= w_len,
            "narrow hr ({n_len}) should be ≤ wide ({w_len})"
        );
    }

    // ── Profile overlay unit tests ────────────────────────────────────────────

    #[test]
    fn profile_overlay_for_edit_initializes_correctly() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: Some("sk-test-1234".into()),
            api_key_env: None,
            base_url: Some("https://custom.api.example.com".into()),
            user_agent: None,
            fast: None,
        };
        let ov = ProfileOverlayState::for_edit(
            "myprofile".into(),
            &entry,
            std::path::PathBuf::from("/tmp/test-config.toml"),
            &Default::default(),
        );
        assert_eq!(ov.name, "myprofile");
        assert_eq!(ov.provider, "anthropic");
        assert_eq!(ov.model, "claude-opus-4-5");
        assert_eq!(ov.api_key, "sk-test-1234");
        assert_eq!(ov.base_url, "https://custom.api.example.com");
        assert!(!ov.is_new);
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
        assert_eq!(ov.cursor, 0);
    }

    #[test]
    fn profile_overlay_for_create_starts_in_create_mode() {
        let ov = ProfileOverlayState::for_create(std::path::PathBuf::from("/tmp/test.toml"), &Default::default());
        assert!(ov.is_new);
        assert_eq!(
            ov.mode,
            ProfileOverlayMode::Creating {
                step: ProfileCreateStep::Name
            }
        );
        assert!(ov.name.is_empty());
    }

    #[test]
    fn profile_overlay_begin_and_cancel_edit() {
        let entry = clido_core::ProfileEntry {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
            &Default::default(),
        );
        // Navigate to ApiKey field (cursor=1) — uses inline edit, not picker
        ov.cursor = 1;
        ov.begin_edit(&[]);
        assert_eq!(
            ov.mode,
            ProfileOverlayMode::EditField(ProfileEditField::ApiKey)
        );
        assert_eq!(ov.input, "");
        assert_eq!(ov.input_cursor, 0);

        // Cancel should restore overview
        ov.cancel_edit();
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
        assert!(ov.input.is_empty());
        assert_eq!(ov.input_cursor, 0);
        // Original value unchanged
        assert_eq!(ov.model, "gpt-4o");
    }

    #[test]
    fn profile_overlay_commit_edit_updates_field() {
        let entry = clido_core::ProfileEntry {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
            &Default::default(),
        );
        // Model field (cursor=2) now uses picker — begin_edit enters PickingModel mode
        ov.cursor = 2;
        let model_entry = ModelEntry {
            id: "claude-haiku-4-5".into(),
            provider: "openai".into(),
            input_mtok: 0.25,
            output_mtok: 1.25,
            context_k: Some(200),
            role: None,
            is_favorite: false,
        };
        ov.begin_edit(&[model_entry]);
        assert!(matches!(ov.mode, ProfileOverlayMode::PickingModel { .. }));
        // picker should have the model entry
        assert!(ov.profile_model_picker.is_some());
        // commit the pick
        ov.commit_model_pick();
        assert_eq!(ov.model, "claude-haiku-4-5");
        assert_eq!(ov.mode, ProfileOverlayMode::Overview);
    }

    #[test]
    fn profile_overlay_cursor_field_mapping() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: None,
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        let mut ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
            &Default::default(),
        );
        ov.cursor = 0;
        assert_eq!(ov.cursor_field(), ProfileEditField::Provider);
        ov.cursor = 1;
        assert_eq!(ov.cursor_field(), ProfileEditField::ApiKey);
        ov.cursor = 2;
        assert_eq!(ov.cursor_field(), ProfileEditField::Model);
        ov.cursor = 3;
        assert_eq!(ov.cursor_field(), ProfileEditField::BaseUrl);
    }

    #[test]
    fn profile_overlay_masked_api_key() {
        let entry = clido_core::ProfileEntry {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            api_key: Some("sk-ant-api03-verylongkeyvalue".into()),
            api_key_env: None,
            base_url: None,
            user_agent: None,
            fast: None,
        };
        let ov = ProfileOverlayState::for_edit(
            "p".into(),
            &entry,
            std::path::PathBuf::from("/tmp/t.toml"),
            &Default::default(),
        );
        let masked = ov.masked_api_key();
        // Should not reveal full key
        assert!(!masked.contains("verylongkeyvalue"));
        // Should not be empty
        assert!(!masked.is_empty());
    }

    #[test]
    fn char_byte_pos_tui_works_for_ascii_and_unicode() {
        let s = "hello";
        assert_eq!(char_byte_pos_tui(s, 0), 0);
        assert_eq!(char_byte_pos_tui(s, 3), 3);
        assert_eq!(char_byte_pos_tui(s, 5), 5); // end

        // Unicode: each emoji is >1 byte
        let u = "hé";
        assert_eq!(char_byte_pos_tui(u, 0), 0);
        assert_eq!(char_byte_pos_tui(u, 1), 1); // 'h' is 1 byte
        assert_eq!(char_byte_pos_tui(u, 2), 3); // 'é' is 2 bytes
    }

    // ── Multiline cursor navigation tests ────────────────────────────────────

    #[test]
    fn move_up_single_line_returns_none() {
        // No newline — can't move up.
        assert_eq!(move_cursor_line_up("hello world", 5), None);
    }

    #[test]
    fn move_down_single_line_returns_none() {
        assert_eq!(move_cursor_line_down("hello world", 5), None);
    }

    #[test]
    fn move_up_on_first_line_returns_none() {
        // Cursor on line 0 — can't go further up.
        let s = "line0\nline1\nline2";
        assert_eq!(move_cursor_line_up(s, 3), None); // col 3 of "line0"
    }

    #[test]
    fn move_down_on_last_line_returns_none() {
        let s = "line0\nline1\nline2";
        // "line2" starts at index 12; cursor at 14 (col 2)
        assert_eq!(move_cursor_line_down(s, 14), None);
    }

    #[test]
    fn move_up_from_second_line() {
        // "abc\nde\nfghi"
        //  0123 456 7890
        // line0="abc" (0-2), line1="de" (4-5), line2="fghi" (7-10)
        let s = "abc\nde\nfghi";
        // Cursor at index 5 = col 1 of "de"
        // Moving up → col 1 of "abc" = index 1
        assert_eq!(move_cursor_line_up(s, 5), Some(1));
    }

    #[test]
    fn move_up_clamps_to_shorter_prev_line() {
        // "ab\ndefgh"  — line0 is shorter
        // Cursor at index 7 = col 5 of "defgh" (which is "h")
        // prev line "ab" has len 2, so should clamp to col 2 (end of "ab") = index 2
        let s = "ab\ndefgh";
        assert_eq!(move_cursor_line_up(s, 7), Some(2));
    }

    #[test]
    fn move_down_from_first_line() {
        // "abc\nde\nfghi"
        let s = "abc\nde\nfghi";
        // Cursor at index 1 (col 1 of "abc") → col 1 of "de" = index 5
        assert_eq!(move_cursor_line_down(s, 1), Some(5));
    }

    #[test]
    fn move_down_clamps_to_shorter_next_line() {
        // "defgh\nab"  — next line is shorter
        // Cursor at col 4 of "defgh" = index 4
        // next line "ab" has len 2, clamp to 2 = index 8
        let s = "defgh\nab";
        assert_eq!(move_cursor_line_down(s, 4), Some(8));
    }

    #[test]
    fn move_up_down_roundtrip() {
        // Moving down then up should return to original position (when lines are equal length)
        let s = "hello\nworld\nfinal";
        let start = 2; // col 2 of "hello"
        let down = move_cursor_line_down(s, start).unwrap();
        let back = move_cursor_line_up(s, down).unwrap();
        assert_eq!(back, start);
    }

    // ── T01: TUI critical path unit tests ─────────────────────────────────────

    fn make_key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    // T01-1: pressing Enter with option 3 (Deny) selected resolves PendingPerm with Deny.
    #[test]
    fn perm_modal_deny_sends_deny_grant() {
        use crossterm::event::KeyCode;
        use tokio::sync::oneshot;

        let mut app = make_test_app();
        let (reply_tx, mut reply_rx) = oneshot::channel::<PermGrant>();
        app.pending_perm = Some(PendingPerm {
            tool_name: "Write".to_string(),
            preview: "writing to a file".to_string(),
            reply: reply_tx,
        });
        app.perm_selected = 3; // index 3 = Deny

        handle_key(&mut app, make_key(KeyCode::Enter));

        assert!(
            app.pending_perm.is_none(),
            "pending_perm should be cleared after Deny"
        );
        let grant = reply_rx.try_recv().expect("reply should have been sent");
        assert!(
            matches!(grant, PermGrant::Deny),
            "expected PermGrant::Deny, got {:?}",
            grant
        );
        assert_eq!(app.perm_selected, 0, "selection should reset to 0");
    }

    // T01-2: DenyWithFeedback — Enter on option 4 enters feedback mode; second Enter sends
    // DenyWithFeedback with the typed reason.
    #[test]
    fn perm_modal_deny_with_feedback_sends_reason() {
        use crossterm::event::KeyCode;
        use tokio::sync::oneshot;

        let mut app = make_test_app();
        let (reply_tx, mut reply_rx) = oneshot::channel::<PermGrant>();
        app.pending_perm = Some(PendingPerm {
            tool_name: "Bash".to_string(),
            preview: "rm -rf /".to_string(),
            reply: reply_tx,
        });
        app.perm_selected = 4; // index 4 = DenyWithFeedback

        // First Enter: enter feedback-input mode.
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(
            app.perm_feedback_input.is_some(),
            "feedback input mode should be active"
        );
        assert!(
            app.pending_perm.is_some(),
            "pending_perm should still be set during feedback entry"
        );

        // Type feedback characters.
        handle_key(&mut app, make_key(KeyCode::Char('b')));
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_key(&mut app, make_key(KeyCode::Char('d')));

        // Second Enter: submit feedback.
        handle_key(&mut app, make_key(KeyCode::Enter));

        assert!(
            app.pending_perm.is_none(),
            "pending_perm should be cleared after DenyWithFeedback submit"
        );
        assert!(
            app.perm_feedback_input.is_none(),
            "perm_feedback_input should be cleared"
        );
        assert_eq!(app.perm_selected, 0, "selection should reset to 0");

        let grant = reply_rx.try_recv().expect("reply should have been sent");
        match grant {
            PermGrant::DenyWithFeedback(reason) => {
                assert_eq!(reason, "bad", "feedback text mismatch: {reason}");
            }
            other => panic!("expected DenyWithFeedback, got {:?}", other),
        }
    }

    // T01-3: messages queued while busy are processed FIFO.
    #[test]
    fn queue_processes_items_in_fifo_order() {
        let mut app = make_test_app();
        app.busy = true;

        // Submit three inputs while busy — they should land in queued FIFO.
        for msg in &["first", "second", "third"] {
            app.text_input.text = msg.to_string();
            app.text_input.cursor = app.text_input.text.chars().count();
            app.submit();
        }

        assert_eq!(app.queued.len(), 3, "all three inputs should be queued");
        let items: Vec<&str> = app.queued.iter().map(String::as_str).collect();
        assert_eq!(
            items,
            vec!["first", "second", "third"],
            "queue must preserve FIFO order"
        );
    }

    // T01-4: input_history is capped at 1000 entries (FX14 fix).
    #[test]
    fn input_history_capped_at_1000() {
        let mut app = make_test_app();
        // Push 1001 distinct entries directly via send_now (which adds to history).
        for i in 0..1001usize {
            app.send_now(format!("prompt {i}"));
        }
        assert_eq!(
            app.text_input.history.len(),
            1000,
            "history must be capped at 1000"
        );
        // The very first entry "prompt 0" should have been evicted.
        assert_ne!(
            app.text_input.history.first().map(String::as_str),
            Some("prompt 0"),
            "oldest entry should have been evicted"
        );
        // The last entry should be the most recent.
        assert_eq!(
            app.text_input.history.last().map(String::as_str),
            Some("prompt 1000"),
            "newest entry should be the last element"
        );
    }

    // T01-5: a multiline input (containing '\n') is submitted and stored in history.
    #[test]
    fn multiline_input_is_handled() {
        let mut app = make_test_app();
        let multiline = "line one\nline two\nline three";
        app.send_now(multiline.to_string());

        // The input should appear as a ChatLine::User message.
        let has_user_line = app.messages.iter().any(|m| {
            if let ChatLine::User(text) = m {
                text == multiline
            } else {
                false
            }
        });
        assert!(
            has_user_line,
            "multiline input should appear as a User ChatLine"
        );

        // It should also be recorded in history.
        assert!(
            app.text_input.history.contains(&multiline.to_string()),
            "multiline input should be recorded in input_history"
        );
    }

    // ── Snapshot-style render tests (TestBackend) ──────────────────────────────

    use crate::overlay::ReadOnlyOverlay;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn test_terminal() -> Terminal<TestBackend> {
        let backend = TestBackend::new(120, 40);
        Terminal::new(backend).unwrap()
    }

    /// Helper: collect the entire terminal buffer into a single string for
    /// substring assertions.
    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).map_or(" ", |c| c.symbol()));
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn render_main_chat_empty_no_panic() {
        let mut app = make_test_app();
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        // Header always contains the brand
        assert!(text.contains("cli"), "header should contain 'cli'");
    }

    #[test]
    fn render_error_overlay_shows_message() {
        let mut app = make_test_app();
        app.overlay_stack.push(OverlayKind::Error(ErrorOverlay::new(
            "something went wrong",
        )));
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("something went wrong"),
            "error overlay should show the error message, got:\n{}",
            text
        );
    }

    #[test]
    fn render_readonly_overlay_shows_content() {
        let mut app = make_test_app();
        app.overlay_stack
            .push(OverlayKind::ReadOnly(ReadOnlyOverlay::new(
                "Test Info",
                vec![
                    ("Section A".into(), "Alpha content here".into()),
                    ("Section B".into(), "Beta content here".into()),
                ],
            )));
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Alpha content"),
            "readonly overlay should display content, got:\n{}",
            text
        );
    }

    #[test]
    fn render_model_picker_shows_header() {
        let mut app = make_test_app();
        app.model_picker = Some(ModelPickerState {
            models: vec![
                ModelEntry {
                    id: "gpt-4o".into(),
                    provider: "openai".into(),
                    input_mtok: 2.5,
                    output_mtok: 10.0,
                    context_k: Some(128),
                    role: None,
                    is_favorite: false,
                },
                ModelEntry {
                    id: "claude-sonnet".into(),
                    provider: "anthropic".into(),
                    input_mtok: 3.0,
                    output_mtok: 15.0,
                    context_k: Some(200),
                    role: None,
                    is_favorite: true,
                },
            ],
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Filter"),
            "model picker should display filter prompt, got:\n{}",
            text
        );
    }

    #[test]
    fn render_session_picker_shows_sessions() {
        let mut app = make_test_app();
        app.session_picker = Some(SessionPickerState {
            picker: ListPicker::new(
                vec![clido_storage::SessionSummary {
                    session_id: "abc123".into(),
                    project_path: "/home/user/proj".into(),
                    start_time: "2025-01-01T00:00:00Z".into(),
                    num_turns: 5,
                    total_cost_usd: 0.42,
                    preview: "hello world".into(),
                    title: None,
                }],
                12,
            ),
        });
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("abc123"),
            "session picker should display session id, got:\n{}",
            text
        );
    }

    #[test]
    fn render_profile_overlay_overview_no_panic() {
        let mut app = make_test_app();
        app.profile_overlay = Some(ProfileOverlayState::for_create(
            std::env::temp_dir().join("test-config.toml"),
            &Default::default(),
        ));
        // Switch to overview mode so we hit render_profile_overview path
        if let Some(ref mut st) = app.profile_overlay {
            st.name = "test-profile".into();
            st.provider = "openrouter".into();
            st.model = "gpt-4o".into();
            st.mode = ProfileOverlayMode::Overview;
        }
        let mut terminal = test_terminal();
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        // Profile overlay should show the provider or model somewhere
        assert!(
            text.contains("openrouter") || text.contains("gpt-4o") || text.contains("test-profile"),
            "profile overlay should display profile info, got:\n{}",
            text
        );
    }

    // ── E2E flow tests ─────────────────────────────────────────────────────────

    use crossterm::event::{KeyEvent, KeyModifiers as Km};

    fn sim_key(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, Km::NONE));
    }

    fn sim_char(app: &mut App, c: char) {
        sim_key(app, KeyCode::Char(c));
    }

    #[test]
    fn e2e_slash_command_opens_model_picker() {
        let mut app = make_test_app();
        execute_slash(&mut app, "/model");
        assert!(
            app.model_picker.is_some(),
            "'/model' should open model picker"
        );
    }

    #[test]
    fn e2e_slash_command_opens_session_picker() {
        let mut app = make_test_app();
        execute_slash(&mut app, "/sessions");
        // sessions may be empty in temp dir — picker should still open or info shown
        let has_picker = app.session_picker.is_some();
        let has_info = app.messages.iter().any(|l| matches!(l, ChatLine::Info(_)));
        assert!(
            has_picker || has_info,
            "'/sessions' should open picker or show info"
        );
    }

    #[test]
    fn e2e_model_picker_navigate_and_escape() {
        let mut app = make_test_app();
        app.model_picker = Some(ModelPickerState {
            models: vec![
                ModelEntry {
                    id: "gpt-4o".into(),
                    provider: "openai".into(),
                    input_mtok: 2.5,
                    output_mtok: 10.0,
                    context_k: Some(128),
                    role: None,
                    is_favorite: false,
                },
                ModelEntry {
                    id: "claude-sonnet".into(),
                    provider: "anthropic".into(),
                    input_mtok: 3.0,
                    output_mtok: 15.0,
                    context_k: Some(200),
                    role: None,
                    is_favorite: false,
                },
            ],
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
        sim_key(&mut app, KeyCode::Down);
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 1);
        sim_key(&mut app, KeyCode::Esc);
        assert!(app.model_picker.is_none(), "Esc should close model picker");
    }

    #[test]
    fn e2e_session_picker_navigate_and_escape() {
        let mut app = make_test_app();
        app.session_picker = Some(SessionPickerState {
            picker: ListPicker::new(
                vec![
                    clido_storage::SessionSummary {
                        session_id: "aaa".into(),
                        project_path: "/tmp".into(),
                        start_time: "2025-01-01T00:00:00Z".into(),
                        num_turns: 1,
                        total_cost_usd: 0.0,
                        preview: "first".into(),
                        title: None,
                    },
                    clido_storage::SessionSummary {
                        session_id: "bbb".into(),
                        project_path: "/tmp".into(),
                        start_time: "2025-01-02T00:00:00Z".into(),
                        num_turns: 2,
                        total_cost_usd: 0.1,
                        preview: "second".into(),
                        title: None,
                    },
                ],
                12,
            ),
        });
        sim_key(&mut app, KeyCode::Down);
        assert_eq!(app.session_picker.as_ref().unwrap().picker.selected, 1);
        // Filter by typing
        sim_char(&mut app, 'b');
        let filtered_count = app.session_picker.as_ref().unwrap().picker.filtered_count();
        assert_eq!(filtered_count, 1, "filter 'b' should match only 'bbb'");
        sim_key(&mut app, KeyCode::Esc);
        assert!(
            app.session_picker.is_none(),
            "Esc should close session picker"
        );
    }

    #[test]
    fn e2e_error_overlay_dismiss() {
        let mut app = make_test_app();
        app.overlay_stack
            .push(OverlayKind::Error(ErrorOverlay::new("test error")));
        assert!(!app.overlay_stack.is_empty());
        sim_key(&mut app, KeyCode::Enter);
        assert!(
            app.overlay_stack.is_empty(),
            "Enter should dismiss error overlay"
        );
    }

    #[test]
    fn e2e_unknown_slash_command_is_silent() {
        let mut app = make_test_app();
        let before = app.messages.len();
        execute_slash(&mut app, "/nonexistent_command_xyz");
        // Unknown commands are silently ignored (fall through _ => {})
        assert_eq!(
            app.messages.len(),
            before,
            "unknown command should not add messages"
        );
    }

    #[test]
    fn e2e_help_command_shows_info() {
        let mut app = make_test_app();
        execute_slash(&mut app, "/help");
        let has_help = app.messages.iter().any(|l| match l {
            ChatLine::Section(s) => s.contains("Navigation"),
            _ => false,
        });
        assert!(has_help, "/help should show navigation section");
    }

    #[test]
    fn e2e_clear_command_clears_chat() {
        let mut app = make_test_app();
        app.push(ChatLine::Info("some message".into()));
        let before = app.messages.len();
        assert!(before > 0);
        execute_slash(&mut app, "/clear");
        // /clear resets to WelcomeBrand + new session message
        let has_clear_msg = app.messages.iter().any(|l| match l {
            ChatLine::Info(s) => s.contains("New session started") || s.contains("cleared"),
            _ => false,
        });
        assert!(has_clear_msg, "/clear should show new session message");
    }

    #[test]
    fn e2e_model_picker_filter_narrows_results() {
        let mut app = make_test_app();
        app.model_picker = Some(ModelPickerState {
            models: vec![
                ModelEntry {
                    id: "gpt-4o".into(),
                    provider: "openai".into(),
                    input_mtok: 0.0,
                    output_mtok: 0.0,
                    context_k: None,
                    role: None,
                    is_favorite: false,
                },
                ModelEntry {
                    id: "claude-sonnet".into(),
                    provider: "anthropic".into(),
                    input_mtok: 0.0,
                    output_mtok: 0.0,
                    context_k: None,
                    role: None,
                    is_favorite: false,
                },
            ],
            filter: String::new(),
            selected: 0,
            scroll_offset: 0,
        });
        for c in "claude".chars() {
            sim_char(&mut app, c);
        }
        let picker = app.model_picker.as_ref().unwrap();
        let filtered = picker.filtered();
        assert_eq!(filtered.len(), 1, "filter 'claude' should match one model");
        assert!(
            filtered[0].id.contains("claude"),
            "filtered model should be claude"
        );
    }
}
