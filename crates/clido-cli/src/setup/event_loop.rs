//! TUI event loop for the setup wizard.

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{backend::CrosstermBackend, Terminal};

use clido_providers::registry::PROVIDER_REGISTRY;

use super::render::draw_setup;
use super::types::{
    make_model_picker, ModelOption, SetupOutcome, SetupPreFill, SetupState, SetupStep,
};

use super::FAST_PROVIDER_OPTIONS;

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
            let provider = clido_providers::build_provider(
                provider_id,
                api_key.to_string(),
                "placeholder".to_string(),
                base_url,
            )
            .ok();
            s.fetched_models = if let Some(p) = provider {
                handle
                    .block_on(p.list_models_metadata())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            s.model_picker = make_model_picker(&s.fetched_models);
            // If reinit, pre-select the current model in the list.
            if !s.current_model.is_empty() {
                if let Some(idx) =
                    s.model_picker.items().iter().position(
                        |o| matches!(o, ModelOption::Metadata(m) if m.id == s.current_model),
                    )
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

        // Fetch fast provider models.
        if s.fast_needs_fetch {
            s.fast_needs_fetch = false;
            let provider_id = PROVIDER_REGISTRY[s.fast_provider_idx].id;
            let is_local_fast = PROVIDER_REGISTRY[s.fast_provider_idx].is_local;
            let (api_key, base_url): (&str, Option<&str>) = if is_local_fast {
                ("", Some(s.fast_credential.as_str()))
            } else {
                // Use typed credential if available, otherwise use saved credential
                let key = if s.fast_credential.is_empty() {
                    s.current_fast_credential.as_deref().unwrap_or("")
                } else {
                    s.fast_credential.as_str()
                };
                (key, None)
            };
            let handle = tokio::runtime::Handle::current();
            let provider = clido_providers::build_provider(
                provider_id,
                api_key.to_string(),
                "placeholder".to_string(),
                base_url,
            )
            .ok();
            s.fast_fetched_models = if let Some(p) = provider {
                handle
                    .block_on(p.list_models_metadata())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            s.fast_custom_model = s.fast_fetched_models.is_empty();
            s.fast_model_picker = make_model_picker(&s.fast_fetched_models);
            s.clear_typed_input();
            s.step = SetupStep::FastModel;
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
                            Some(ModelOption::Metadata(m)) => {
                                if !m.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        m.id
                                    ));
                                } else {
                                    s.model = m.id.clone();
                                    s.step = SetupStep::FastProviderIntro;
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
                            s.step = SetupStep::FastProviderIntro;
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

                // ── Fast provider intro ───────────────────────────────────
                SetupStep::FastProviderIntro => match key.code {
                    KeyCode::Up => {
                        if s.fast_intro_cursor > 0 {
                            s.fast_intro_cursor -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if s.fast_intro_cursor < FAST_PROVIDER_OPTIONS.len() - 1 {
                            s.fast_intro_cursor += 1;
                        }
                    }
                    KeyCode::Enter => match s.fast_intro_cursor {
                        0 => {
                            // Configure fast provider
                            s.configure_fast = true;
                            s.fast_provider_idx = s.provider;
                            s.fast_provider_picker.selected = s.provider;
                            s.step = SetupStep::FastProvider;
                        }
                        _ => {
                            // Skip
                            return Ok(SetupOutcome::Finished(Box::new(s)));
                        }
                    },
                    KeyCode::Esc => {
                        // Esc = go back to model step
                        s.step = SetupStep::Model;
                    }
                    _ => {}
                },

                // ── Fast provider selection ───────────────────────────────
                SetupStep::FastProvider => match key.code {
                    KeyCode::Enter => {
                        s.fast_provider_idx = s.fast_provider_picker.selected;
                        if s.fast_provider_idx == s.provider {
                            // Same provider as main — reuse credential
                            s.fast_credential = s.credential.clone();
                            s.current_fast_credential = s.current_credential.clone();
                            s.fast_needs_fetch = true;
                            s.step = SetupStep::FetchingFastModels;
                        } else {
                            s.clear_typed_input();
                            s.init_fast_credential_step();
                            s.step = SetupStep::FastCredential;
                        }
                    }
                    KeyCode::Esc => {
                        s.configure_fast = false;
                        return Ok(SetupOutcome::Finished(Box::new(s)));
                    }
                    _ => {
                        s.fast_provider_picker.handle_key(key);
                    }
                },

                // ── Fast credential ───────────────────────────────────────
                SetupStep::FastCredential => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            if PROVIDER_REGISTRY[s.fast_provider_idx].is_local {
                                s.fast_credential = "http://localhost:11434".to_string();
                            } else {
                                s.error = Some("API key required. Press Esc to skip.".into());
                                continue;
                            }
                        } else {
                            s.fast_credential = s.text_input.text.clone();
                        }
                        s.fast_needs_fetch = true;
                        s.clear_typed_input();
                        s.step = SetupStep::FetchingFastModels;
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
                        s.configure_fast = false;
                        return Ok(SetupOutcome::Finished(Box::new(s)));
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },

                // ── FetchingFastModels (handled in fetch block above) ─────
                SetupStep::FetchingFastModels => {}

                // ── Fast model list ───────────────────────────────────────
                SetupStep::FastModel if s.fast_model_list_mode() => {
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    s.fast_model_picker.visible_rows = visible_rows;
                    match key.code {
                        KeyCode::Enter => match s.fast_model_picker.selected_item() {
                            Some(ModelOption::Custom) => {
                                s.fast_custom_model = true;
                                s.clear_typed_input();
                            }
                            Some(ModelOption::Metadata(m)) => {
                                if !m.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        m.id
                                    ));
                                } else {
                                    s.fast_model = m.id.clone();
                                    return Ok(SetupOutcome::Finished(Box::new(s)));
                                }
                            }
                            None => {}
                        },
                        KeyCode::Esc => {
                            if !s.fast_model_picker.filter.text.is_empty() {
                                s.fast_model_picker.filter.clear();
                                s.fast_model_picker.apply_filter();
                                s.fast_model_picker.home();
                            } else {
                                s.step = SetupStep::FastProvider;
                            }
                        }
                        _ => {
                            s.fast_model_picker.handle_key(key);
                        }
                    }
                }

                // ── Fast model text input ─────────────────────────────────
                SetupStep::FastModel => match key.code {
                    KeyCode::Enter => {
                        if s.text_input.text.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.fast_model = s.text_input.text.clone();
                            s.clear_typed_input();
                            return Ok(SetupOutcome::Finished(Box::new(s)));
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
                        if !s.fast_fetched_models.is_empty() {
                            s.fast_custom_model = false;
                            s.clear_typed_input();
                        } else {
                            s.step = SetupStep::FastProvider;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.text_input.insert_char(c);
                    }
                    _ => {}
                },
            }
        }
    }
}
