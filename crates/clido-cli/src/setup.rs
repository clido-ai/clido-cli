//! Interactive setup flow: first-run, `clido init`, provider + model selection.
//!
//! Flow: choose provider → enter API key / base URL → fetch models from API → choose model.
//!
//! TTY  → full-screen ratatui TUI.
//! No TTY → plain stdin/stdout (CI, pipes).

use std::env;
use std::io::{self, BufRead};
use std::path::PathBuf;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};

use crate::errors::CliError;
use crate::ui::{setup_use_color, setup_use_rich_ui, SETUP_BANNER_ASCII};

use clido_providers::ModelEntry;

// ── Provider metadata ─────────────────────────────────────────────────────────

/// (display name, internal provider ID, description)
const PROVIDERS: [(&str, &str, &str); 6] = [
    (
        "OpenRouter",
        "openrouter",
        "access any model — openrouter.ai",
    ),
    (
        "Anthropic",
        "anthropic",
        "Claude models — console.anthropic.com",
    ),
    ("OpenAI", "openai", "GPT & o-series — platform.openai.com"),
    ("Mistral", "mistral", "Mistral models — console.mistral.ai"),
    (
        "Alibaba Cloud",
        "alibabacloud",
        "Qwen models — dashscope.aliyuncs.com",
    ),
    (
        "Local / Ollama",
        "local",
        "no key needed, runs on your machine",
    ),
];

const PROVIDER_KEY_ENV: [&str; 6] = [
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "MISTRAL_API_KEY",
    "DASHSCOPE_API_KEY",
    "", // local: no key
];

/// Current config values passed to the setup wizard when re-running from TUI (/init).
/// Each field is optional — empty string = nothing known.
pub struct SetupPreFill {
    /// Current provider ID (e.g. "anthropic", "openrouter").
    pub provider: String,
    /// Current API key (shown masked, Enter to keep).
    pub api_key: String,
    /// Current model ID (pre-selected in list).
    pub model: String,
    /// Current roles (pre-populated in roles step).
    pub roles: Vec<(String, String)>,
    /// Profile name (used in profile-create / profile-edit flows).
    pub profile_name: String,
    /// True when creating a brand-new named profile (shows ProfileName step first).
    pub is_new_profile: bool,
}

// ── TUI setup state ───────────────────────────────────────────────────────────

/// Setup steps: [ProfileName →] Provider → Credential → FetchModels → Model → SubAgentIntro → [Worker] → [Reviewer] → Roles → Done.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SetupStep {
    /// New named profile: ask for a profile name first.
    ProfileName,
    // Main agent
    Provider,
    Credential,
    FetchingModels,
    Model,
    // Sub-agents (optional)
    SubAgentIntro,
    WorkerProvider,
    WorkerCredential,
    FetchingWorkerModels,
    WorkerModel,
    ReviewerProvider,
    ReviewerCredential,
    FetchingReviewerModels,
    ReviewerModel,
    // Roles (optional)
    Roles,
}

struct SetupState {
    step: SetupStep,
    needs_fetch: bool,
    /// Profile name when in profile-create / profile-edit mode.
    profile_name: String,
    provider_cursor: usize,
    model_cursor: usize,
    model_scroll: usize,
    model_search: String,
    custom_model: bool,
    provider: usize,
    credential: String,
    model: String,
    input: String,
    fetched_models: Vec<ModelEntry>,
    // ── Roles step ────────────────────────────────────────────
    roles: Vec<(String, String)>, // (role_name, model_id)
    role_cursor: usize,
    role_edit_field: RoleEditField,
    role_input: String, // text being typed in a role field
    // ── Sub-agent configuration ────────────────────────────────
    subagent_intro_cursor: usize,
    configure_worker: bool,
    configure_reviewer: bool,
    worker_provider: usize,
    worker_credential: String,
    worker_model: String,
    worker_fetched_models: Vec<ModelEntry>,
    worker_model_cursor: usize,
    worker_model_scroll: usize,
    worker_model_search: String,
    worker_custom_model: bool,
    reviewer_provider: usize,
    reviewer_credential: String,
    reviewer_model: String,
    reviewer_fetched_models: Vec<ModelEntry>,
    reviewer_model_cursor: usize,
    reviewer_model_scroll: usize,
    reviewer_model_search: String,
    reviewer_custom_model: bool,
    worker_needs_fetch: bool,
    reviewer_needs_fetch: bool,
    // ──────────────────────────────────────────────────────────
    error: Option<String>,
    /// Stored credential from pre-fill (kept so user can press Enter to keep it).
    current_credential: Option<String>,
    /// Current model ID from pre-fill (used to pre-select after model fetch).
    current_model: String,
}

/// Which field is being edited in the roles step.
#[derive(Debug, Clone, PartialEq)]
enum RoleEditField {
    None,
    Name(usize),  // editing role name at index (usize::MAX = new)
    Model(usize), // editing model id at index
}

impl SetupState {
    fn new() -> Self {
        Self {
            step: SetupStep::Provider,
            needs_fetch: false,
            profile_name: String::new(),
            provider_cursor: 0,
            model_cursor: 0,
            model_scroll: 0,
            model_search: String::new(),
            custom_model: false,
            provider: 0,
            credential: String::new(),
            model: String::new(),
            input: String::new(),
            fetched_models: Vec::new(),
            roles: Vec::new(),
            role_cursor: 0,
            role_edit_field: RoleEditField::None,
            role_input: String::new(),
            subagent_intro_cursor: 0,
            configure_worker: false,
            configure_reviewer: false,
            worker_provider: 0,
            worker_credential: String::new(),
            worker_model: String::new(),
            worker_fetched_models: Vec::new(),
            worker_model_cursor: 0,
            worker_model_scroll: 0,
            worker_model_search: String::new(),
            worker_custom_model: false,
            reviewer_provider: 0,
            reviewer_credential: String::new(),
            reviewer_model: String::new(),
            reviewer_fetched_models: Vec::new(),
            reviewer_model_cursor: 0,
            reviewer_model_scroll: 0,
            reviewer_model_search: String::new(),
            reviewer_custom_model: false,
            worker_needs_fetch: false,
            reviewer_needs_fetch: false,
            error: None,
            current_credential: None,
            current_model: String::new(),
        }
    }

    fn new_with_prefill(pre_fill: SetupPreFill) -> Self {
        let provider_idx = PROVIDERS
            .iter()
            .position(|(_, id, _)| *id == pre_fill.provider.as_str())
            .unwrap_or(0);
        let current_credential = if pre_fill.api_key.is_empty() {
            None
        } else {
            Some(pre_fill.api_key)
        };
        let initial_step = if pre_fill.is_new_profile {
            SetupStep::ProfileName
        } else {
            SetupStep::Provider
        };
        Self {
            step: initial_step,
            needs_fetch: false,
            profile_name: pre_fill.profile_name.clone(),
            provider_cursor: provider_idx,
            model_cursor: 0,
            model_scroll: 0,
            model_search: String::new(),
            custom_model: false,
            provider: provider_idx,
            credential: String::new(),
            model: pre_fill.model.clone(),
            input: String::new(),
            fetched_models: Vec::new(),
            roles: pre_fill.roles,
            role_cursor: 0,
            role_edit_field: RoleEditField::None,
            role_input: String::new(),
            subagent_intro_cursor: 0,
            configure_worker: false,
            configure_reviewer: false,
            worker_provider: provider_idx,
            worker_credential: String::new(),
            worker_model: String::new(),
            worker_fetched_models: Vec::new(),
            worker_model_cursor: 0,
            worker_model_scroll: 0,
            worker_model_search: String::new(),
            worker_custom_model: false,
            reviewer_provider: provider_idx,
            reviewer_credential: String::new(),
            reviewer_model: String::new(),
            reviewer_fetched_models: Vec::new(),
            reviewer_model_cursor: 0,
            reviewer_model_scroll: 0,
            reviewer_model_search: String::new(),
            reviewer_custom_model: false,
            worker_needs_fetch: false,
            reviewer_needs_fetch: false,
            error: None,
            current_credential,
            current_model: pre_fill.model,
        }
    }

    fn is_local(&self) -> bool {
        self.provider == 5
    }

    fn key_env(&self) -> &'static str {
        PROVIDER_KEY_ENV[self.provider]
    }

    fn model_list_mode(&self) -> bool {
        !self.fetched_models.is_empty() && !self.custom_model
    }

    /// Returns filtered model entries matching the current search query.
    fn filtered_models(&self) -> Vec<&ModelEntry> {
        if self.model_search.is_empty() {
            self.fetched_models.iter().collect()
        } else {
            let q = self.model_search.to_lowercase();
            self.fetched_models
                .iter()
                .filter(|m| m.id.to_lowercase().contains(&q))
                .collect()
        }
    }

    fn worker_model_list_mode(&self) -> bool {
        !self.worker_fetched_models.is_empty() && !self.worker_custom_model
    }

    fn reviewer_model_list_mode(&self) -> bool {
        !self.reviewer_fetched_models.is_empty() && !self.reviewer_custom_model
    }

    fn filtered_worker_models(&self) -> Vec<&ModelEntry> {
        if self.worker_model_search.is_empty() {
            self.worker_fetched_models.iter().collect()
        } else {
            let q = self.worker_model_search.to_lowercase();
            self.worker_fetched_models
                .iter()
                .filter(|m| m.id.to_lowercase().contains(&q))
                .collect()
        }
    }

    fn filtered_reviewer_models(&self) -> Vec<&ModelEntry> {
        if self.reviewer_model_search.is_empty() {
            self.reviewer_fetched_models.iter().collect()
        } else {
            let q = self.reviewer_model_search.to_lowercase();
            self.reviewer_fetched_models
                .iter()
                .filter(|m| m.id.to_lowercase().contains(&q))
                .collect()
        }
    }

    fn clamp_model_scroll(&mut self, visible_rows: usize) {
        let visible = visible_rows.max(1);
        if self.model_cursor < self.model_scroll {
            self.model_scroll = self.model_cursor;
        } else if self.model_cursor >= self.model_scroll + visible {
            self.model_scroll = self.model_cursor + 1 - visible;
        }
        let total = self.filtered_models().len() + 1;
        let max_scroll = total.saturating_sub(visible);
        if self.model_scroll > max_scroll {
            self.model_scroll = max_scroll;
        }
    }
}

// ── TUI rendering ─────────────────────────────────────────────────────────────

fn draw_setup(f: &mut Frame, s: &SetupState) {
    let area = f.area();
    let [hdr, body, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let step_label = match s.step {
        SetupStep::ProfileName => "new profile — enter name",
        SetupStep::Provider => "main agent — choose provider",
        SetupStep::Credential => {
            if s.is_local() {
                "main agent — set base URL"
            } else {
                "main agent — enter API key"
            }
        }
        SetupStep::FetchingModels => "main agent — fetching models…",
        SetupStep::Model => "main agent — choose model",
        SetupStep::SubAgentIntro => "sub-agents — optional",
        SetupStep::WorkerProvider => "worker agent — choose provider",
        SetupStep::WorkerCredential => "worker agent — enter API key",
        SetupStep::FetchingWorkerModels => "worker agent — fetching models…",
        SetupStep::WorkerModel => "worker agent — choose model",
        SetupStep::ReviewerProvider => "reviewer agent — choose provider",
        SetupStep::ReviewerCredential => "reviewer agent — enter API key",
        SetupStep::FetchingReviewerModels => "reviewer agent — fetching models…",
        SetupStep::ReviewerModel => "reviewer agent — choose model",
        SetupStep::Roles => "configure roles  (optional)",
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "clido",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{}  │  setup — ", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(step_label, Style::default().fg(Color::White)),
        ])),
        hdr,
    );

    match s.step {
        SetupStep::ProfileName => draw_profile_name(f, body, s),
        SetupStep::Provider => draw_provider(f, body, s),
        SetupStep::Credential => draw_credential(f, body, s),
        SetupStep::FetchingModels => draw_fetching(f, body),
        SetupStep::Model => draw_model(f, body, s),
        SetupStep::SubAgentIntro => draw_subagent_intro(f, body, s),
        SetupStep::WorkerProvider | SetupStep::ReviewerProvider => {
            draw_subagent_provider(f, body, s, s.step == SetupStep::ReviewerProvider)
        }
        SetupStep::WorkerCredential | SetupStep::ReviewerCredential => {
            draw_subagent_credential(f, body, s, s.step == SetupStep::ReviewerCredential)
        }
        SetupStep::FetchingWorkerModels | SetupStep::FetchingReviewerModels => {
            draw_fetching(f, body)
        }
        SetupStep::WorkerModel => draw_worker_model(f, body, s),
        SetupStep::ReviewerModel => draw_reviewer_model(f, body, s),
        SetupStep::Roles => draw_roles(f, body, s),
    }

    // Hint / error line
    if let Some(err) = &s.error {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  ✗  ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(err.clone(), Style::default().fg(Color::Red)),
            ])),
            hint_area,
        );
    } else {
        let hint = match s.step {
            SetupStep::Provider => "  ↑↓ navigate   Enter select   Ctrl+C cancel",
            SetupStep::Credential => "  Enter confirm   Backspace edit   Esc back   Ctrl+C cancel",
            SetupStep::FetchingModels | SetupStep::FetchingWorkerModels | SetupStep::FetchingReviewerModels => "",
            SetupStep::Model if s.model_list_mode() => {
                "  ↑↓ navigate   Enter select   type to search   Backspace erase   Esc back   Ctrl+C cancel"
            }
            SetupStep::SubAgentIntro => "  ↑↓ navigate   Enter select   Ctrl+C cancel",
            SetupStep::WorkerProvider | SetupStep::ReviewerProvider => {
                "  ↑↓ navigate   Enter select   Esc skip this sub-agent   Ctrl+C cancel"
            }
            SetupStep::WorkerCredential | SetupStep::ReviewerCredential => {
                "  Enter confirm   Backspace edit   Esc skip sub-agent   Ctrl+C cancel"
            }
            SetupStep::WorkerModel if s.worker_model_list_mode() => {
                "  ↑↓ navigate   Enter select   type to search   Backspace erase   Esc back   Ctrl+C cancel"
            }
            SetupStep::ReviewerModel if s.reviewer_model_list_mode() => {
                "  ↑↓ navigate   Enter select   type to search   Backspace erase   Esc back   Ctrl+C cancel"
            }
            SetupStep::Roles if s.role_edit_field == RoleEditField::None => {
                "  ↑↓ navigate   Enter edit/select   n new role   d delete   Tab finish   Ctrl+C cancel"
            }
            SetupStep::Roles => "  Enter confirm   Backspace edit   Esc cancel edit   Ctrl+C cancel",
            _ => "  Enter confirm   Backspace edit   Esc back   Ctrl+C cancel",
        };
        f.render_widget(
            Paragraph::new(hint).style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
            hint_area,
        );
    }
}

fn draw_profile_name(f: &mut Frame, area: Rect, s: &SetupState) {
    let block = Block::default()
        .title(" Profile Name ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [_pad, input_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
    let display = format!("  {}", s.input);
    let cursor_col = input_area.x + 2 + s.input.chars().count() as u16;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  Profile name: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                s.input.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        input_area,
    );
    let _ = display;
    f.set_cursor_position((cursor_col, input_area.y));
}

fn draw_provider(f: &mut Frame, area: Rect, s: &SetupState) {
    let mut lines = vec![Line::raw("")];
    for (i, (name, _, desc)) in PROVIDERS.iter().enumerate() {
        lines.push(if i == s.provider_cursor {
            let mut spans = vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16}", name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", desc), Style::default().fg(Color::DarkGray)),
            ];
            if s.current_credential.is_some() && i == s.provider_cursor {
                spans.push(Span::styled(
                    "  (current)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ));
            }
            Line::from(spans)
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<16}", name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", desc),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));
    let block = Block::default()
        .title(" Provider ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_credential(f: &mut Frame, area: Rect, s: &SetupState) {
    let [info_area, input_area, _] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let (pname, _, _) = PROVIDERS[s.provider];
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ]),
        info_area,
    );

    if s.is_local() {
        draw_text_input(f, input_area, " Base URL ", &s.input, false);
    } else {
        let title = format!(" {} ", s.key_env());
        let masked: String = s.input.chars().map(|_| '•').collect();
        let display = if s.input.is_empty() {
            let placeholder = if let Some(ref k) = s.current_credential {
                let masked_key: String = k
                    .chars()
                    .enumerate()
                    .map(|(i, c)| if i < 4 || i + 4 >= k.len() { c } else { '•' })
                    .collect();
                format!(" Enter to keep: {}", masked_key)
            } else {
                " paste key here".to_string()
            };
            Line::from(vec![Span::styled(
                placeholder,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        } else {
            Line::from(vec![Span::styled(
                format!(" {}", masked),
                Style::default().fg(Color::White),
            )])
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        f.render_widget(Paragraph::new(display).block(block), input_area);
    }
}

fn draw_fetching(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![Span::styled(
                "  ⏳  Fetching models from API…",
                Style::default().fg(Color::DarkGray),
            )]),
        ]),
        area,
    );
}

fn draw_model(f: &mut Frame, area: Rect, s: &SetupState) {
    let (pname, _, _) = PROVIDERS[s.provider];

    if s.model_list_mode() {
        // Layout: provider info | search box | scrollable model list
        let [info_area, search_area, list_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(area);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );

        // Search box
        let search_block = Block::default()
            .title(" Search ")
            .borders(Borders::ALL)
            .border_style(if s.model_search.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            });
        let search_content = if s.model_search.is_empty() {
            Line::from(vec![Span::styled(
                " type to filter…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        } else {
            Line::from(format!(" {}", s.model_search))
        };
        f.render_widget(
            Paragraph::new(search_content).block(search_block),
            search_area,
        );
        // Show cursor in search box
        f.set_cursor_position((
            search_area.x + 2 + s.model_search.chars().count() as u16,
            search_area.y + 1,
        ));

        // Filtered model list + "Custom…"
        // We use a sentinel `None` to represent the "Custom…" entry at the end.
        let filtered = s.filtered_models();
        let visible_rows = list_area.height.saturating_sub(2) as usize;

        // Build an index list: Some(entry) for real models, None for "Custom…"
        let display_entries: Vec<Option<&ModelEntry>> = filtered
            .iter()
            .map(|e| Some(*e))
            .chain(std::iter::once(None))
            .collect();

        let scroll = s.model_scroll;
        let visible = &display_entries[scroll.min(display_entries.len())..];
        let visible = &visible[..visible.len().min(visible_rows)];

        let mut lines = vec![Line::raw("")];
        for (rel_i, entry) in visible.iter().enumerate() {
            let abs_i = scroll + rel_i;
            let selected = abs_i == s.model_cursor;
            match entry {
                None => {
                    // "Custom…" entry
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                "Custom…",
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled("Custom…", Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                Some(entry) if entry.available => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                entry.id.clone(),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(entry.id.clone(), Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                Some(entry) => {
                    // Unavailable model — greyed out with a "no endpoints" marker
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(" ▶ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(entry.id.clone(), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                "  no endpoints",
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(
                                format!("{}  no endpoints", entry.id),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ])
                    });
                }
            }
        }
        lines.push(Line::raw(""));

        let avail_count = filtered.iter().filter(|e| e.available).count();
        let title = if !s.model_search.is_empty() {
            format!(
                " Model  ({} available / {} matched / {} total) ",
                avail_count,
                filtered.len(),
                s.fetched_models.len()
            )
        } else {
            let total_avail = s.fetched_models.iter().filter(|e| e.available).count();
            format!(
                " Model  ({} available of {}) ",
                total_avail,
                s.fetched_models.len()
            )
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(lines).block(block), list_area);
    } else {
        // Text input: fetch failed or user chose Custom…
        let [info_area, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  Provider: "),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );
        let title = if s.fetched_models.is_empty() {
            " Model ID (couldn't fetch — type manually) "
        } else {
            " Model ID "
        };
        draw_text_input(f, list_area, title, &s.input, false);
    }
}

fn draw_text_input(f: &mut Frame, area: Rect, title: &str, value: &str, _masked: bool) {
    let block = Block::default()
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let display = if value.is_empty() {
        Line::from(vec![Span::styled(
            " ",
            Style::default().fg(Color::DarkGray),
        )])
    } else {
        Line::from(format!(" {}", value))
    };
    f.render_widget(Paragraph::new(display).block(block), area);
    f.set_cursor_position((area.x + 2 + value.chars().count() as u16, area.y + 1));
}

/// Options shown on the sub-agent intro screen.
const SUBAGENT_OPTIONS: &[(&str, &str)] = &[
    (
        "Worker sub-agent",
        "cheaper model handles file filtering, summarizing, formatting",
    ),
    (
        "Worker + Reviewer",
        "worker for mechanical tasks, reviewer for quality checks",
    ),
    ("Skip for now", "can add sub-agents later via /settings"),
];

fn draw_subagent_intro(f: &mut Frame, area: Rect, s: &SetupState) {
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![Span::styled(
            "  Sub-agents route mechanical tasks to a smaller, cheaper model — reducing cost.",
            Style::default().fg(Color::Gray),
        )]),
        Line::from(vec![Span::styled(
            "  The main agent handles routing automatically; you never think about it.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]),
        Line::raw(""),
    ];

    for (i, (name, desc)) in SUBAGENT_OPTIONS.iter().enumerate() {
        let selected = i == s.subagent_intro_cursor;
        lines.push(if selected {
            Line::from(vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<24}", name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", desc), Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<24}", name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", desc),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));

    let block = Block::default()
        .title(" Sub-Agents  (optional) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_subagent_provider(f: &mut Frame, area: Rect, s: &SetupState, is_reviewer: bool) {
    let [title_area, body_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    let agent_name = if is_reviewer {
        "Reviewer agent"
    } else {
        "Worker agent"
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    agent_name,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  (optional — cheaper model for mechanical tasks)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]),
        ]),
        title_area,
    );

    let cursor = if is_reviewer {
        s.reviewer_provider
    } else {
        s.worker_provider
    };
    let mut lines = vec![Line::raw("")];
    for (i, (name, _, desc)) in PROVIDERS.iter().enumerate() {
        lines.push(if i == cursor {
            Line::from(vec![
                Span::styled(
                    " ▶ ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16}", name),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", desc), Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{:<16}", name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  {}", desc),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        });
    }
    lines.push(Line::raw(""));
    let block = Block::default()
        .title(format!(" {} — Provider ", agent_name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), body_area);
}

fn draw_subagent_credential(f: &mut Frame, area: Rect, s: &SetupState, is_reviewer: bool) {
    let [info_area, input_area, _] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let agent_name = if is_reviewer {
        "Reviewer agent"
    } else {
        "Worker agent"
    };
    let prov_idx = if is_reviewer {
        s.reviewer_provider
    } else {
        s.worker_provider
    };
    let (pname, _, _) = PROVIDERS[prov_idx];
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw(format!("  {} — Provider: ", agent_name)),
                Span::styled(
                    pname.to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ]),
        info_area,
    );

    let key_env = PROVIDER_KEY_ENV[prov_idx];
    let title = if key_env.is_empty() {
        " Base URL ".to_string()
    } else {
        format!(" {} ", key_env)
    };
    let masked: String = s.input.chars().map(|_| '•').collect();
    let display = if s.input.is_empty() {
        Line::from(vec![Span::styled(
            " paste key here",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )])
    } else {
        Line::from(vec![Span::styled(
            format!(" {}", masked),
            Style::default().fg(Color::White),
        )])
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    f.render_widget(Paragraph::new(display).block(block), input_area);
}

fn draw_worker_model(f: &mut Frame, area: Rect, s: &SetupState) {
    draw_subagent_model(f, area, s, false);
}

fn draw_reviewer_model(f: &mut Frame, area: Rect, s: &SetupState) {
    draw_subagent_model(f, area, s, true);
}

fn draw_subagent_model(f: &mut Frame, area: Rect, s: &SetupState, is_reviewer: bool) {
    let agent_name = if is_reviewer {
        "Reviewer agent"
    } else {
        "Worker agent"
    };
    let prov_idx = if is_reviewer {
        s.reviewer_provider
    } else {
        s.worker_provider
    };
    let (pname, _, _) = PROVIDERS[prov_idx];
    let list_mode = if is_reviewer {
        s.reviewer_model_list_mode()
    } else {
        s.worker_model_list_mode()
    };

    if list_mode {
        let [info_area, search_area, list_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .areas(area);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!("  {} — Provider: ", agent_name)),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );

        let search = if is_reviewer {
            &s.reviewer_model_search
        } else {
            &s.worker_model_search
        };
        let search_block = Block::default()
            .title(" Search ")
            .borders(Borders::ALL)
            .border_style(if search.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            });
        let search_content = if search.is_empty() {
            Line::from(vec![Span::styled(
                " type to filter…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )])
        } else {
            Line::from(format!(" {}", search))
        };
        f.render_widget(
            Paragraph::new(search_content).block(search_block),
            search_area,
        );
        f.set_cursor_position((
            search_area.x + 2 + search.chars().count() as u16,
            search_area.y + 1,
        ));

        let (filtered, cursor, scroll) = if is_reviewer {
            (
                s.filtered_reviewer_models(),
                s.reviewer_model_cursor,
                s.reviewer_model_scroll,
            )
        } else {
            (
                s.filtered_worker_models(),
                s.worker_model_cursor,
                s.worker_model_scroll,
            )
        };
        let visible_rows = list_area.height.saturating_sub(2) as usize;
        let display_entries: Vec<Option<&ModelEntry>> = filtered
            .iter()
            .map(|e| Some(*e))
            .chain(std::iter::once(None))
            .collect();
        let visible_slice = &display_entries[scroll.min(display_entries.len())..];
        let visible_slice = &visible_slice[..visible_slice.len().min(visible_rows)];

        let mut lines = vec![Line::raw("")];
        for (rel_i, entry) in visible_slice.iter().enumerate() {
            let abs_i = scroll + rel_i;
            let selected = abs_i == cursor;
            match entry {
                None => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                "Custom…",
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled("Custom…", Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                Some(e) if e.available => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(
                                " ▶ ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                e.id.clone(),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(e.id.clone(), Style::default().fg(Color::DarkGray)),
                        ])
                    });
                }
                Some(e) => {
                    lines.push(if selected {
                        Line::from(vec![
                            Span::styled(" ▶ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(e.id.clone(), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                "  no endpoints",
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::raw("   "),
                            Span::styled(
                                format!("{}  no endpoints", e.id),
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::DIM),
                            ),
                        ])
                    });
                }
            }
        }
        lines.push(Line::raw(""));
        let block = Block::default()
            .title(format!(" {} — Model ", agent_name))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(lines).block(block), list_area);
    } else {
        let [info_area, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!("  {} — Provider: ", agent_name)),
                Span::styled(
                    pname,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            info_area,
        );
        draw_text_input(f, list_area, " Model ID ", &s.input, false);
    }
}

// ── TUI event loop ────────────────────────────────────────────────────────────

fn setup_event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    pre_fill: Option<SetupPreFill>,
) -> Result<SetupState, anyhow::Error> {
    let mut s = match pre_fill {
        Some(pf) => SetupState::new_with_prefill(pf),
        None => SetupState::new(),
    };

    loop {
        terminal.draw(|f| draw_setup(f, &s))?;

        // After rendering "FetchingModels", do the blocking fetch.
        if s.needs_fetch {
            s.needs_fetch = false;
            let provider_id = PROVIDERS[s.provider].1;
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
            s.custom_model = s.fetched_models.is_empty();
            s.model_cursor = 0;
            s.model_scroll = 0;
            // If reinit, pre-select the current model in the list.
            if !s.current_model.is_empty() {
                if let Some(idx) = s
                    .fetched_models
                    .iter()
                    .position(|m| m.id == s.current_model)
                {
                    s.model_cursor = idx;
                    // Try to center it in the visible window (assume ~10 visible rows).
                    s.model_scroll = idx.saturating_sub(5);
                }
            }
            s.model_search.clear();
            s.input.clear();
            s.step = SetupStep::Model;
            continue;
        }

        // Fetch worker models.
        if s.worker_needs_fetch {
            s.worker_needs_fetch = false;
            let provider_id = PROVIDERS[s.worker_provider].1;
            let is_local_worker = s.worker_provider == PROVIDERS.len() - 1;
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
            s.worker_model_cursor = 0;
            s.worker_model_scroll = 0;
            s.worker_model_search.clear();
            s.input.clear();
            s.step = SetupStep::WorkerModel;
            continue;
        }

        // Fetch reviewer models.
        if s.reviewer_needs_fetch {
            s.reviewer_needs_fetch = false;
            let provider_id = PROVIDERS[s.reviewer_provider].1;
            let is_local_reviewer = s.reviewer_provider == PROVIDERS.len() - 1;
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
            s.reviewer_model_cursor = 0;
            s.reviewer_model_scroll = 0;
            s.reviewer_model_search.clear();
            s.input.clear();
            s.step = SetupStep::ReviewerModel;
            continue;
        }

        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Ctrl+C / Ctrl+Q → cancel
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('q'))
            {
                return Err(anyhow::anyhow!("Setup cancelled."));
            }

            s.error = None;

            match s.step {
                // ── Profile name input (new named profiles only) ──────────
                SetupStep::ProfileName => match key.code {
                    KeyCode::Enter => {
                        let name = s.input.trim().to_string();
                        if name.is_empty() {
                            s.error =
                                Some("Profile name required — type a name and press Enter".into());
                        } else if name.contains(' ') || name.contains('/') {
                            s.error = Some("Profile name must not contain spaces or '/'".into());
                        } else {
                            s.profile_name = name;
                            s.input.clear();
                            s.step = SetupStep::Provider;
                        }
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── Provider selection ────────────────────────────────────
                SetupStep::Provider => match key.code {
                    KeyCode::Up => {
                        if s.provider_cursor > 0 {
                            s.provider_cursor -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if s.provider_cursor < PROVIDERS.len() - 1 {
                            s.provider_cursor += 1;
                        }
                    }
                    KeyCode::Enter => {
                        s.provider = s.provider_cursor;
                        s.input.clear();
                        s.step = SetupStep::Credential;
                    }
                    _ => {}
                },

                // ── Credential input ──────────────────────────────────────
                SetupStep::Credential => match key.code {
                    KeyCode::Enter => {
                        if !s.is_local() && s.input.is_empty() {
                            if let Some(k) = s.current_credential.clone() {
                                s.credential = k;
                                s.input.clear();
                                s.step = SetupStep::FetchingModels;
                                s.needs_fetch = true;
                            } else {
                                s.error = Some(
                                    "API key required — paste your key and press Enter".into(),
                                );
                            }
                        } else {
                            s.credential = s.input.clone();
                            s.input.clear();
                            s.step = SetupStep::FetchingModels;
                            s.needs_fetch = true;
                        }
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        s.step = SetupStep::Provider;
                        s.input.clear();
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── FetchingModels (no key handling — handled above) ──────
                SetupStep::FetchingModels => {}

                // ── Model list selection ──────────────────────────────────
                SetupStep::Model if s.model_list_mode() => {
                    // Compute visible rows from terminal size for scroll clamping.
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    // Overhead: header(1) + info(2) + search_box(3) + hint(1) + list_borders(2)
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);

                    match key.code {
                        KeyCode::Up => {
                            if s.model_cursor > 0 {
                                s.model_cursor -= 1;
                                s.clamp_model_scroll(visible_rows);
                            }
                        }
                        KeyCode::Down => {
                            let filtered_len = s.filtered_models().len();
                            // +1 for "Custom…" entry
                            if s.model_cursor < filtered_len {
                                s.model_cursor += 1;
                                s.clamp_model_scroll(visible_rows);
                            }
                        }
                        KeyCode::Enter => {
                            let filtered = s.filtered_models();
                            if s.model_cursor == filtered.len() {
                                // "Custom…" selected
                                s.custom_model = true;
                                s.input.clear();
                            } else if s.model_cursor < filtered.len() {
                                let entry = filtered[s.model_cursor];
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
                        }
                        KeyCode::Backspace => {
                            s.model_search.pop();
                            s.model_cursor = 0;
                            s.model_scroll = 0;
                        }
                        KeyCode::Esc => {
                            if !s.model_search.is_empty() {
                                s.model_search.clear();
                                s.model_cursor = 0;
                                s.model_scroll = 0;
                            } else {
                                s.step = SetupStep::Credential;
                                s.input = s.credential.clone();
                            }
                        }
                        KeyCode::Char(c) => {
                            s.model_search.push(c);
                            s.model_cursor = 0;
                            s.model_scroll = 0;
                        }
                        _ => {}
                    }
                }

                // ── Model text input (custom or fetch failed) ─────────────
                SetupStep::Model => match key.code {
                    KeyCode::Enter => {
                        if s.input.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.model = s.input.clone();
                            s.step = SetupStep::SubAgentIntro;
                        }
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        if !s.fetched_models.is_empty() {
                            // Back to list
                            s.custom_model = false;
                            s.input.clear();
                        } else {
                            // Back to credential
                            s.step = SetupStep::Credential;
                            s.input = s.credential.clone();
                        }
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
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
                            s.step = SetupStep::WorkerProvider;
                        }
                        1 => {
                            // Worker + Reviewer
                            s.configure_worker = true;
                            s.configure_reviewer = true;
                            s.worker_provider = s.provider;
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
                    KeyCode::Up => {
                        if s.worker_provider > 0 {
                            s.worker_provider -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if s.worker_provider < PROVIDERS.len() - 1 {
                            s.worker_provider += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if s.worker_provider == s.provider {
                            // Same provider as main — reuse credential
                            s.worker_credential = s.credential.clone();
                            s.worker_needs_fetch = true;
                            s.step = SetupStep::FetchingWorkerModels;
                        } else {
                            s.input.clear();
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
                    _ => {}
                },

                // ── Worker credential ─────────────────────────────────────
                SetupStep::WorkerCredential => match key.code {
                    KeyCode::Enter => {
                        if s.input.is_empty() {
                            if PROVIDERS[s.worker_provider].1 == "local" {
                                s.worker_credential = "http://localhost:11434".to_string();
                            } else {
                                s.error = Some("API key required. Press Esc to skip.".into());
                                continue;
                            }
                        } else {
                            s.worker_credential = s.input.clone();
                        }
                        s.worker_needs_fetch = true;
                        s.input.clear();
                        s.step = SetupStep::FetchingWorkerModels;
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        s.configure_worker = false;
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── FetchingWorkerModels (handled in fetch block above) ────
                SetupStep::FetchingWorkerModels => {}

                // ── Worker model list ─────────────────────────────────────
                SetupStep::WorkerModel if s.worker_model_list_mode() => {
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    match key.code {
                        KeyCode::Up => {
                            if s.worker_model_cursor > 0 {
                                s.worker_model_cursor -= 1;
                            }
                        }
                        KeyCode::Down => {
                            let filtered_len = s.filtered_worker_models().len();
                            if s.worker_model_cursor < filtered_len {
                                s.worker_model_cursor += 1;
                            }
                        }
                        KeyCode::Enter => {
                            let filtered = s.filtered_worker_models();
                            if s.worker_model_cursor == filtered.len() {
                                s.worker_custom_model = true;
                                s.input.clear();
                            } else if s.worker_model_cursor < filtered.len() {
                                let entry = filtered[s.worker_model_cursor];
                                if !entry.available {
                                    s.error = Some(format!(
                                        "{} has no endpoints — pick a different model",
                                        entry.id
                                    ));
                                } else {
                                    s.worker_model = entry.id.clone();
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
                        }
                        KeyCode::Backspace => {
                            s.worker_model_search.pop();
                            s.worker_model_cursor = 0;
                            s.worker_model_scroll = 0;
                        }
                        KeyCode::Esc => {
                            if !s.worker_model_search.is_empty() {
                                s.worker_model_search.clear();
                                s.worker_model_cursor = 0;
                                s.worker_model_scroll = 0;
                            } else {
                                s.step = SetupStep::WorkerProvider;
                            }
                        }
                        KeyCode::Char(c) => {
                            s.worker_model_search.push(c);
                            s.worker_model_cursor = 0;
                            s.worker_model_scroll = 0;
                        }
                        _ => {}
                    }
                    // Clamp scroll
                    let filtered_len = s.filtered_worker_models().len();
                    let total = filtered_len + 1;
                    if s.worker_model_cursor < s.worker_model_scroll {
                        s.worker_model_scroll = s.worker_model_cursor;
                    } else if s.worker_model_cursor >= s.worker_model_scroll + visible_rows {
                        s.worker_model_scroll = s.worker_model_cursor + 1 - visible_rows;
                    }
                    let max_scroll = total.saturating_sub(visible_rows);
                    if s.worker_model_scroll > max_scroll {
                        s.worker_model_scroll = max_scroll;
                    }
                }

                // ── Worker model text input ───────────────────────────────
                SetupStep::WorkerModel => match key.code {
                    KeyCode::Enter => {
                        if s.input.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.worker_model = s.input.clone();
                            s.input.clear();
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
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        if !s.worker_fetched_models.is_empty() {
                            s.worker_custom_model = false;
                            s.input.clear();
                        } else {
                            s.step = SetupStep::WorkerProvider;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── Reviewer provider ─────────────────────────────────────
                SetupStep::ReviewerProvider => match key.code {
                    KeyCode::Up => {
                        if s.reviewer_provider > 0 {
                            s.reviewer_provider -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if s.reviewer_provider < PROVIDERS.len() - 1 {
                            s.reviewer_provider += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if s.reviewer_provider == s.provider {
                            s.reviewer_credential = s.credential.clone();
                            s.reviewer_needs_fetch = true;
                            s.step = SetupStep::FetchingReviewerModels;
                        } else {
                            s.input.clear();
                            s.step = SetupStep::ReviewerCredential;
                        }
                    }
                    KeyCode::Esc => {
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    _ => {}
                },

                // ── Reviewer credential ───────────────────────────────────
                SetupStep::ReviewerCredential => match key.code {
                    KeyCode::Enter => {
                        if s.input.is_empty() {
                            if PROVIDERS[s.reviewer_provider].1 == "local" {
                                s.reviewer_credential = "http://localhost:11434".to_string();
                            } else {
                                s.error = Some("API key required. Press Esc to skip.".into());
                                continue;
                            }
                        } else {
                            s.reviewer_credential = s.input.clone();
                        }
                        s.reviewer_needs_fetch = true;
                        s.input.clear();
                        s.step = SetupStep::FetchingReviewerModels;
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        s.configure_reviewer = false;
                        s.step = SetupStep::Roles;
                        s.role_cursor = 0;
                        s.role_edit_field = RoleEditField::None;
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── FetchingReviewerModels (handled in fetch block above) ──
                SetupStep::FetchingReviewerModels => {}

                // ── Reviewer model list ───────────────────────────────────
                SetupStep::ReviewerModel if s.reviewer_model_list_mode() => {
                    let term_h = terminal.size().map(|s| s.height).unwrap_or(24);
                    let visible_rows = (term_h as usize).saturating_sub(9).max(1);
                    match key.code {
                        KeyCode::Up => {
                            if s.reviewer_model_cursor > 0 {
                                s.reviewer_model_cursor -= 1;
                            }
                        }
                        KeyCode::Down => {
                            let filtered_len = s.filtered_reviewer_models().len();
                            if s.reviewer_model_cursor < filtered_len {
                                s.reviewer_model_cursor += 1;
                            }
                        }
                        KeyCode::Enter => {
                            let filtered = s.filtered_reviewer_models();
                            if s.reviewer_model_cursor == filtered.len() {
                                s.reviewer_custom_model = true;
                                s.input.clear();
                            } else if s.reviewer_model_cursor < filtered.len() {
                                let entry = filtered[s.reviewer_model_cursor];
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
                        }
                        KeyCode::Backspace => {
                            s.reviewer_model_search.pop();
                            s.reviewer_model_cursor = 0;
                            s.reviewer_model_scroll = 0;
                        }
                        KeyCode::Esc => {
                            if !s.reviewer_model_search.is_empty() {
                                s.reviewer_model_search.clear();
                                s.reviewer_model_cursor = 0;
                                s.reviewer_model_scroll = 0;
                            } else {
                                s.step = SetupStep::ReviewerProvider;
                            }
                        }
                        KeyCode::Char(c) => {
                            s.reviewer_model_search.push(c);
                            s.reviewer_model_cursor = 0;
                            s.reviewer_model_scroll = 0;
                        }
                        _ => {}
                    }
                    // Clamp scroll
                    let filtered_len = s.filtered_reviewer_models().len();
                    let total = filtered_len + 1;
                    if s.reviewer_model_cursor < s.reviewer_model_scroll {
                        s.reviewer_model_scroll = s.reviewer_model_cursor;
                    } else if s.reviewer_model_cursor >= s.reviewer_model_scroll + visible_rows {
                        s.reviewer_model_scroll = s.reviewer_model_cursor + 1 - visible_rows;
                    }
                    let max_scroll = total.saturating_sub(visible_rows);
                    if s.reviewer_model_scroll > max_scroll {
                        s.reviewer_model_scroll = max_scroll;
                    }
                }

                // ── Reviewer model text input ─────────────────────────────
                SetupStep::ReviewerModel => match key.code {
                    KeyCode::Enter => {
                        if s.input.is_empty() {
                            s.error = Some("Model ID required — type a model name".into());
                        } else {
                            s.reviewer_model = s.input.clone();
                            s.input.clear();
                            s.step = SetupStep::Roles;
                            s.role_cursor = 0;
                            s.role_edit_field = RoleEditField::None;
                        }
                    }
                    KeyCode::Backspace => {
                        s.input.pop();
                    }
                    KeyCode::Esc => {
                        if !s.reviewer_fetched_models.is_empty() {
                            s.reviewer_custom_model = false;
                            s.input.clear();
                        } else {
                            s.step = SetupStep::ReviewerProvider;
                        }
                    }
                    KeyCode::Char(c) => {
                        s.input.push(c);
                    }
                    _ => {}
                },

                // ── Roles configuration ───────────────────────────────────
                SetupStep::Roles => {
                    match &s.role_edit_field {
                        RoleEditField::None => match key.code {
                            // Tab or Ctrl+Enter: finish setup
                            KeyCode::Tab | KeyCode::BackTab => {
                                return Ok(s);
                            }
                            // Enter on a row: begin editing the model for that role
                            KeyCode::Enter => {
                                if s.role_cursor < s.roles.len() {
                                    let model = s.roles[s.role_cursor].1.clone();
                                    s.role_input = model;
                                    s.role_edit_field = RoleEditField::Model(s.role_cursor);
                                } else {
                                    // Cursor on "Done" row
                                    return Ok(s);
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
                                if s.custom_model {
                                    s.step = SetupStep::Model;
                                } else {
                                    s.step = SetupStep::Model;
                                    s.model_search.clear();
                                    s.model_cursor = 0;
                                    s.model_scroll = 0;
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

fn draw_roles(f: &mut Frame, area: Rect, s: &SetupState) {
    let [info_area, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    // Info bar
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::from(vec![
                Span::styled("  Model: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    s.model.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "   |   assign shortcuts like  fast → haiku  smart → opus",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ]),
        ]),
        info_area,
    );

    let mut lines = vec![Line::raw("")];

    // Existing roles
    for (i, (name, model)) in s.roles.iter().enumerate() {
        let selected = i == s.role_cursor && s.role_edit_field == RoleEditField::None;
        let editing_name = matches!(&s.role_edit_field, RoleEditField::Name(idx) if *idx == i);
        let editing_model = matches!(&s.role_edit_field, RoleEditField::Model(idx) if *idx == i);

        let name_span = if editing_name {
            Span::styled(
                format!(" {:12}", s.role_input),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" {:12}", name),
                Style::default().fg(if selected { Color::White } else { Color::Cyan }),
            )
        };

        let arrow = Span::styled(" → ", Style::default().fg(Color::DarkGray));

        let model_span = if editing_model {
            Span::styled(
                s.role_input.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else if model.is_empty() {
            Span::styled(
                "(no model — press Enter to set)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )
        } else {
            Span::styled(
                model.clone(),
                Style::default().fg(if selected {
                    Color::White
                } else {
                    Color::DarkGray
                }),
            )
        };

        let marker = if selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        };
        lines.push(Line::from(vec![marker, name_span, arrow, model_span]));
    }

    // New-role name input
    if matches!(&s.role_edit_field, RoleEditField::Name(idx) if *idx == usize::MAX) {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!(" {:12}", s.role_input),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" → ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "type name, Enter to continue",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    lines.push(Line::raw(""));

    // "Done" row
    let done_selected = s.role_cursor == s.roles.len() && s.role_edit_field == RoleEditField::None;
    lines.push(Line::from(vec![
        if done_selected {
            Span::styled(
                " ▶ ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        },
        Span::styled(
            "Done  (Tab to skip)",
            Style::default()
                .fg(if done_selected {
                    Color::Green
                } else {
                    Color::DarkGray
                })
                .add_modifier(if done_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ]));
    lines.push(Line::raw(""));

    let block = Block::default()
        .title(" Roles  (optional) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), list_area);

    // Cursor in model edit field
    if let RoleEditField::Model(idx) = &s.role_edit_field {
        if *idx < s.roles.len() {
            // Approximate cursor position: border(1) + blank(1) + row(idx+1) + 1
            let row = list_area.y + 1 + 1 + (*idx as u16 + 1);
            let col = list_area.x + 3 + 14 + 3 + s.role_input.chars().count() as u16;
            f.set_cursor_position((col, row));
        }
    }
}

fn build_full_config_toml(s: &SetupState) -> String {
    build_toml_impl(s, None)
}

/// Alias used by tests.
#[cfg(test)]
fn build_toml(s: &SetupState) -> String {
    build_full_config_toml(s)
}

/// Internal TOML builder.
/// - `profile_name = None` → full config (first-run / /init).
/// - `profile_name = Some(name)` → only the `[profile.<name>]` block (profile wizard).
fn build_toml_impl(s: &SetupState, profile_name: Option<&str>) -> String {
    let (_, provider, _) = PROVIDERS[s.provider];

    if let Some(pname) = profile_name {
        // ── Profile mode: generate only [profile.<name>] and sub-agent blocks ──
        let main_key_line = if s.is_local() {
            let base_url = if s.credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else if !s.credential.is_empty() {
            format!("# api_key is stored in plain text — keep this file private (chmod 600).\napi_key = \"{}\"\n", s.credential)
        } else {
            String::new()
        };
        let mut out = if s.is_local() {
            let base_url = if s.credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.credential.as_str()
            };
            format!(
                "[profile.{}]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n",
                pname, s.model, base_url
            )
        } else {
            format!(
                "[profile.{}]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, provider, s.model, main_key_line
            )
        };
        if s.configure_worker && !s.worker_model.is_empty() {
            let (_, worker_prov, _) = PROVIDERS[s.worker_provider];
            let is_local_worker = s.worker_provider == PROVIDERS.len() - 1;
            let worker_key_line = if is_local_worker {
                let base_url = if s.worker_credential.is_empty() {
                    "http://localhost:11434"
                } else {
                    s.worker_credential.as_str()
                };
                format!("base_url = \"{}\"\n", base_url)
            } else if !s.worker_credential.is_empty() {
                format!("api_key = \"{}\"\n", s.worker_credential)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n[profile.{}.worker]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, worker_prov, s.worker_model, worker_key_line
            ));
        }
        if s.configure_reviewer && !s.reviewer_model.is_empty() {
            let (_, reviewer_prov, _) = PROVIDERS[s.reviewer_provider];
            let is_local_reviewer = s.reviewer_provider == PROVIDERS.len() - 1;
            let reviewer_key_line = if is_local_reviewer {
                let base_url = if s.reviewer_credential.is_empty() {
                    "http://localhost:11434"
                } else {
                    s.reviewer_credential.as_str()
                };
                format!("base_url = \"{}\"\n", base_url)
            } else if !s.reviewer_credential.is_empty() {
                format!("api_key = \"{}\"\n", s.reviewer_credential)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n[profile.{}.reviewer]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
                pname, reviewer_prov, s.reviewer_model, reviewer_key_line
            ));
        }
        return out;
    }

    // ── Full config mode (first-run / /init) ──────────────────────────────────
    let roles_toml = if s.roles.is_empty() {
        String::new()
    } else {
        let mut t = "\n[roles]\n".to_string();
        for (name, model) in &s.roles {
            t.push_str(&format!("{} = \"{}\"\n", name, model));
        }
        t
    };

    let profile_toml = if s.is_local() {
        let base_url = if s.credential.is_empty() {
            "http://localhost:11434"
        } else {
            s.credential.as_str()
        };
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n{}",
            s.model, base_url, roles_toml
        )
    } else {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"{}\"\nmodel = \"{}\"\n# api_key is stored in plain text — keep this file private (chmod 600).\napi_key = \"{}\"\n{}",
            provider, s.model, s.credential, roles_toml
        )
    };

    // Build [agents] section
    let main_key_line = if s.is_local() {
        let base_url = if s.credential.is_empty() {
            "http://localhost:11434"
        } else {
            s.credential.as_str()
        };
        format!("base_url = \"{}\"\n", base_url)
    } else if !s.credential.is_empty() {
        format!("api_key = \"{}\"\n", s.credential)
    } else {
        String::new()
    };

    let mut agents_toml = format!(
        "\n[agents.main]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
        provider, s.model, main_key_line
    );

    if s.configure_worker && !s.worker_model.is_empty() {
        let (_, worker_prov, _) = PROVIDERS[s.worker_provider];
        let is_local_worker = s.worker_provider == PROVIDERS.len() - 1;
        let worker_key_line = if is_local_worker {
            let base_url = if s.worker_credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.worker_credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else if !s.worker_credential.is_empty() {
            format!("api_key = \"{}\"\n", s.worker_credential)
        } else {
            String::new()
        };
        agents_toml.push_str(&format!(
            "\n[agents.worker]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
            worker_prov, s.worker_model, worker_key_line
        ));
    }

    if s.configure_reviewer && !s.reviewer_model.is_empty() {
        let (_, reviewer_prov, _) = PROVIDERS[s.reviewer_provider];
        let is_local_reviewer = s.reviewer_provider == PROVIDERS.len() - 1;
        let reviewer_key_line = if is_local_reviewer {
            let base_url = if s.reviewer_credential.is_empty() {
                "http://localhost:11434"
            } else {
                s.reviewer_credential.as_str()
            };
            format!("base_url = \"{}\"\n", base_url)
        } else if !s.reviewer_credential.is_empty() {
            format!("api_key = \"{}\"\n", s.reviewer_credential)
        } else {
            String::new()
        };
        agents_toml.push_str(&format!(
            "\n[agents.reviewer]\nprovider = \"{}\"\nmodel = \"{}\"\n{}",
            reviewer_prov, s.reviewer_model, reviewer_key_line
        ));
    }

    format!("{}{}", profile_toml, agents_toml)
}

// ── TUI entry point ───────────────────────────────────────────────────────────

fn run_tui_setup_blocking(
    pre_fill: Option<SetupPreFill>,
) -> Result<(PathBuf, String), anyhow::Error> {
    let config_path = if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = setup_event_loop(&mut terminal, pre_fill);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result.map(|state| (config_path, build_full_config_toml(&state)))
}

async fn run_tui_setup(pre_fill: Option<SetupPreFill>) -> Result<(PathBuf, String), anyhow::Error> {
    tokio::task::spawn_blocking(move || run_tui_setup_blocking(pre_fill))
        .await
        .map_err(|e| anyhow::anyhow!("setup join: {}", e))?
}

// ── Plain-text fallback (non-TTY) ─────────────────────────────────────────────

/// Non-TTY setup: plain stdin/stdout prompts (CI, pipes).
pub fn run_interactive_setup_blocking(
    _init_subline: Option<&str>,
) -> Result<(PathBuf, String), anyhow::Error> {
    let config_path = if let Ok(p) = env::var("CLIDO_CONFIG") {
        PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let mut line = String::new();
    let mut stdin = io::stdin().lock();
    eprintln!("{}", SETUP_BANNER_ASCII);

    // Step 1: Provider
    eprintln!("  Provider:");
    for (i, (name, _, desc)) in PROVIDERS.iter().enumerate() {
        eprintln!("    {}) {:<16}  {}", i + 1, name, desc);
    }
    eprintln!("  Enter 1–{}: ", PROVIDERS.len());
    line.clear();
    stdin
        .read_line(&mut line)
        .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
    let pidx: usize = line.trim().parse::<usize>().unwrap_or(0);
    if pidx < 1 || pidx > PROVIDERS.len() {
        return Err(anyhow::anyhow!(
            "Invalid choice. Run 'clido init' again and enter 1–{}.",
            PROVIDERS.len()
        ));
    }
    let pidx = pidx - 1;
    let (_, provider, _) = PROVIDERS[pidx];
    let is_local = pidx == PROVIDERS.len() - 1;

    // Step 2: Credential (API key or base URL)
    let credential = if is_local {
        eprintln!("  Base URL (Enter for http://localhost:11434):");
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let url = line.trim();
        if url.is_empty() {
            "http://localhost:11434".to_string()
        } else {
            url.to_string()
        }
    } else {
        let key_env = PROVIDER_KEY_ENV[pidx];
        eprintln!("  {} (paste and press Enter):", key_env);
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let api_key = line.trim();
        if api_key.is_empty() {
            return Err(anyhow::anyhow!(
                "No API key entered. Run 'clido init' again and paste your key."
            ));
        }
        api_key.to_string()
    };

    // Step 3: Fetch models from API, then ask user to pick
    let (api_key_for_fetch, base_url_for_fetch): (&str, Option<&str>) = if is_local {
        ("", Some(credential.as_str()))
    } else {
        (credential.as_str(), None)
    };
    let handle = tokio::runtime::Handle::current();
    let fetched = handle.block_on(clido_providers::fetch_provider_models(
        provider,
        api_key_for_fetch,
        base_url_for_fetch,
    ));

    let model = if fetched.is_empty() {
        eprintln!("  (Couldn't fetch model list — enter model ID manually)");
        eprintln!("  Model ID:");
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let m = line.trim();
        if m.is_empty() {
            return Err(anyhow::anyhow!(
                "No model entered. Run 'clido init' again and type a model ID."
            ));
        }
        m.to_string()
    } else {
        eprintln!("  Model:");
        for (i, m) in fetched.iter().enumerate() {
            let avail_note = if !m.available { "  [no endpoints]" } else { "" };
            eprintln!("    {}) {}{}", i + 1, m.id, avail_note);
        }
        eprintln!("  Enter 1–{} (or type a custom ID): ", fetched.len());
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|e| anyhow::anyhow!("stdin: {}", e))?;
        let choice = line.trim();
        if let Ok(midx) = choice.parse::<usize>() {
            if midx >= 1 && midx <= fetched.len() {
                fetched[midx - 1].id.clone()
            } else {
                return Err(anyhow::anyhow!("Invalid choice. Run 'clido init' again."));
            }
        } else if !choice.is_empty() {
            choice.to_string()
        } else {
            return Err(anyhow::anyhow!(
                "No model entered. Run 'clido init' again and type a model ID."
            ));
        }
    };

    let toml = if is_local {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"local\"\nmodel = \"{}\"\nbase_url = \"{}\"\n",
            model, credential
        )
    } else {
        format!(
            "default_profile = \"default\"\n\n[profile.default]\nprovider = \"{}\"\nmodel = \"{}\"\n# api_key is stored in plain text — keep this file private (chmod 600).\napi_key = \"{}\"\n",
            provider, model, credential
        )
    };

    Ok((config_path, toml))
}

// ── Anonymize helper ──────────────────────────────────────────────────────────

/// Show first 4 + `···` + last 4 chars of a key.
pub fn anonymize_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "···".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}···{}", head, tail)
}

// ── Public entry points ───────────────────────────────────────────────────────

/// First-run: no config and TTY → run TUI setup, write config, continue.
pub async fn run_first_run_setup() -> Result<(), anyhow::Error> {
    write_setup_config(false, None).await
}

/// `clido init` subcommand.
pub async fn run_init() -> Result<(), anyhow::Error> {
    write_setup_config(true, None).await
}

/// Re-run setup from within the TUI (/init command), pre-filling with current config values.
pub async fn run_reinit(pre_fill: SetupPreFill) -> Result<(), anyhow::Error> {
    write_setup_config(true, Some(pre_fill)).await
}

/// Create a new named profile via the guided wizard.
pub async fn run_create_profile(initial_name: Option<String>) -> Result<(), anyhow::Error> {
    let pre_fill = SetupPreFill {
        provider: String::new(),
        api_key: String::new(),
        model: String::new(),
        roles: Vec::new(),
        profile_name: initial_name.clone().unwrap_or_default(),
        is_new_profile: initial_name.is_none(), // show ProfileName step if no name given
    };

    let config_path = if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        std::path::PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let state = if setup_use_rich_ui() {
        tokio::task::spawn_blocking(move || run_tui_setup_state_blocking(Some(pre_fill)))
            .await
            .map_err(|e| anyhow::anyhow!("setup join: {}", e))??
    } else {
        return Err(anyhow::anyhow!(
            "Profile creation requires an interactive terminal. Run in a TTY."
        ));
    };

    // Determine profile name from state (either from ProfileName step or from initial_name)
    let pname = if state.profile_name.is_empty() {
        return Err(anyhow::anyhow!("No profile name provided."));
    } else {
        state.profile_name.clone()
    };

    let entry = state_to_profile_entry(&state);

    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;

    clido_core::upsert_profile_in_config(&config_path, &pname, &entry)
        .map_err(|e| CliError::Config(e.to_string()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }

    let use_color = setup_use_color();
    if use_color {
        println!(
            "\x1b[32m  Profile '{}' created. Run 'clido profile switch {}' to activate.\x1b[0m",
            pname, pname
        );
    } else {
        println!(
            "  Profile '{}' created. Run 'clido profile switch {}' to activate.",
            pname, pname
        );
    }
    Ok(())
}

/// Edit an existing profile via the guided wizard (pre-filled with current values).
pub async fn run_edit_profile(
    name: String,
    entry: clido_core::ProfileEntry,
) -> Result<(), anyhow::Error> {
    let api_key = entry
        .api_key
        .clone()
        .or_else(|| {
            entry
                .api_key_env
                .as_ref()
                .and_then(|e| std::env::var(e).ok())
        })
        .unwrap_or_default();

    let pre_fill = SetupPreFill {
        provider: entry.provider.clone(),
        api_key,
        model: entry.model.clone(),
        roles: Vec::new(),
        profile_name: name.clone(),
        is_new_profile: false,
    };

    let config_path = if let Ok(p) = std::env::var("CLIDO_CONFIG") {
        std::path::PathBuf::from(p)
    } else {
        let dir = directories::ProjectDirs::from("", "", "clido")
            .ok_or_else(|| CliError::Usage("Could not determine config directory.".into()))?;
        dir.config_dir().join("config.toml")
    };

    let state = if setup_use_rich_ui() {
        tokio::task::spawn_blocking(move || run_tui_setup_state_blocking(Some(pre_fill)))
            .await
            .map_err(|e| anyhow::anyhow!("setup join: {}", e))??
    } else {
        return Err(anyhow::anyhow!(
            "Profile editing requires an interactive terminal. Run in a TTY."
        ));
    };

    let updated_entry = state_to_profile_entry(&state);

    clido_core::upsert_profile_in_config(&config_path, &name, &updated_entry)
        .map_err(|e| CliError::Config(e.to_string()))?;

    let use_color = setup_use_color();
    if use_color {
        println!("\x1b[32m  Profile '{}' updated.\x1b[0m", name);
    } else {
        println!("  Profile '{}' updated.", name);
    }
    Ok(())
}

/// Build a `ProfileEntry` from the wizard state.
fn state_to_profile_entry(s: &SetupState) -> clido_core::ProfileEntry {
    let (_, provider, _) = PROVIDERS[s.provider];
    let (api_key, base_url) = if s.is_local() {
        let url = if s.credential.is_empty() {
            Some("http://localhost:11434".to_string())
        } else {
            Some(s.credential.clone())
        };
        (None, url)
    } else {
        (Some(s.credential.clone()).filter(|k| !k.is_empty()), None)
    };

    let worker = if s.configure_worker && !s.worker_model.is_empty() {
        let (_, worker_prov, _) = PROVIDERS[s.worker_provider];
        let is_local_worker = s.worker_provider == PROVIDERS.len() - 1;
        let (w_key, w_url) = if is_local_worker {
            let url = if s.worker_credential.is_empty() {
                Some("http://localhost:11434".to_string())
            } else {
                Some(s.worker_credential.clone())
            };
            (None, url)
        } else {
            (
                Some(s.worker_credential.clone()).filter(|k| !k.is_empty()),
                None,
            )
        };
        Some(clido_core::AgentSlotConfig {
            provider: worker_prov.to_string(),
            model: s.worker_model.clone(),
            api_key: w_key,
            api_key_env: None,
            base_url: w_url,
        })
    } else {
        None
    };

    let reviewer = if s.configure_reviewer && !s.reviewer_model.is_empty() {
        let (_, reviewer_prov, _) = PROVIDERS[s.reviewer_provider];
        let is_local_reviewer = s.reviewer_provider == PROVIDERS.len() - 1;
        let (r_key, r_url) = if is_local_reviewer {
            let url = if s.reviewer_credential.is_empty() {
                Some("http://localhost:11434".to_string())
            } else {
                Some(s.reviewer_credential.clone())
            };
            (None, url)
        } else {
            (
                Some(s.reviewer_credential.clone()).filter(|k| !k.is_empty()),
                None,
            )
        };
        Some(clido_core::AgentSlotConfig {
            provider: reviewer_prov.to_string(),
            model: s.reviewer_model.clone(),
            api_key: r_key,
            api_key_env: None,
            base_url: r_url,
        })
    } else {
        None
    };

    clido_core::ProfileEntry {
        provider: provider.to_string(),
        model: s.model.clone(),
        api_key,
        api_key_env: None,
        base_url,
        worker,
        reviewer,
    }
}

/// Run the TUI setup and return the final `SetupState` (for profile create/edit callers).
fn run_tui_setup_state_blocking(
    pre_fill: Option<SetupPreFill>,
) -> Result<SetupState, anyhow::Error> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = setup_event_loop(&mut terminal, pre_fill);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

async fn write_setup_config(
    use_stdout: bool,
    pre_fill: Option<SetupPreFill>,
) -> Result<(), anyhow::Error> {
    let (config_path, toml) = if setup_use_rich_ui() {
        run_tui_setup(pre_fill).await?
    } else {
        tokio::task::spawn_blocking(|| run_interactive_setup_blocking(None))
            .await
            .map_err(|e| anyhow::anyhow!("setup: {}", e))??
    };

    let parent = config_path
        .parent()
        .ok_or_else(|| CliError::Usage("Invalid config path.".into()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&config_path, toml.trim_start())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&config_path, perms);
    }

    let msg = format!(
        "  Created {}. Run 'clido doctor' to verify.",
        config_path.display()
    );
    let use_color = setup_use_color();
    if use_stdout {
        if use_color {
            println!("\x1b[32m{}\x1b[0m", msg);
        } else {
            println!("{}", msg);
        }
    } else if use_color {
        eprintln!("\x1b[32m{}\x1b[0m", msg);
    } else {
        eprintln!("{}", msg);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymize_key_short_returns_dots() {
        assert_eq!(anonymize_key("short"), "···");
        assert_eq!(anonymize_key("12345678"), "···");
    }

    #[test]
    fn anonymize_key_long_shows_head_and_tail() {
        let key = "sk-ant-api03-longkeysomethinghere12345";
        let anon = anonymize_key(key);
        assert!(anon.starts_with("sk-a"));
        assert!(anon.ends_with("2345"));
        assert!(anon.contains("···"));
    }

    #[test]
    fn anonymize_key_exactly_nine_chars() {
        let key = "123456789";
        let anon = anonymize_key(key);
        assert!(anon.starts_with("1234"));
        assert!(anon.ends_with("6789"));
    }

    #[test]
    fn build_toml_local_provider() {
        let mut s = SetupState::new();
        s.provider = 5; // Local / Ollama
        s.model = "llama3.2".to_string();
        s.credential.clear();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"local\""));
        assert!(toml.contains("model = \"llama3.2\""));
        assert!(toml.contains("base_url = \"http://localhost:11434\""));
        assert!(!toml.contains("api_key"));
    }

    #[test]
    fn build_toml_local_provider_custom_url() {
        let mut s = SetupState::new();
        s.provider = 5;
        s.model = "mistral".to_string();
        s.credential = "http://127.0.0.1:8080".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("base_url = \"http://127.0.0.1:8080\""));
    }

    #[test]
    fn build_toml_anthropic_provider() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-api03-secret".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"anthropic\""));
        assert!(toml.contains("model = \"claude-sonnet-4-5\""));
        assert!(toml.contains("api_key = \"sk-ant-api03-secret\""));
    }

    #[test]
    fn build_toml_openrouter_provider() {
        let mut s = SetupState::new();
        s.provider = 0; // OpenRouter
        s.model = "anthropic/claude-3-5-sonnet".to_string();
        s.credential = "sk-or-test-key".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"openrouter\""));
        assert!(toml.contains("api_key = \"sk-or-test-key\""));
    }

    #[test]
    fn build_toml_openai_provider() {
        let mut s = SetupState::new();
        s.provider = 2; // OpenAI
        s.model = "gpt-4o".to_string();
        s.credential = "sk-openai-test".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"openai\""));
        assert!(toml.contains("model = \"gpt-4o\""));
        assert!(toml.contains("api_key = \"sk-openai-test\""));
    }

    #[test]
    fn build_toml_mistral_provider() {
        let mut s = SetupState::new();
        s.provider = 3; // Mistral
        s.model = "mistral-large-latest".to_string();
        s.credential = "mk-test-key".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("provider = \"mistral\""));
        assert!(toml.contains("model = \"mistral-large-latest\""));
    }

    #[test]
    fn setup_state_new_defaults() {
        let s = SetupState::new();
        assert_eq!(s.step, SetupStep::Provider);
        assert_eq!(s.provider_cursor, 0);
        assert_eq!(s.model_cursor, 0);
        assert!(!s.custom_model);
        assert!(s.model.is_empty());
        assert!(s.input.is_empty());
        assert!(s.error.is_none());
        assert!(s.fetched_models.is_empty());
    }

    #[test]
    fn setup_state_is_local() {
        let mut s = SetupState::new();
        s.provider = 5; // Local / Ollama is index 5
        assert!(s.is_local());
        s.provider = 0;
        assert!(!s.is_local());
    }

    #[test]
    fn setup_state_model_list_mode() {
        let mut s = SetupState::new();
        assert!(!s.model_list_mode()); // no fetched models
        s.fetched_models = vec![ModelEntry::available("gpt-4o")];
        assert!(s.model_list_mode()); // has models, not in custom mode
        s.custom_model = true;
        assert!(!s.model_list_mode()); // custom mode overrides
    }

    #[test]
    fn providers_array_consistency() {
        assert_eq!(PROVIDERS.len(), PROVIDER_KEY_ENV.len());
        // Last provider is local (no key needed)
        assert_eq!(PROVIDER_KEY_ENV[PROVIDERS.len() - 1], "");
        // All named providers have non-empty IDs
        for (_, id, _) in &PROVIDERS {
            assert!(!id.is_empty());
        }
    }

    // ── build_toml agents section tests ────────────────────────────────────

    #[test]
    fn build_toml_agents_main_uses_api_key_not_env() {
        // Regression: [agents.main] must store api_key = "..." not api_key_env = "..."
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-secret-key".to_string();
        let toml = build_toml(&s);
        assert!(
            toml.contains("[agents.main]"),
            "agents.main section missing"
        );
        assert!(
            toml.contains("api_key = \"sk-ant-secret-key\""),
            "agents.main should store api_key directly, got:\n{}",
            toml
        );
        assert!(
            !toml.contains("api_key_env"),
            "agents.main must not use api_key_env when credential is known"
        );
    }

    #[test]
    fn build_toml_agents_with_worker_configured() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic (main)
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-main-key".to_string();
        s.configure_worker = true;
        s.worker_provider = 2; // OpenAI
        s.worker_model = "gpt-4o-mini".to_string();
        s.worker_credential = "sk-openai-worker-key".to_string();
        let toml = build_toml(&s);
        assert!(
            toml.contains("[agents.worker]"),
            "agents.worker section missing"
        );
        assert!(
            toml.contains("provider = \"openai\""),
            "worker provider missing"
        );
        assert!(
            toml.contains("model = \"gpt-4o-mini\""),
            "worker model missing"
        );
        assert!(
            toml.contains("api_key = \"sk-openai-worker-key\""),
            "worker should store api_key directly"
        );
        assert!(
            !toml.contains("api_key_env"),
            "worker must not use api_key_env"
        );
    }

    #[test]
    fn build_toml_agents_with_reviewer_configured() {
        let mut s = SetupState::new();
        s.provider = 0; // OpenRouter (main)
        s.model = "anthropic/claude-3-5-sonnet".to_string();
        s.credential = "sk-or-main-key".to_string();
        s.configure_reviewer = true;
        s.reviewer_provider = 1; // Anthropic
        s.reviewer_model = "claude-opus-4-6".to_string();
        s.reviewer_credential = "sk-ant-reviewer-key".to_string();
        let toml = build_toml(&s);
        assert!(
            toml.contains("[agents.reviewer]"),
            "agents.reviewer section missing"
        );
        assert!(
            toml.contains("provider = \"anthropic\""),
            "reviewer provider missing"
        );
        assert!(
            toml.contains("model = \"claude-opus-4-6\""),
            "reviewer model missing"
        );
        assert!(
            toml.contains("api_key = \"sk-ant-reviewer-key\""),
            "reviewer should store api_key directly"
        );
        assert!(
            !toml.contains("api_key_env"),
            "reviewer must not use api_key_env"
        );
    }

    #[test]
    fn build_toml_agents_local_worker_uses_base_url() {
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic (main)
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-key".to_string();
        s.configure_worker = true;
        s.worker_provider = 5; // Local
        s.worker_model = "llama3.2".to_string();
        s.worker_credential = "http://127.0.0.1:8080".to_string();
        let toml = build_toml(&s);
        assert!(toml.contains("[agents.worker]"));
        assert!(toml.contains("base_url = \"http://127.0.0.1:8080\""));
        // The [agents.worker] section should not have api_key or api_key_env
        let worker_section = &toml[toml.find("[agents.worker]").unwrap()..];
        assert!(
            !worker_section.contains("api_key"),
            "local worker should not have api_key"
        );
        assert!(
            !worker_section.contains("api_key_env"),
            "local worker should not have api_key_env"
        );
    }

    #[test]
    fn build_toml_agents_main_no_credential_no_key_line() {
        // When credential is empty, [agents.main] should not have an api_key line.
        let mut s = SetupState::new();
        s.provider = 1; // Anthropic
        s.model = "claude-sonnet-4-5".to_string();
        s.credential.clear();
        let toml = build_toml(&s);
        // [agents.main] should exist but without api_key line
        assert!(toml.contains("[agents.main]"));
        let agents_section = &toml[toml.find("[agents.main]").unwrap()..];
        assert!(
            !agents_section.contains("api_key ="),
            "agents.main should not have api_key when credential is empty"
        );
    }

    #[test]
    fn build_toml_no_worker_no_reviewer_by_default() {
        let mut s = SetupState::new();
        s.provider = 1;
        s.model = "claude-sonnet-4-5".to_string();
        s.credential = "sk-ant-key".to_string();
        let toml = build_toml(&s);
        assert!(
            !toml.contains("[agents.worker]"),
            "worker section should not appear when not configured"
        );
        assert!(
            !toml.contains("[agents.reviewer]"),
            "reviewer section should not appear when not configured"
        );
    }
}
