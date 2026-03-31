//! TUI event loop for the setup wizard.

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{backend::CrosstermBackend, Terminal};

use clido_providers::registry::PROVIDER_REGISTRY;

use super::render::draw_setup;
use super::types::{
    make_model_picker, ModelOption, RoleEditField, SetupOutcome, SetupPreFill, SetupState,
    SetupStep,
};

use super::SUBAGENT_OPTIONS;

pub(super) fn setup_event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    pre_fill: Option<SetupPreFill>,
) -> Result<SetupOutcome, anyhow::Error> {
    let mut s = match pre_fill {
        Some(pf) => SetupState::new_with_prefill(pf),
        None => SetupState::new(),
    };

    loop {
        terminal.draw(|f| draw_setup(f, &s))?;

        // After rendering "FetchingModels", do the blocking fetch.
        if s.needs_fetch {
            s.needs_fetch = false;
            let provider_id = PROVIDER_REGISTRY[s.provider].id;
            let (api_key, base_url): (&str, Option<&str>) = if s.is_local() {
                ("", Some(s.credential.as_str()))
            } else {
                (s.credential.as_str(), None)
            };
            let handle = tokio::runtime::Handle::current();
            s.fetched_models = handle.block_on(clido_providers::fetch_provider_models(
                provider_id,
                api_key,
                base_url,
            ));
            s.model_picker = make_model_picker(&s.fetched_models);
            // If reinit, pre-select the current model in the list.
            if !s.current_model.is_empty() {
                if let Some(idx) = s
                    .model_picker
                    .items()
                    .iter()
                    .position(|o| matches!(o, ModelOption::Entry(m) if m.id == s.current_model))
                {
                    s.model_picker.selected = idx;
                    // Try to center it in the visible window (assume ~10 visible rows).
                    s.model_picker.scroll_offset = idx.saturating_sub(5);
                }
            }
            s.model_picker.filter.clear();
            s.clear_typed_input();
            s.step = SetupStep::Model;
            continue;
        }

        // Fetch worker models.
        if s.worker_needs_fetch {
            s.worker_needs_fetch = false;
            let provider_id = PROVIDER_REGISTRY[s.worker_provider].id;
            let is_local_worker = PROVIDER_REGISTRY[s.worker_provider].is_local;
            let (api_key, base_url): (&str, Option<&str>) = if is_local_worker {
                ("", Some(s.worker_credential.as_str()))
            } else {
                (s.worker_credential.as_str(), None)
            };
            let handle = tokio::runtime::Handle::current();
            s.worker_fetched_models = handle.block_on(clido_providers::fetch_provider_models(
                provider_id,
                api_key,
                base_url,
            ));
            s.worker_custom_model = s.worker_fetched_models.is_empty();
            s.worker_model_picker = make_model_picker(&s.worker_fetched_models);
            s.clear_typed_input();
            s.step = SetupStep::WorkerModel;
            continue;
        }

        // Fetch reviewer models.
        if s.reviewer_needs_fetch {
            s.reviewer_needs_fetch = false;
            let provider_id = PROVIDER_REGISTRY[s.reviewer_provider].id;
            let is_local_reviewer = PROVIDER_REGISTRY[s.reviewer_provider].is_local;
            let (api_key, base_url): (&str, Option<&str>) = if is_local_reviewer {
                ("", Some(s.reviewer_credential.as_str()))
            } else {
                (s.reviewer_credential.as_str(), None)
            };
            let handle = tokio::runtime::Handle::current();
            s.reviewer_fetched_models = handle.block_on(clido_providers::fetch_provider_models(
                provider_id,
                api_key,
                base_url,
            ));
            s.reviewer_custom_model = s.reviewer_fetched_models.is_empty();
            s.reviewer_model_picker = make_model_picker(&s.reviewer_fetched_models);
            s.clear_typed_input();
            s.step = SetupStep::ReviewerModel;
            continue;
        }

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Ctrl+C / Ctrl+Q → cancel wizard (same as Esc on first screens)
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('q'))
            {
                return Ok(SetupOutcome::Cancelled);
            }

            s.error = None;

            match s.step {
                // ── Profile name input (new named profiles only) ──────────
                SetupStep::ProfileName => match key.code {
                    KeyCode::Esc => {
                        return Ok(SetupOutcome::Cancelled);
                    }
                    KeyCode::Enter => {
                        let name = s.text_input.text.trim().to_string();
                        if name.is_empty() {
                            s.error =
                                Some("Profile name required — type a name and press Enter".into());
                        } else if name.contains(' ') || name.contains('/') {
                            s.error = Some("Profile name must not contain spaces or '/'".into());
                        } else {
                            s.profile_name = name;
                            s.clear_typed_input();
                            s.step = SetupStep::Provider;
                        }
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── Provider selection ────────────────────────────────────
                SetupStep::Provider => match key.code {
                    KeyCode::Esc => {
                        if s.started_with_profile_name {
                            s.step = SetupStep::ProfileName;
                            s.text_input.set_text(s.profile_name.clone());
                        } else {
                            return Ok(SetupOutcome::Cancelled);
                        }
                    }
                    KeyCode::Enter => {
                        s.provider = s.provider_picker.selected;
                        s.init_credential_step();
                        s.step = SetupStep::Credential;
                    }
                    _ => {
                        s.provider_picker.handle_key(key);
                    }
                },

                // ── Credential input ──────────────────────────────────────
                SetupStep::Credential => match key.code {
                    KeyCode::Esc => {
                        s.step = SetupStep::Provider;
                        s.credential_pick_active = false;
                        s.clear_typed_input();
                    }
                    KeyCode::Enter => {
                        if s.credential_pick_active {
                            let offers = s.saved_keys_for_current_provider();
                            if offers.is_empty() {
                                s.credential_pick_active = false;
                                continue;
                            }
                            let idx = s.credential_pick_index.min(offers.len().saturating_sub(1));
                            s.credential = offers[idx].api_key.clone();
                            s.clear_typed_input();
                            s.step = SetupStep::FetchingModels;
                            s.needs_fetch = true;
                            continue;
                        }
                        if !s.is_local() && s.text_input.text.is_empty() {
                            if let Some(k) = s.current_credential.clone() {
                                s.credential = k;
                                s.clear_typed_input();
                                s.step = SetupStep::FetchingModels;
                                s.needs_fetch = true;
                            } else {
                                s.error = Some(
                                    "API key required — paste your key and press Enter".into(),
                                );
                            }
                        } else {
                            s.credential = s.text_input.text.clone();
                            s.clear_typed_input();
                            s.step = SetupStep::FetchingModels;
                            s.needs_fetch = true;
                        }
                    }
                    KeyCode::Up if s.credential_pick_active => {
                        if s.credential_pick_index > 0 {
                            s.credential_pick_index -= 1;
                        }
                    }
                    KeyCode::Down if s.credential_pick_active => {
                        let max = s.saved_keys_for_current_provider().len().saturating_sub(1);
                        if s.credential_pick_index < max {
                            s.credential_pick_index += 1;
                        }
                    }
                    KeyCode::Char('n') if s.credential_pick_active => {
                        s.credential_pick_active = false;
                        s.clear_typed_input();
                    }
                    KeyCode::Backspace if !s.credential_pick_active => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete if !s.credential_pick_active => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left if !s.credential_pick_active => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right if !s.credential_pick_active => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home if !s.credential_pick_active => {
                        s.text_input.home();
                    }
                    KeyCode::End if !s.credential_pick_active => {
                        s.text_input.end();
                    }
                    KeyCode::Char(c) if !s.credential_pick_active => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── FetchingModels (no key handling — handled above) ──────
                SetupStep::FetchingModels => {}

                // ── Model list selection ──────────────────────────────────
                SetupStep::Model if s.model_list_mode() => {
                    // Update visible rows for scroll management.
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    s.model_picker.visible_rows = visible_rows;

                    match key.code {
                        KeyCode::Enter => match s.model_picker.selected_item() {
                            Some(ModelOption::Custom) => {
                                s.custom_model = true;
                                s.clear_typed_input();
                            }
                            Some(ModelOption::Entry(entry)) => {
                                if !entry.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        entry.id
                                    ));
                                } else {
                                    s.model = entry.id.clone();
                                    s.step = SetupStep::SubAgentIntro;
                                }
                            }
                            None => {}
                        },
                        KeyCode::Esc => {
                            if !s.model_picker.filter.text.is_empty() {
                                s.model_picker.filter.clear();
                                s.model_picker.apply_filter();
                                s.model_picker.home();
                            } else {
                                s.step = SetupStep::Credential;
                                s.text_input.set_text(s.credential.clone());
                                s.credential_pick_active = false;
                            }
                        }
                        _ => {
                            s.model_picker.handle_key(key);
                        }
                    }
                }

                // ── Model text input (custom or fetch failed) ─────────────
                SetupStep::Model => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.model = s.text_input.text.clone();
                            s.step = SetupStep::SubAgentIntro;
                        }
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Esc => {
                        if !s.fetched_models.is_empty() {
                            // Back to list
                            s.custom_model = false;
                            s.clear_typed_input();
                        } else {
                            // Back to credential
                            s.step = SetupStep::Credential;
                            s.text_input.set_text(s.credential.clone());
                            s.credential_pick_active = false;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    _ => {}
                },

                // ── Sub-agent intro ───────────────────────────────────────
                SetupStep::SubAgentIntro => match key.code {
                    KeyCode::Up => {
                        if s.subagent_intro_cursor > 0 {
                            s.subagent_intro_cursor -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if s.subagent_intro_cursor < SUBAGENT_OPTIONS.len() - 1 {
                            s.subagent_intro_cursor += 1;
                        }
                    }
                    KeyCode::Enter => match s.subagent_intro_cursor {
                        0 => {
                            // Worker only
                            s.configure_worker = true;
                            s.configure_reviewer = false;
                            s.worker_provider = s.provider;
                            s.worker_provider_picker.selected = s.provider;
                            s.step = SetupStep::WorkerProvider;
                        }
                        1 => {
                            // Worker + Reviewer
                            s.configure_worker = true;
                            s.configure_reviewer = true;
                            s.worker_provider = s.provider;
                            s.worker_provider_picker.selected = s.provider;
                            s.step = SetupStep::WorkerProvider;
                        }
                        _ => {
                            // Skip
                            s.step = SetupStep::Roles;
                            s.role_cursor = 0;
                            s.role_edit_field = RoleEditField::None;
                        }
                    },
                    KeyCode::Esc => {
                        // Esc = go back to model step
                        s.step = SetupStep::Model;
                    }
                    _ => {}
                },

                // ── Worker provider ───────────────────────────────────────
                SetupStep::WorkerProvider => match key.code {
                    KeyCode::Enter => {
                        s.worker_provider = s.worker_provider_picker.selected;
                        if s.worker_provider == s.provider {
                            // Same provider as main — reuse credential
                            s.worker_credential = s.credential.clone();
                            s.worker_needs_fetch = true;
                            s.step = SetupStep::FetchingWorkerModels;
                        } else {
                            s.clear_typed_input();
                            s.step = SetupStep::WorkerCredential;
                        }
                    }
                    KeyCode::Esc => {
                        s.configure_worker = false;
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    _ => {
                        s.worker_provider_picker.handle_key(key);
                    }
                },

                // ── Worker credential ─────────────────────────────────────
                SetupStep::WorkerCredential => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            if PROVIDER_REGISTRY[s.worker_provider].is_local {
                                s.worker_credential = "http://localhost:11434".to_string();
                            } else {
                                s.error = Some("API key required. Press Esc to skip.".into());
                                continue;
                            }
                        } else {
                            s.worker_credential = s.text_input.text.clone();
                        }
                        s.worker_needs_fetch = true;
                        s.clear_typed_input();
                        s.step = SetupStep::FetchingWorkerModels;
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    KeyCode::Esc => {
                        s.configure_worker = false;
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── FetchingWorkerModels (handled in fetch block above) ────
                SetupStep::FetchingWorkerModels => {}

                // ── Worker model list ─────────────────────────────────────
                SetupStep::WorkerModel if s.worker_model_list_mode() => {
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    s.worker_model_picker.visible_rows = visible_rows;
                    match key.code {
                        KeyCode::Enter => match s.worker_model_picker.selected_item() {
                            Some(ModelOption::Custom) => {
                                s.worker_custom_model = true;
                                s.clear_typed_input();
                            }
                            Some(ModelOption::Entry(entry)) => {
                                if !entry.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        entry.id
                                    ));
                                } else {
                                    s.worker_model = entry.id.clone();
                                    if s.configure_reviewer {
                                        s.reviewer_provider = s.provider;
                                        s.reviewer_provider_picker.selected = s.provider;
                                        s.step = SetupStep::ReviewerProvider;
                                    } else {
                                        s.step = SetupStep::Roles;
                                        s.role_cursor = 0;
                                        s.role_edit_field = RoleEditField::None;
                                    }
                                }
                            }
                            None => {}
                        },
                        KeyCode::Esc => {
                            if !s.worker_model_picker.filter.text.is_empty() {
                                s.worker_model_picker.filter.clear();
                                s.worker_model_picker.apply_filter();
                                s.worker_model_picker.home();
                            } else {
                                s.step = SetupStep::WorkerProvider;
                            }
                        }
                        _ => {
                            s.worker_model_picker.handle_key(key);
                        }
                    }
                }

                // ── Worker model text input ───────────────────────────────
                SetupStep::WorkerModel => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.worker_model = s.text_input.text.clone();
                            s.clear_typed_input();
                            if s.configure_reviewer {
                                s.reviewer_provider = s.provider;
                                s.step = SetupStep::ReviewerProvider;
                            } else {
                                s.step = SetupStep::Roles;
                                s.role_cursor = 0;
                                s.role_edit_field = RoleEditField::None;
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    KeyCode::Esc => {
                        if !s.worker_fetched_models.is_empty() {
                            s.worker_custom_model = false;
                            s.clear_typed_input();
                        } else {
                            s.step = SetupStep::WorkerProvider;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── Reviewer provider ─────────────────────────────────────
                SetupStep::ReviewerProvider => match key.code {
                    KeyCode::Enter => {
                        s.reviewer_provider = s.reviewer_provider_picker.selected;
                        if s.reviewer_provider == s.provider {
                            s.reviewer_credential = s.credential.clone();
                            s.reviewer_needs_fetch = true;
                            s.step = SetupStep::FetchingReviewerModels;
                        } else {
                            s.clear_typed_input();
                            s.step = SetupStep::ReviewerCredential;
                        }
                    }
                    KeyCode::Esc => {
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    _ => {
                        s.reviewer_provider_picker.handle_key(key);
                    }
                },

                // ── Reviewer credential ───────────────────────────────────
                SetupStep::ReviewerCredential => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            if PROVIDER_REGISTRY[s.reviewer_provider].is_local {
                                s.reviewer_credential = "http://localhost:11434".to_string();
                            } else {
                                s.error = Some("API key required. Press Esc to skip.".into());
                                continue;
                            }
                        } else {
                            s.reviewer_credential = s.text_input.text.clone();
                        }
                        s.reviewer_needs_fetch = true;
                        s.clear_typed_input();
                        s.step = SetupStep::FetchingReviewerModels;
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    KeyCode::Esc => {
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── FetchingReviewerModels (handled in fetch block above) ──
                SetupStep::FetchingReviewerModels => {}

                // ── Reviewer model list ───────────────────────────────────
                SetupStep::ReviewerModel if s.reviewer_model_list_mode() => {
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    s.reviewer_model_picker.visible_rows = visible_rows;
                    match key.code {
                        KeyCode::Enter => match s.reviewer_model_picker.selected_item() {
                            Some(ModelOption::Custom) => {
                                s.reviewer_custom_model = true;
                                s.clear_typed_input();
                            }
                            Some(ModelOption::Entry(entry)) => {
                                if !entry.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        entry.id
                                    ));
                                } else {
                                    s.reviewer_model = entry.id.clone();
                                    s.step = SetupStep::Roles;
                                    s.role_cursor = 0;
                                    s.role_edit_field = RoleEditField::None;
                                }
                            }
                            None => {}
                        },
                        KeyCode::Esc => {
                            if !s.reviewer_model_picker.filter.text.is_empty() {
                                s.reviewer_model_picker.filter.clear();
                                s.reviewer_model_picker.apply_filter();
                                s.reviewer_model_picker.home();
                            } else {
                                s.step = SetupStep::ReviewerProvider;
                            }
                        }
                        _ => {
                            s.reviewer_model_picker.handle_key(key);
                        }
                    }
                }

                // ── Reviewer model text input ─────────────────────────────
                SetupStep::ReviewerModel => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.reviewer_model = s.text_input.text.clone();
                            s.clear_typed_input();
                            s.step = SetupStep::Roles;
                            s.role_cursor = 0;
                            s.role_edit_field = RoleEditField::None;
                        }
                    }
                    KeyCode::Backspace => {
                        s.text_input.delete_back();
                    }
                    KeyCode::Delete => {
                        s.text_input.delete_forward();
                    }
                    KeyCode::Left => {
                        s.text_input.cursor_left();
                    }
                    KeyCode::Right => {
                        s.text_input.cursor_right();
                    }
                    KeyCode::Home => {
                        s.text_input.home();
                    }
                    KeyCode::End => {
                        s.text_input.end();
                    }
                    KeyCode::Esc => {
                        if !s.reviewer_fetched_models.is_empty() {
                            s.reviewer_custom_model = false;
                            s.clear_typed_input();
                        } else {
                            s.step = SetupStep::ReviewerProvider;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── Roles configuration ───────────────────────────────────
                SetupStep::Roles => {
                    match &s.role_edit_field {
                        RoleEditField::None => match key.code {
                            // Tab or Ctrl+Enter: finish setup
                            KeyCode::Tab | KeyCode::BackTab => {
                                return Ok(SetupOutcome::Finished(Box::new(s)));
                            }
                            // Enter on a row: begin editing the model for that role
                            KeyCode::Enter => {
                                if s.role_cursor < s.roles.len() {
                                    let model = s.roles[s.role_cursor].1.clone();
                                    s.role_input = model;
                                    s.role_edit_field = RoleEditField::Model(s.role_cursor);
                                } else {
                                    // Cursor on "Done" row
                                    return Ok(SetupOutcome::Finished(Box::new(s)));
                                }
                            }
                            KeyCode::Up => {
                                if s.role_cursor > 0 {
                                    s.role_cursor -= 1;
                                }
                            }
                            KeyCode::Down => {
                                // roles.len() = "Done" row index
                                if s.role_cursor < s.roles.len() {
                                    s.role_cursor += 1;
                                }
                            }
                            // 'n': add a new role
                            KeyCode::Char('n') => {
                                s.role_input.clear();
                                s.role_edit_field = RoleEditField::Name(usize::MAX);
                            }
                            // 'd': delete selected role
                            KeyCode::Char('d') => {
                                if s.role_cursor < s.roles.len() {
                                    s.roles.remove(s.role_cursor);
                                    if s.role_cursor > 0 && s.role_cursor >= s.roles.len() {
                                        s.role_cursor -= 1;
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                // Back to model selection (keep chosen model)
                                s.step = SetupStep::Model;
                                if !s.custom_model {
                                    s.model_picker.filter.clear();
                                    s.model_picker.apply_filter();
                                    s.model_picker.home();
                                }
                            }
                            _ => {}
                        },
                        RoleEditField::Name(_) => match key.code {
                            KeyCode::Enter => {
                                let name = s.role_input.trim().to_string();
                                if name.is_empty() {
                                    s.role_edit_field = RoleEditField::None;
                                } else {
                                    // Move to editing the model for this new role
                                    s.roles.push((name, String::new()));
                                    let idx = s.roles.len() - 1;
                                    s.role_cursor = idx;
                                    s.role_input.clear();
                                    s.role_edit_field = RoleEditField::Model(idx);
                                }
                            }
                            KeyCode::Backspace => {
                                s.role_input.pop();
                            }
                            KeyCode::Esc => {
                                s.role_edit_field = RoleEditField::None;
                                s.role_input.clear();
                            }
                            KeyCode::Char(c) => {
                                s.role_input.push(c);
                            }
                            _ => {}
                        },
                        RoleEditField::Model(idx) => {
                            let idx = *idx;
                            match key.code {
                                KeyCode::Enter => {
                                    let model = s.role_input.trim().to_string();
                                    if model.is_empty() {
                                        // Remove the role if no model given
                                        if idx < s.roles.len() {
                                            s.roles.remove(idx);
                                        }
                                    } else if idx < s.roles.len() {
                                        s.roles[idx].1 = model;
                                    }
                                    s.role_edit_field = RoleEditField::None;
                                    s.role_input.clear();
                                }
                                KeyCode::Backspace => {
                                    s.role_input.pop();
                                }
                                KeyCode::Esc => {
                                    // Cancel edit — remove if model is still empty
                                    if idx < s.roles.len() && s.roles[idx].1.is_empty() {
                                        s.roles.remove(idx);
                                    }
                                    s.role_edit_field = RoleEditField::None;
                                    s.role_input.clear();
                                }
                                KeyCode::Char(c) => {
                                    s.role_input.push(c);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}
