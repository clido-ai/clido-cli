use super::*;

// ── Profile overlay keyboard handler ─────────────────────────────────────────

pub fn handle_profile_overlay_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode::*;
    use crossterm::event::KeyModifiers as Km;

    let st = match app.profile_overlay.as_mut() {
        Some(s) => s,
        None => return,
    };
    st.status = None;

    match &st.mode.clone() {
        // ── Creating: step-by-step wizard ─────────────────────────────────
        ProfileOverlayMode::Creating { step } => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                    app.push(ChatLine::Info("  Profile creation cancelled.".into()));
                }
                Enter => {
                    let value = st.input.trim().to_string();
                    match step {
                        ProfileCreateStep::Name => {
                            use super::super::state::ProfileCreateNameChoice;
                            match st.profile_create_name_choice {
                                ProfileCreateNameChoice::AutoGenerate => {
                                    st.name.clear();
                                    st.input.clear();
                                    st.input_cursor = 0;
                                    st.provider_picker = ProviderPickerState::new();
                                    st.provider_picker.clamp();
                                    st.mode = ProfileOverlayMode::Creating {
                                        step: ProfileCreateStep::Provider,
                                    };
                                }
                                ProfileCreateNameChoice::TypeCustomName => {
                                    if value.is_empty() {
                                        st.status = Some(
                                            "  ✗ Enter a name, or press Up for auto-generated name"
                                                .into(),
                                        );
                                        return;
                                    }
                                    let dup = clido_core::load_config(&app.workspace_root)
                                        .ok()
                                        .map(|c| c.profiles.contains_key(&value))
                                        .unwrap_or(false);
                                    if dup {
                                        st.status =
                                            Some(format!("  ✗ Profile '{}' already exists", value));
                                        return;
                                    }
                                    st.name = value;
                                    st.input.clear();
                                    st.input_cursor = 0;
                                    st.provider_picker = ProviderPickerState::new();
                                    st.provider_picker.clamp();
                                    st.mode = ProfileOverlayMode::Creating {
                                        step: ProfileCreateStep::Provider,
                                    };
                                }
                            }
                        }
                        ProfileCreateStep::Provider => {
                            if let Some(id) = st.provider_picker.selected_id() {
                                st.provider = id.to_string();
                                // Auto-generate profile name only if user left it blank.
                                if st.name.is_empty() {
                                    let existing = clido_core::load_config(&app.workspace_root)
                                        .ok()
                                        .map(|c| c.profiles.keys().cloned().collect::<Vec<_>>())
                                        .unwrap_or_default();
                                    let base = id.to_string();
                                    if !existing.contains(&base) {
                                        st.name = base;
                                    } else {
                                        let mut n = 2u32;
                                        loop {
                                            let candidate = format!("{base}-{n}");
                                            if !existing.contains(&candidate) {
                                                st.name = candidate;
                                                break;
                                            }
                                            n += 1;
                                        }
                                    }
                                }
                                let needs_key = st.provider_picker.selected_requires_key();
                                let mut picker = ModelPickerState {
                                    models: vec![],
                                    filter: String::new(),
                                    selected: 0,
                                    scroll_offset: 0,
                                };
                                picker.clamp();
                                st.profile_model_picker = Some(picker);
                                st.input.clear();
                                st.input_cursor = 0;
                                // Check credentials file for an existing key for this provider.
                                let saved_key = if needs_key {
                                    crate::setup::read_credential(&st.config_path, id)
                                } else {
                                    None
                                };
                                // Check if this provider requires a custom base URL step.
                                let provider_needs_base_url = clido_providers::PROVIDER_REGISTRY
                                    .iter()
                                    .find(|d| d.id == id)
                                    .map(|d| d.needs_base_url)
                                    .unwrap_or(false);

                                let next_step = if provider_needs_base_url {
                                    // Leave input empty — placeholder shows the default.
                                    // Empty = use registry default; typed value = custom endpoint.
                                    st.input.clear();
                                    st.input_cursor = 0;
                                    ProfileCreateStep::BaseUrl
                                } else if let Some(key) = saved_key {
                                    // Pre-fill the key and skip to model selection.
                                    st.api_key = key.clone();
                                    // Trigger model fetch with the saved key.
                                    spawn_model_fetch(
                                        st.provider.clone(),
                                        key,
                                        None,
                                        app.channels.fetch_tx.clone(),
                                    );
                                    app.models_loading = true;
                                    ProfileCreateStep::Model
                                } else if needs_key {
                                    ProfileCreateStep::ApiKey
                                } else {
                                    st.api_key.clear();
                                    ProfileCreateStep::Model
                                };
                                // Filter saved keys to only those matching the selected provider.
                                st.saved_keys.retain(|k| k.provider_id == st.provider);
                                if next_step == ProfileCreateStep::ApiKey
                                    && !st.saved_keys.is_empty()
                                {
                                    st.mode = ProfileOverlayMode::PickingSavedKey {
                                        selected: 0,
                                        show_type_new_row: true,
                                    };
                                } else {
                                    st.mode = ProfileOverlayMode::Creating { step: next_step };
                                }
                            } else {
                                st.status = Some("  ✗ Select a provider from the list".into());
                            }
                        }
                        ProfileCreateStep::BaseUrl => {
                            // Save the custom base URL (may be empty to use default).
                            st.base_url = value.trim().to_string();
                            st.input.clear();
                            st.input_cursor = 0;
                            let needs_key = clido_providers::PROVIDER_REGISTRY
                                .iter()
                                .find(|d| d.id == st.provider.as_str())
                                .map(|d| !d.id.eq_ignore_ascii_case("local"))
                                .unwrap_or(true);
                            // Check for saved key
                            let saved_key = if needs_key {
                                crate::setup::read_credential(&st.config_path, &st.provider.clone())
                            } else {
                                None
                            };
                            if let Some(key) = saved_key {
                                st.api_key = key.clone();
                                let base_url_for_fetch = if st.base_url.is_empty() {
                                    None
                                } else {
                                    Some(st.base_url.clone())
                                };
                                spawn_model_fetch(
                                    st.provider.clone(),
                                    key,
                                    base_url_for_fetch,
                                    app.channels.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                                st.mode = ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Model,
                                };
                            } else if needs_key {
                                // Filter saved keys to only those matching the selected provider.
                                st.saved_keys.retain(|k| k.provider_id == st.provider);
                                if !st.saved_keys.is_empty() {
                                    st.mode = ProfileOverlayMode::PickingSavedKey {
                                        selected: 0,
                                        show_type_new_row: true,
                                    };
                                } else {
                                    st.mode = ProfileOverlayMode::Creating {
                                        step: ProfileCreateStep::ApiKey,
                                    };
                                }
                            } else {
                                // No key needed (local provider)
                                st.api_key.clear();
                                st.mode = ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Model,
                                };
                            }
                        }
                        ProfileCreateStep::ApiKey => {
                            // API key may be empty for local providers
                            st.api_key = value.clone();
                            st.input.clear();
                            st.input_cursor = 0;
                            // Trigger a live model fetch for this provider + key so the model
                            // picker is populated when the user reaches the model selection step.
                            let provider_for_fetch = st.provider.clone();
                            let base_url_for_fetch = if st.base_url.is_empty() {
                                None
                            } else {
                                Some(st.base_url.clone())
                            };
                            if !value.is_empty() {
                                spawn_model_fetch(
                                    provider_for_fetch,
                                    value,
                                    base_url_for_fetch,
                                    app.channels.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                            }
                            st.mode = ProfileOverlayMode::Creating {
                                step: ProfileCreateStep::Model,
                            };
                        }
                        ProfileCreateStep::Model => {
                            // Prefer selected item from picker; fall back to whatever is typed
                            // in the filter box so users can always type a model ID manually.
                            let model_id = st
                                .profile_model_picker
                                .as_ref()
                                .and_then(|p| {
                                    let filtered = p.filtered();
                                    filtered.get(p.selected).map(|m| m.id.clone())
                                })
                                .or_else(|| {
                                    // Use the filter text as a literal model ID if no picker match.
                                    st.profile_model_picker
                                        .as_ref()
                                        .map(|p| p.filter.trim().to_string())
                                        .filter(|s| !s.is_empty())
                                });
                            if let Some(id) = model_id {
                                st.model = id;
                                st.input.clear();
                                st.input_cursor = 0;
                                st.mode = ProfileOverlayMode::Overview;
                                let Some(st) = app.profile_overlay.as_mut() else {
                                    return;
                                };
                                st.save();
                                let name = st.name.clone();
                                let msg = st
                                    .status
                                    .clone()
                                    .unwrap_or_else(|| format!("  ✓ Profile '{}' created", name));
                                app.push(ChatLine::Info(msg));
                                app.profile_overlay = None;
                                super::super::commands::switch_profile_seamless(app, &name);
                            } else {
                                st.status = Some("  ✗ Type a model name and press Enter".into());
                            }
                        }
                    }
                }
                Backspace => {
                    match step {
                        ProfileCreateStep::Name => {
                            if st.profile_create_name_choice
                                == super::super::state::ProfileCreateNameChoice::TypeCustomName
                                && !st.input.is_empty()
                            {
                                if event.modifiers.contains(Km::CONTROL) {
                                    while st.input_cursor > 0 {
                                        let b = char_byte_pos_tui(&st.input, st.input_cursor - 1);
                                        let ch = st.input[..b].chars().last();
                                        if ch.map(|c| c == ' ').unwrap_or(false)
                                            && st.input_cursor > 1
                                        {
                                            break;
                                        }
                                        st.input_cursor -= 1;
                                        let pos = char_byte_pos_tui(&st.input, st.input_cursor);
                                        st.input.remove(pos);
                                    }
                                } else {
                                    delete_char_before_cursor_pe(st);
                                }
                            }
                        }
                        ProfileCreateStep::Provider => {
                            st.provider_picker.filter.pop();
                            st.provider_picker.clamp();
                        }
                        ProfileCreateStep::Model => {
                            if let Some(ref mut picker) = st.profile_model_picker {
                                picker.filter.pop();
                                picker.clamp();
                            }
                        }
                        _ => {
                            if !st.input.is_empty() {
                                if event.modifiers.contains(Km::CONTROL) {
                                    // Ctrl+Backspace: delete word
                                    while st.input_cursor > 0 {
                                        let b = char_byte_pos_tui(&st.input, st.input_cursor - 1);
                                        let ch = st.input[..b].chars().last();
                                        if ch.map(|c| c == ' ').unwrap_or(false)
                                            && st.input_cursor > 1
                                        {
                                            break;
                                        }
                                        st.input_cursor -= 1;
                                        let pos = char_byte_pos_tui(&st.input, st.input_cursor);
                                        st.input.remove(pos);
                                    }
                                } else {
                                    delete_char_before_cursor_pe(st);
                                }
                            }
                        }
                    }
                }
                Delete => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        delete_char_at_cursor_pe(st);
                    }
                },
                Left => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor > 0 {
                            st.input_cursor -= 1;
                        }
                    }
                },
                Right => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        if st.input_cursor < st.input.chars().count() {
                            st.input_cursor += 1;
                        }
                    }
                },
                Home => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = 0;
                    }
                },
                End => match step {
                    ProfileCreateStep::Provider | ProfileCreateStep::Model => {}
                    _ => {
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Up => match step {
                    ProfileCreateStep::Name => {
                        st.profile_create_name_choice =
                            super::super::state::ProfileCreateNameChoice::AutoGenerate;
                    }
                    ProfileCreateStep::Provider => {
                        if st.provider_picker.selected > 0 {
                            st.provider_picker.selected -= 1;
                            if st.provider_picker.selected < st.provider_picker.scroll_offset {
                                st.provider_picker.scroll_offset = st.provider_picker.selected;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            if picker.selected > 0 {
                                picker.selected -= 1;
                                if picker.selected < picker.scroll_offset {
                                    picker.scroll_offset = picker.selected;
                                }
                            }
                        }
                    }
                    _ => {
                        // Text steps (e.g. API key): jump to start of line.
                        st.input_cursor = 0;
                    }
                },
                Down => match step {
                    ProfileCreateStep::Name => {
                        st.profile_create_name_choice =
                            super::super::state::ProfileCreateNameChoice::TypeCustomName;
                    }
                    ProfileCreateStep::Provider => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        let n = st.provider_picker.filtered().len();
                        if n > 0 && st.provider_picker.selected + 1 < n {
                            st.provider_picker.selected += 1;
                            if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis
                            {
                                st.provider_picker.scroll_offset =
                                    st.provider_picker.selected + 1 - vis;
                            }
                        }
                    }
                    ProfileCreateStep::Model => {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        if let Some(ref mut picker) = st.profile_model_picker {
                            let n = picker.filtered().len();
                            if n > 0 && picker.selected + 1 < n {
                                picker.selected += 1;
                                if picker.selected >= picker.scroll_offset + vis {
                                    picker.scroll_offset = picker.selected + 1 - vis;
                                }
                            }
                        }
                    }
                    _ => {
                        // Text steps: jump to end of line.
                        st.input_cursor = st.input.chars().count();
                    }
                },
                Char(c) => match step {
                    ProfileCreateStep::Name => {
                        use super::super::state::ProfileCreateNameChoice;
                        if st.profile_create_name_choice == ProfileCreateNameChoice::AutoGenerate {
                            st.profile_create_name_choice = ProfileCreateNameChoice::TypeCustomName;
                        }
                        let b = char_byte_pos_tui(&st.input, st.input_cursor);
                        st.input.insert(b, c);
                        st.input_cursor += 1;
                    }
                    ProfileCreateStep::Provider => {
                        st.provider_picker.filter.push(c);
                        st.provider_picker.clamp();
                    }
                    ProfileCreateStep::Model => {
                        if let Some(ref mut picker) = st.profile_model_picker {
                            picker.filter.push(c);
                            picker.clamp();
                        }
                    }
                    ProfileCreateStep::ApiKey if c == 'k' && !st.saved_keys.is_empty() => {
                        st.mode = ProfileOverlayMode::PickingSavedKey {
                            selected: 0,
                            show_type_new_row: true,
                        };
                        st.status = None;
                    }
                    _ => {
                        let prev_len = st.input.len();
                        let b = char_byte_pos_tui(&st.input, st.input_cursor);
                        st.input.insert(b, c);
                        st.input_cursor += 1;
                        // Trigger live key validation once the input looks like a
                        // complete API key (crosses 20-char threshold or was just pasted).
                        // Skip validation on the BaseUrl step.
                        const MIN_KEY_LEN: usize = 20;
                        if *step == ProfileCreateStep::ApiKey
                            && prev_len < MIN_KEY_LEN
                            && st.input.len() >= MIN_KEY_LEN
                            && !app.models_loading
                        {
                            st.status = Some("  Validating…".to_string());
                            spawn_model_fetch(
                                st.provider.clone(),
                                st.input.trim().to_string(),
                                if st.base_url.is_empty() {
                                    None
                                } else {
                                    Some(st.base_url.clone())
                                },
                                app.channels.fetch_tx.clone(),
                            );
                            app.models_loading = true;
                        }
                    }
                },
                _ => {}
            }
        }

        // ── PickingProvider: structured provider picker ─────────────────────
        ProfileOverlayMode::PickingProvider { .. } => match event.code {
            Esc => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.provider_picker = ProviderPickerState::new();
                    st.mode = ProfileOverlayMode::Overview;
                }
            }
            Enter => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.commit_provider_pick();
                    st.save();
                    let name = st.name.clone();
                    let provider = st.provider.clone();
                    let model = st.model.clone();
                    if app.current_profile == name {
                        app.provider = provider;
                        app.model = model;
                    }
                }
            }
            Up => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    if st.provider_picker.selected > 0 {
                        st.provider_picker.selected -= 1;
                        if st.provider_picker.selected < st.provider_picker.scroll_offset {
                            st.provider_picker.scroll_offset = st.provider_picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    let n = st.provider_picker.filtered().len();
                    if n > 0 && st.provider_picker.selected + 1 < n {
                        st.provider_picker.selected += 1;
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        if st.provider_picker.selected >= st.provider_picker.scroll_offset + vis {
                            st.provider_picker.scroll_offset =
                                st.provider_picker.selected + 1 - vis;
                        }
                    }
                }
            }
            Char(c) => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.provider_picker.filter.push(c);
                    st.provider_picker.clamp();
                }
            }
            Backspace => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.provider_picker.filter.pop();
                    st.provider_picker.clamp();
                }
            }
            _ => {}
        },

        // ── PickingModel: structured model picker ────────────────────────────
        ProfileOverlayMode::PickingModel { .. } => match event.code {
            Esc => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.profile_model_picker = None;
                    st.mode = ProfileOverlayMode::Overview;
                }
            }
            Enter => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    st.commit_model_pick();
                    st.save();
                    let name = st.name.clone();
                    let provider = st.provider.clone();
                    let model = st.model.clone();
                    if app.current_profile == name {
                        app.provider = provider;
                        app.model = model;
                    }
                }
            }
            Up => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    if let Some(ref mut picker) = st.profile_model_picker {
                        if picker.selected > 0 {
                            picker.selected -= 1;
                            if picker.selected < picker.scroll_offset {
                                picker.scroll_offset = picker.selected;
                            }
                        }
                    }
                }
            }
            Down => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    if let Some(ref mut picker) = st.profile_model_picker {
                        let vis = crossterm::terminal::size()
                            .map(|(_, h)| (h as usize).saturating_sub(12).max(3))
                            .unwrap_or(20);
                        let n = picker.filtered().len();
                        if n > 0 && picker.selected + 1 < n {
                            picker.selected += 1;
                            if picker.selected >= picker.scroll_offset + vis {
                                picker.scroll_offset = picker.selected + 1 - vis;
                            }
                        }
                    }
                }
            }
            Char(c) => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    if let Some(ref mut picker) = st.profile_model_picker {
                        picker.filter.push(c);
                        picker.clamp();
                    }
                }
            }
            Backspace => {
                if let Some(st) = app.profile_overlay.as_mut() {
                    if let Some(ref mut picker) = st.profile_model_picker {
                        picker.filter.pop();
                        picker.clamp();
                    }
                }
            }
            _ => {}
        },

        // ── PickingSavedKey: choose a saved API key ─────────────────────────
        ProfileOverlayMode::PickingSavedKey {
            selected,
            show_type_new_row,
        } => {
            let n_keys = st.saved_keys.len();
            let last_idx = if *show_type_new_row {
                n_keys
            } else {
                n_keys.saturating_sub(1)
            };
            match event.code {
                Esc => {
                    st.mode = ProfileOverlayMode::Creating {
                        step: ProfileCreateStep::ApiKey,
                    };
                    st.status = None;
                }
                Enter => {
                    let idx = *selected;
                    if *show_type_new_row && n_keys > 0 && idx == n_keys {
                        st.mode = ProfileOverlayMode::Creating {
                            step: ProfileCreateStep::ApiKey,
                        };
                        st.status = None;
                        return;
                    }
                    if *show_type_new_row && n_keys == 0 && idx == 0 {
                        st.mode = ProfileOverlayMode::Creating {
                            step: ProfileCreateStep::ApiKey,
                        };
                        st.status = None;
                        return;
                    }
                    if idx < st.saved_keys.len() {
                        let offer = &st.saved_keys[idx];
                        let all_profiles = clido_core::load_config(&app.workspace_root)
                            .map(|c| c.profiles)
                            .unwrap_or_default();
                        if let Some(src_entry) = all_profiles.get(&offer.source_profile) {
                            let real_key =
                                crate::setup::read_credential(&st.config_path, &offer.provider_id)
                                    .or_else(|| {
                                        src_entry
                                            .api_key_env
                                            .as_ref()
                                            .and_then(|e| std::env::var(e).ok())
                                    })
                                    .or_else(|| src_entry.api_key.clone())
                                    .unwrap_or_default();
                            if !real_key.is_empty() {
                                st.api_key = real_key;
                                spawn_model_fetch(
                                    st.provider.clone(),
                                    st.api_key.clone(),
                                    if st.base_url.is_empty() {
                                        None
                                    } else {
                                        Some(st.base_url.clone())
                                    },
                                    app.channels.fetch_tx.clone(),
                                );
                                app.models_loading = true;
                                st.mode = ProfileOverlayMode::Creating {
                                    step: ProfileCreateStep::Model,
                                };
                                st.status = None;
                            } else {
                                st.status = Some("  ✗ Could not retrieve saved key".into());
                            }
                        } else {
                            st.status = Some("  ✗ Source profile not found".into());
                        }
                    }
                }
                Up => {
                    if *selected > 0 {
                        if let ProfileOverlayMode::PickingSavedKey { selected: s, .. } =
                            &mut st.mode
                        {
                            *s -= 1;
                        }
                    }
                }
                Down => {
                    if *selected < last_idx {
                        if let ProfileOverlayMode::PickingSavedKey { selected: s, .. } =
                            &mut st.mode
                        {
                            *s += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        // ── Overview: navigate fields ──────────────────────────────────────
        ProfileOverlayMode::Overview => {
            match event.code {
                Esc => {
                    app.profile_overlay = None;
                }
                Up => {
                    if st.cursor > 0 {
                        st.cursor -= 1;
                    }
                }
                Down => {
                    if st.cursor + 1 < ProfileOverlayState::field_count() {
                        st.cursor += 1;
                    }
                }
                Enter => {
                    let models: Vec<ModelEntry> = app.known_models.clone();
                    if let Some(st) = app.profile_overlay.as_mut() {
                        st.begin_edit(&models);
                    }
                }
                crossterm::event::KeyCode::Char('s') if event.modifiers.contains(Km::CONTROL) => {
                    let Some(st) = app.profile_overlay.as_mut() else {
                        return;
                    };
                    st.save();
                    // Update live app state if it's the active profile
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            app.profile_overlay = None;
                            super::super::commands::reload_active_profile_in_agent(app, &name);
                            app.push(ChatLine::Info(
                                "  ↻ Profile updated — agent reloaded with new credentials".into(),
                            ));
                        } else {
                            // Only model changed — live-switch
                            let _ = app.channels.model_switch_tx.send(new_model);
                        }
                    }
                }
                _ => {}
            }
        }

        // ── EditField: typing into a single field ──────────────────────────
        ProfileOverlayMode::EditField(_) => {
            match event.code {
                Esc => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        st.cancel_edit();
                    }
                }
                Enter => {
                    let Some(st) = app.profile_overlay.as_mut() else {
                        return;
                    };
                    st.commit_edit();
                    // Auto-save on field commit
                    st.save();
                    let name = st.name.clone();
                    let new_provider = st.provider.clone();
                    let new_model = st.model.clone();
                    let new_api_key = st.api_key.clone();
                    if app.current_profile == name {
                        let provider_changed = app.provider != new_provider;
                        let key_changed = app.api_key != new_api_key;
                        app.provider = new_provider;
                        app.model = new_model.clone();
                        app.api_key = new_api_key;
                        if provider_changed || key_changed {
                            app.profile_overlay = None;
                            super::super::commands::reload_active_profile_in_agent(app, &name);
                            app.push(ChatLine::Info(
                                "  ↻ Profile updated — agent reloaded with new credentials".into(),
                            ));
                        } else {
                            let _ = app.channels.model_switch_tx.send(new_model);
                        }
                    }
                }
                Backspace => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        delete_char_before_cursor_pe(st);
                    }
                }
                Delete => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        delete_char_at_cursor_pe(st);
                    }
                }
                Left => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        if st.input_cursor > 0 {
                            st.input_cursor -= 1;
                        }
                    }
                }
                Right => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        if st.input_cursor < st.input.chars().count() {
                            st.input_cursor += 1;
                        }
                    }
                }
                Home => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        st.input_cursor = 0;
                    }
                }
                End => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        st.input_cursor = st.input.chars().count();
                    }
                }
                Char(c) => {
                    if let Some(st) = app.profile_overlay.as_mut() {
                        let b = char_byte_pos_tui(&st.input, st.input_cursor);
                        st.input.insert(b, c);
                        st.input_cursor += 1;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Delete the character before the cursor in `ProfileOverlayState.input`.
pub fn delete_char_before_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor == 0 || st.input.is_empty() {
        return;
    }
    st.input_cursor -= 1;
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// Delete the character at the cursor in `ProfileOverlayState.input`.
pub fn delete_char_at_cursor_pe(st: &mut ProfileOverlayState) {
    if st.input_cursor >= st.input.chars().count() {
        return;
    }
    let b = char_byte_pos_tui(&st.input, st.input_cursor);
    st.input.remove(b);
}

/// char_byte_pos for ProfileOverlayState (same logic as the TUI char_byte_pos helper).
pub fn char_byte_pos_tui(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}
