use super::*;

/// Process app-level actions returned by overlays.
pub fn handle_app_action(app: &mut App, action: AppAction) {
    match action {
        AppAction::SwitchModel { model_id, save } => {
            let _ = app.channels.model_switch_tx.send(model_id.clone());
            app.model = model_id;
            if save {
                // persist to config
            }
        }
        AppAction::SwitchProfile { profile_name } => {
            super::commands::switch_profile_seamless(app, &profile_name);
        }
        AppAction::ResumeSession { session_id } => {
            let _ = app.channels.resume_tx.send(session_id);
        }
        AppAction::GrantPermission(_grant) => {
            // Handled through the permission modal and `pending_perm` in the main loop; this
            // variant is reserved for programmatic flows that send `AppAction` directly.
        }
        AppAction::ShowError(msg) => {
            app.overlay_stack
                .push(OverlayKind::Error(ErrorOverlay::new(msg)));
        }
        AppAction::RunCommand(cmd) => {
            execute_slash(app, &cmd);
        }
        AppAction::Quit => {
            app.quit = true;
        }
    }
}

// ── Input handling ────────────────────────────────────────────────────────────
