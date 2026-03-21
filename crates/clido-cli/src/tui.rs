//! Full-screen ratatui TUI: scrollable conversation + persistent input bar.

use std::collections::HashSet;
use std::env;
use std::io::{stdout, Write as _};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clido_agent::{AgentLoop, AskUser, EventEmitter};
use clido_core::ClidoError;
use clido_planner;
use clido_storage::SessionWriter;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use pulldown_cmark::Parser;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::sync::{mpsc, oneshot};

use crate::agent_setup::AgentSetup;
use crate::cli::Cli;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/clear", "clear the conversation"),
    ("/help", "show key bindings and all slash commands"),
    ("/model", "show current model and provider"),
    ("/sessions", "list recent sessions for this project"),
    ("/session", "show current session ID"),
    ("/workdir", "show working directory"),
    ("/cost", "show session cost so far"),
    ("/tokens", "show token usage so far"),
    ("/memory", "search long-term memory (usage: /memory <query>)"),
    ("/plan", "show current task plan (requires --planner)"),
    ("/index", "show repo index stats"),
    ("/quit", "exit clido"),
];

// ── Permission grant options ───────────────────────────────────────────────────

#[derive(Debug)]
enum PermGrant {
    /// Allow this single invocation.
    Once,
    /// Allow this tool for the rest of the session.
    Session,
    /// Allow all tools for the rest of the session (workdir-wide).
    Workdir,
    /// Deny.
    Deny,
}

// ── Session-level permission state (shared between TuiAskUser calls) ──────────

#[derive(Default)]
struct PermsState {
    /// Tool names granted for the whole session.
    session_allowed: HashSet<String>,
    /// All tools open for this session (workdir-wide grant).
    workdir_open: bool,
}

// ── Agent → TUI events ────────────────────────────────────────────────────────

enum AgentEvent {
    ToolStart { name: String, detail: String },
    ToolDone { name: String, is_error: bool, diff: Option<String> },
    /// Intermediate text the model emits while it's still calling tools.
    Thinking(String),
    Response(String),
    Interrupted,
    Err(String),
    /// Emitted once when the agent session is created.
    SessionStarted(String),
    /// Emitted when a session is resumed; carries display messages.
    ResumedSession { messages: Vec<(String, String)> },
    /// Token usage update after agent turn completion.
    TokenUsage { input_tokens: u64, output_tokens: u64, cost_usd: f64 },
    /// Emitted when the planner produces a valid task graph (--planner mode).
    /// Each string is a human-readable description of one planned task.
    PlanCreated { tasks: Vec<String> },
}

// ── Session picker popup state ────────────────────────────────────────────────

struct SessionPickerState {
    sessions: Vec<clido_storage::SessionSummary>,
    selected: usize,
    scroll_offset: usize,
}

// ── Permission request (agent → TUI, reply via oneshot) ───────────────────────

struct PermRequest {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<PermGrant>,
}

// ── TuiEmitter ────────────────────────────────────────────────────────────────

struct TuiEmitter {
    tx: mpsc::UnboundedSender<AgentEvent>,
}

#[async_trait]
impl EventEmitter for TuiEmitter {
    async fn on_tool_start(&self, name: &str, input: &serde_json::Value) {
        let detail = format_tool_input(name, input);
        let _ = self.tx.send(AgentEvent::ToolStart {
            name: name.to_string(),
            detail,
        });
    }
    async fn on_tool_done(&self, name: &str, is_error: bool, diff: Option<String>) {
        let _ = self.tx.send(AgentEvent::ToolDone {
            name: name.to_string(),
            is_error,
            diff,
        });
    }
    async fn on_assistant_text(&self, text: &str) {
        if !text.trim().is_empty() {
            let _ = self.tx.send(AgentEvent::Thinking(text.to_string()));
        }
    }
}

fn format_tool_input(name: &str, input: &serde_json::Value) -> String {
    let s = match name {
        "Read" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Write" | "Edit" => input["file_path"].as_str().unwrap_or("").to_string(),
        "Bash" => input["command"]
            .as_str()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
        "Glob" => input["pattern"].as_str().unwrap_or("").to_string(),
        "Grep" => format!(
            "{}{}",
            input["pattern"].as_str().unwrap_or(""),
            input["path"]
                .as_str()
                .map(|p| format!("  {}", p))
                .unwrap_or_default()
        ),
        _ => input.to_string(),
    };
    if s.len() > 72 {
        format!("{}…", &s[..72])
    } else {
        s
    }
}

// ── TuiAskUser ────────────────────────────────────────────────────────────────

struct TuiAskUser {
    perm_tx: mpsc::UnboundedSender<PermRequest>,
    perms: Arc<Mutex<PermsState>>,
}

#[async_trait]
impl AskUser for TuiAskUser {
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        // Fast-path: check session/workdir grants before going to the TUI.
        {
            let state = self.perms.lock().unwrap();
            if state.workdir_open || state.session_allowed.contains(tool_name) {
                return true;
            }
        }

        let raw = serde_json::to_string(input).unwrap_or_default();
        let preview = if raw.len() > 120 {
            format!("{}…", &raw[..120])
        } else {
            raw
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .perm_tx
            .send(PermRequest {
                tool_name: tool_name.to_string(),
                preview,
                reply: reply_tx,
            })
            .is_err()
        {
            return false;
        }
        match reply_rx.await.unwrap_or(PermGrant::Deny) {
            PermGrant::Once => true,
            PermGrant::Session => {
                self.perms
                    .lock()
                    .unwrap()
                    .session_allowed
                    .insert(tool_name.to_string());
                true
            }
            PermGrant::Workdir => {
                self.perms.lock().unwrap().workdir_open = true;
                true
            }
            PermGrant::Deny => false,
        }
    }
}

// ── Status strip ─────────────────────────────────────────────────────────────

struct StatusEntry {
    name: String,
    detail: String,
    done: bool,
    is_error: bool,
    start: std::time::Instant,
    elapsed_ms: Option<u64>,
}

// ── Chat lines ────────────────────────────────────────────────────────────────

enum ChatLine {
    User(String),
    Assistant(String),
    /// Intermediate text emitted by the model while still calling tools (dim, no label).
    Thinking(String),
    ToolCall {
        name: String,
        detail: String,
        done: bool,
        is_error: bool,
    },
    Diff(String),

    Info(String),
}

// ── App state ─────────────────────────────────────────────────────────────────

struct PendingPerm {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<PermGrant>,
}

struct App {
    messages: Vec<ChatLine>,
    /// Live activity log shown in the status strip (last 2 entries).
    status_log: std::collections::VecDeque<StatusEntry>,
    input: String,
    cursor: usize,
    /// Current scroll offset (logical lines from top). Updated by handle_key; clamped in render.
    scroll: u16,
    /// Max scroll as computed during the last render — used by handle_key to scroll up correctly.
    max_scroll: u16,
    following: bool,
    busy: bool,
    spinner_tick: usize,
    pending_perm: Option<PendingPerm>,
    /// Error modal: shown as overlay, dismissed with Enter/Esc/Space.
    pending_error: Option<String>,
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Channel to request session resume in agent_task.
    resume_tx: mpsc::UnboundedSender<String>,
    /// Queued input typed while agent was busy — sent automatically when agent finishes.
    queued: Option<String>,
    /// Session picker popup state (Some = popup visible).
    session_picker: Option<SessionPickerState>,
    /// Signal to cancel the current agent run (force send).
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Selected option in the permission popup (0=once, 1=session, 2=workdir, 3=deny).
    perm_selected: usize,
    /// Selected index in the slash-command popup (None = no popup).
    selected_cmd: Option<usize>,
    quit: bool,
    provider: String,
    model: String,
    /// Session ID of the current agent session (set after SessionStarted event).
    current_session_id: Option<String>,
    /// Project root used for listing sessions.
    workspace_root: std::path::PathBuf,
    /// Previously submitted prompts (oldest first), for Up/Down history navigation.
    input_history: Vec<String>,
    /// Index into input_history while navigating (None = current draft).
    history_idx: Option<usize>,
    /// Saved draft while navigating history, so Down restores it.
    history_draft: String,
    /// Horizontal scroll offset for the input field (in chars), so long inputs stay visible.
    input_scroll: usize,
    /// Cumulative tokens for current session (updated after each turn).
    session_input_tokens: u64,
    session_output_tokens: u64,
    session_cost_usd: f64,
    /// Last plan produced by the planner (--planner mode), stored for /plan command.
    last_plan: Option<Vec<String>>,
}

impl App {
    fn new(
        prompt_tx: mpsc::UnboundedSender<String>,
        resume_tx: mpsc::UnboundedSender<String>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        provider: String,
        model: String,
        workspace_root: std::path::PathBuf,
    ) -> Self {
        let mut app = Self {
            messages: Vec::new(),
            status_log: std::collections::VecDeque::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            max_scroll: 0,
            following: true,
            busy: false,
            spinner_tick: 0,
            pending_perm: None,
            pending_error: None,
            prompt_tx,
            resume_tx,
            queued: None,
            session_picker: None,
            cancel,
            perm_selected: 0,
            selected_cmd: None,
            quit: false,
            provider,
            model,
            current_session_id: None,
            workspace_root,
            input_history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            input_scroll: 0,
            session_input_tokens: 0,
            session_output_tokens: 0,
            session_cost_usd: 0.0,
            last_plan: None,
        };
        for line in crate::ui::BANNER.lines() {
            app.messages.push(ChatLine::Info(line.to_string()));
        }
        app.messages.push(ChatLine::Info(String::new()));
        app.messages.push(ChatLine::Info(
            "Type your message and press Enter. Use /help for commands, /sessions to resume a session.".into(),
        ));
        app
    }

    fn push(&mut self, line: ChatLine) {
        self.messages.push(line);
        // scroll position is computed at render time when following=true
    }

    /// Send immediately (not busy). Moves input → chat + agent.
    fn send_now(&mut self, text: String) {
        self.push(ChatLine::User(text.clone()));
        let _ = self.prompt_tx.send(text.clone());
        self.input.clear();
        self.cursor = 0;
        self.busy = true;
        self.following = true;
        // Record in history (skip duplicates of the most recent entry).
        if self.input_history.last().map(|s| s.as_str()) != Some(text.as_str()) {
            self.input_history.push(text);
        }
        self.history_idx = None;
        self.history_draft.clear();
    }

    /// Normal Enter: send if idle, queue if busy.
    fn submit(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.busy {
            // Queue for after the current run finishes.
            self.queued = Some(text);
            self.input.clear();
            self.cursor = 0;
        } else {
            self.send_now(text);
        }
    }

    /// Ctrl+Enter: cancel current run and send input immediately.
    fn force_send(&mut self) {
        if self.pending_perm.is_some() {
            return;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.busy {
            // Cancel the running agent turn, then queue this as next prompt.
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.queued = Some(text);
            self.input.clear();
            self.cursor = 0;
            self.push(ChatLine::Info("  (interrupted — sending next)".into()));
        } else {
            self.send_now(text);
        }
    }

    fn push_status(&mut self, name: String, detail: String) {
        self.status_log.push_back(StatusEntry {
            name,
            detail,
            done: false,
            is_error: false,
            start: std::time::Instant::now(),
            elapsed_ms: None,
        });
        // Keep only the last 2 visible entries.
        while self.status_log.len() > 2 {
            self.status_log.pop_front();
        }
    }

    fn finish_status(&mut self, name: &str, is_error: bool) {
        // Mark the most recent in-progress entry with this name as done.
        for entry in self.status_log.iter_mut().rev() {
            if entry.name == name && !entry.done {
                entry.done = true;
                entry.is_error = is_error;
                entry.elapsed_ms = Some(entry.start.elapsed().as_millis() as u64);
                break;
            }
        }
    }

    /// Called when agent finishes a turn. Drains queue if any.
    fn on_agent_done(&mut self) {
        self.busy = false;
        self.status_log.clear();
        self.cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(queued) = self.queued.take() {
            self.send_now(queued);
        }
    }

    fn tick_spinner(&mut self) {
        if self.busy || self.pending_perm.is_some() {
            self.spinner_tick = (self.spinner_tick + 1) % SPINNER.len();
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Layout: header | chat | status (2) | queue (1) | hint (1) | input (3)
    let [header_area, chat_area, status_area, queue_area, hint_area, input_area] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .areas(area);

    // ── Header ──
    let version = env!("CARGO_PKG_VERSION");
    let mut header_spans = vec![
        Span::styled(
            "clido",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  v{}  ", version),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{}  {}", app.provider, app.model),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
    ];
    if app.session_cost_usd > 0.0 {
        let total_tok = app.session_input_tokens + app.session_output_tokens;
        let tok_str = if total_tok >= 1000 {
            format!("{:.1}k tok", total_tok as f64 / 1000.0)
        } else {
            format!("{} tok", total_tok)
        };
        header_spans.push(Span::styled(
            format!("   ${:.4}  {}", app.session_cost_usd, tok_str),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }
    let header = Paragraph::new(Line::from(header_spans));
    frame.render_widget(header, header_area);

    // ── Chat ──
    // Use ratatui's own line_count() so the scroll calculation matches actual rendering.
    let lines = build_lines(app);
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total_height = para.line_count(chat_area.width) as u16;
    let max_scroll = total_height.saturating_sub(chat_area.height);
    // Store for use in handle_key (Up/PageUp need the current max_scroll).
    app.max_scroll = max_scroll;
    let scroll = if app.following {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };
    frame.render_widget(para.scroll((scroll, 0)), chat_area);

    // ── Status strip ──
    {
        let status_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let spinner = SPINNER[app.spinner_tick];
        let mut slines: Vec<Line<'static>> = Vec::new();
        for entry in &app.status_log {
            let (icon, style, elapsed_str) = if entry.done {
                let ms = entry.elapsed_ms.unwrap_or(0);
                let t = format!("  {}ms", ms);
                if entry.is_error {
                    (
                        "✗",
                        Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                        t,
                    )
                } else {
                    ("✓", status_style, t)
                }
            } else {
                let elapsed = entry.start.elapsed();
                let secs = elapsed.as_secs_f64();
                let t = if secs < 1.0 {
                    format!("  {:.0}ms", elapsed.as_millis())
                } else {
                    format!("  {:.1}s", secs)
                };
                let running_color = tool_color(&entry.name, false, false);
                (
                    spinner,
                    Style::default()
                        .fg(running_color)
                        .add_modifier(Modifier::DIM),
                    t,
                )
            };
            slines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), style),
                Span::styled(entry.name.clone(), style),
                Span::styled(format!("  {}", entry.detail), status_style),
                Span::styled(elapsed_str, status_style),
            ]));
        }
        while slines.len() < 2 {
            slines.push(Line::raw(""));
        }
        frame.render_widget(Paragraph::new(slines), status_area);
    }

    // ── Queue strip ──
    {
        let queue_line = if let Some(ref q) = app.queued {
            let preview = if q.chars().count() > 60 {
                format!("{}…", q.chars().take(60).collect::<String>())
            } else {
                q.clone()
            };
            Line::from(vec![
                Span::styled(
                    "  ⟳ queued  ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("\"{}\"", preview),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::raw("")
        };
        frame.render_widget(Paragraph::new(queue_line), queue_area);
    }

    // ── Input box (always rendered, even when permission popup is showing) ──
    // Compute horizontal scroll so the cursor stays visible.
    // Visible width = input_area.width - 2 (borders) - 2 (leading " " + 1 margin).
    let input_visible_w = (input_area.width as usize).saturating_sub(4).max(1);
    if app.cursor < app.input_scroll {
        app.input_scroll = app.cursor;
    } else if app.cursor >= app.input_scroll + input_visible_w {
        app.input_scroll = app.cursor - input_visible_w + 1;
    }
    // Slice the visible window of the input.
    let input_display: String = app
        .input
        .chars()
        .skip(app.input_scroll)
        .take(input_visible_w)
        .collect();
    let cursor_col = (app.cursor - app.input_scroll) as u16;

    if app.busy || app.pending_perm.is_some() {
        let spinner = SPINNER[app.spinner_tick];
        let title_line = if app.pending_perm.is_some() {
            Line::from(vec![
                Span::styled("⏸", Style::default().fg(Color::LightMagenta)),
                Span::styled(" waiting for permission… ", Style::default().fg(Color::LightMagenta)),
            ])
        } else if app.queued.is_some() {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "queued — Ctrl+Enter to interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else if app.input.is_empty() {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "thinking…  (type to queue, Ctrl+Enter to interrupt)".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    "thinking…  Enter=queue  Ctrl+Enter=interrupt".to_string(),
                    Style::default().fg(Color::LightMagenta),
                ),
            ])
        };
        let block = Block::default()
            .title(title_line)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightMagenta));
        let para = Paragraph::new(format!(" {}", input_display)).block(block);
        frame.render_widget(para, input_area);
        if app.pending_perm.is_none() {
            frame.set_cursor_position((input_area.x + 2 + cursor_col, input_area.y + 1));
        }
    } else {
        let idle_title = if app.input.is_empty() {
            " Ask anything  (Enter=send  ↑↓=history  /help=commands) ".to_string()
        } else {
            " Ask anything  (Enter=send  ↑↓=history  /help=commands) ".to_string()
        };
        let block = Block::default()
            .title(idle_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let para = Paragraph::new(format!(" {}", input_display)).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((input_area.x + 2 + cursor_col, input_area.y + 1));
    }

    // ── Hint line ──
    let hint_dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let mut hint_spans = vec![
        Span::styled("  Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(" send  ", hint_dim),
        Span::styled("↑↓", Style::default().fg(Color::DarkGray)),
        Span::styled(" history  ", hint_dim),
        Span::styled("PgUp/PgDn", Style::default().fg(Color::DarkGray)),
        Span::styled(" scroll  ", hint_dim),
        Span::styled("/help", Style::default().fg(Color::DarkGray)),
        Span::styled(" commands  ", hint_dim),
        Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
        Span::styled(" quit", hint_dim),
    ];
    if app.session_cost_usd > 0.0 {
        hint_spans.push(Span::styled(
            format!("  ${:.4}", app.session_cost_usd),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ));
    }
    // Scroll position indicator when not following.
    if app.max_scroll > 0 && !app.following {
        let pct = (app.scroll as u32 * 100 / app.max_scroll as u32).min(100);
        hint_spans.push(Span::styled(
            format!("  [{}%]", pct),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ));
    }
    let hint = Paragraph::new(Line::from(hint_spans));
    frame.render_widget(hint, hint_area);

    // ── "↓ new messages" scroll indicator ──
    if !app.following && app.max_scroll > app.scroll {
        let unread_hint = Span::styled(
            "  ↓ new  (PgDn) ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        );
        let hint_line = Line::from(vec![unread_hint]);
        let hint_para = Paragraph::new(hint_line);
        let hint_rect = Rect {
            x: chat_area.x + chat_area.width.saturating_sub(20),
            y: chat_area.y + chat_area.height.saturating_sub(1),
            width: 20,
            height: 1,
        };
        frame.render_widget(hint_para, hint_rect);
    }

    // ── Overlay modals (all rendered above the input field, same structure) ──
    //
    // Rendering order matters: later draws on top. Only one modal is active at
    // a time (handle_key enforces this), but we still render in priority order:
    //   slash completions < session picker < permission < error
    //
    // Shared helpers used by every modal:
    //   popup_above_input(input_area, h, w) → Rect anchored just above input
    //   modal_block(title, border_color)    → styled Block
    //   modal_row(label, selected)          → selectable option Line

    // ── Slash command popup ──
    let completions = slash_completions(&app.input);
    if !completions.is_empty() && app.pending_perm.is_none() && app.session_picker.is_none() {
        let popup_h = completions.len() as u16 + 2;
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width.min(54));
        frame.render_widget(Clear, popup_rect);
        let items: Vec<Line<'static>> = completions
            .iter()
            .enumerate()
            .map(|(i, (cmd, desc))| {
                let selected = app.selected_cmd == Some(i);
                modal_row_two_col(
                    format!(" {:<13}", cmd),
                    format!(" {}", desc),
                    Color::Cyan,
                    Color::DarkGray,
                    selected,
                )
            })
            .collect();
        frame.render_widget(
            Paragraph::new(items).block(modal_block("", Color::Blue)),
            popup_rect,
        );
    }

    // ── Session picker ───────────────────────────────────────────────────────
    if let Some(ref picker) = app.session_picker {
        const VISIBLE: usize = 12;
        let n_rows = picker.sessions.len().min(VISIBLE) as u16;
        // border(2) + header(1) + blank(1) + rows = n_rows + 4
        let popup_h = (n_rows + 4).min(input_area.y.saturating_sub(hint_area.y) + hint_area.y + input_area.y);
        let popup_h = popup_h.min(area.height.saturating_sub(4));
        let popup_h = (n_rows + 4).min(popup_h.max(6));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let inner_w = popup_rect.width.saturating_sub(4) as usize;
        // fixed cols: marker(2) id(8) sep(2) msg(3) sep(2) cost(6) sep(2) date(11) sep(2) = 38
        let preview_w = inner_w.saturating_sub(38).max(8);

        let mut content: Vec<Line<'static>> = vec![
            Line::from(vec![Span::styled(
                format!("  {:<8}  {:<3}  {:<6}  {:<11}  {}", "id", "msg", "cost", "date", "preview"),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )]),
            Line::raw(""),
        ];

        let end = (picker.scroll_offset + VISIBLE).min(picker.sessions.len());
        for (di, s) in picker.sessions[picker.scroll_offset..end].iter().enumerate() {
            let selected = picker.scroll_offset + di == picker.selected;
            let bg = if selected { Color::Blue } else { Color::Reset };
            let fg = if selected { Color::White } else { Color::Gray };
            let id_short = &s.session_id[..s.session_id.len().min(8)];
            let date_str = if s.start_time.len() >= 16 {
                format!("{} {}", &s.start_time[5..10], &s.start_time[11..16])
            } else {
                s.start_time.clone()
            };
            let preview_str: String = s.preview.chars().take(preview_w).collect();
            let marker = if selected { "▶ " } else { "  " };
            content.push(Line::from(vec![Span::styled(
                format!(
                    "{}{:<8}  {:>3}  ${:<5.2}  {:<11}  {}",
                    marker, id_short, s.num_turns, s.total_cost_usd, date_str, preview_str
                ),
                Style::default().fg(fg).bg(bg),
            )]));
        }

        // Add scroll indicators if there are more sessions above or below visible range.
        let above = picker.scroll_offset;
        let below = picker.sessions.len().saturating_sub(picker.scroll_offset + VISIBLE);
        if above > 0 || below > 0 {
            let mut scroll_parts = Vec::new();
            if above > 0 {
                scroll_parts.push(format!("↑↑ {} more above", above));
            }
            if below > 0 {
                scroll_parts.push(format!("↓↓ {} more below", below));
            }
            content.push(Line::from(vec![Span::styled(
                format!("  {}", scroll_parts.join("  ")),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )]));
        }

        let total = picker.sessions.len();
        let picker_title = format!(" Sessions — {} total  (↑↓  Enter=resume  Esc=close) ", total);
        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content)
                .block(modal_block(&picker_title, Color::Cyan)),
            popup_rect,
        );
    }

    // ── Permission popup ─────────────────────────────────────────────────────
    if let Some(perm) = &app.pending_perm {
        // 1 preview + 1 blank + 4 options + 2 borders = 8
        let popup_rect = popup_above_input(input_area, 8, input_area.width);
        let inner_w = popup_rect.width.saturating_sub(4) as usize;
        let preview = truncate_chars(&perm.preview, inner_w);

        const OPTIONS: &[&str] =
            &["Allow once", "Allow always — this session", "Allow all in workdir — this session", "Deny"];

        let mut content = vec![
            Line::from(vec![Span::styled(
                format!("  {}", preview),
                Style::default().fg(Color::DarkGray),
            )]),
            Line::raw(""),
        ];
        for (i, label) in OPTIONS.iter().enumerate() {
            content.push(modal_row(label, i == app.perm_selected));
        }

        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content)
                .block(modal_block(&format!(" Allow {}? ", perm.tool_name), Color::Yellow)),
            popup_rect,
        );
    }

    // ── Error modal ──────────────────────────────────────────────────────────
    if let Some(ref err_msg) = app.pending_error {
        let inner_w = input_area.width.saturating_sub(4) as usize;
        let wrapped = word_wrap(err_msg, inner_w);
        // blank + "[ OK ]" = +2; borders = +2
        let popup_h = ((wrapped.len() as u16) + 4).min(area.height.saturating_sub(4));
        let popup_rect = popup_above_input(input_area, popup_h, input_area.width);

        let mut content: Vec<Line<'static>> = wrapped
            .into_iter()
            .map(|l| {
                Line::from(vec![Span::styled(
                    format!("  {}", l),
                    Style::default().fg(Color::White),
                )])
            })
            .collect();
        content.push(Line::raw(""));
        content.push(Line::from(vec![Span::styled(
            "  [ OK ]  (Enter / Esc / Space)",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )]));

        frame.render_widget(Clear, popup_rect);
        frame.render_widget(
            Paragraph::new(content).block(modal_block(" Error ", Color::Red)),
            popup_rect,
        );
    }
}

// ── Modal component helpers ───────────────────────────────────────────────────

/// Rect anchored just above the input field (grows upward).
fn popup_above_input(input_area: Rect, h: u16, w: u16) -> Rect {
    let w = w.min(input_area.width);
    let x = input_area.x + (input_area.width.saturating_sub(w)) / 2;
    let y = input_area.y.saturating_sub(h);
    Rect { x, y, width: w, height: h }
}

/// Styled popup block — same structure for every modal.
fn modal_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

/// Single selectable option row with ▶ marker for selected state.
fn modal_row(label: &str, selected: bool) -> Line<'static> {
    if selected {
        Line::from(vec![
            Span::styled(" ▶ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(label.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(label.to_string(), Style::default().fg(Color::DarkGray)),
        ])
    }
}

/// Two-column row (e.g. for slash completions): cmd | description, with highlight on selection.
fn modal_row_two_col(
    left: String,
    right: String,
    left_color: Color,
    right_color: Color,
    selected: bool,
) -> Line<'static> {
    let bg = if selected { Color::Blue } else { Color::Reset };
    Line::from(vec![
        Span::styled(left, Style::default().fg(left_color).bg(bg).add_modifier(Modifier::BOLD)),
        Span::styled(right, Style::default().fg(right_color).bg(bg)),
    ])
}

/// Truncate a string to at most `max_chars` characters, appending `…` if cut.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars.saturating_sub(1)).collect::<String>())
    }
}

/// Word-wrap `text` to lines of at most `width` characters.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut cur = String::new();
        for word in paragraph.split_whitespace() {
            if cur.is_empty() {
                cur = word.to_string();
            } else if cur.len() + 1 + word.len() <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(cur);
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

/// Return the semantic color for a tool call based on its type and state.
fn tool_color(name: &str, done: bool, is_error: bool) -> Color {
    if is_error {
        return Color::Red;
    }
    if done {
        return Color::DarkGray;
    }
    match name {
        "Read" | "Glob" | "Grep" => Color::Blue,
        "Write" | "Edit" => Color::Green,
        "Bash" => Color::Yellow,
        "SemanticSearch" => Color::Cyan,
        "WebFetch" | "WebSearch" => Color::Magenta,
        _ => Color::White,
    }
}

fn slash_completions(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') {
        return vec![];
    }
    SLASH_COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(input))
        .map(|(cmd, desc)| (*cmd, *desc))
        .collect()
}

fn execute_slash(app: &mut App, cmd: &str) {
    match cmd {
        "/clear" => {
            app.messages.clear();
        }
        "/help" => {
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Info("  Navigation".into()));
            app.push(ChatLine::Info("  Enter              send message".into()));
            app.push(ChatLine::Info("  Ctrl+Enter         interrupt & send".into()));
            app.push(ChatLine::Info("  ↑↓ / PgUp/PgDn    scroll conversation".into()));
            app.push(ChatLine::Info("  ↑↓ (with input)   history navigation".into()));
            app.push(ChatLine::Info("  Ctrl+U             clear input".into()));
            app.push(ChatLine::Info("  Mouse scroll       scroll conversation".into()));
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Info("  Slash Commands".into()));
            app.push(ChatLine::Info("  /clear             clear conversation".into()));
            app.push(ChatLine::Info("  /help              show this help".into()));
            app.push(ChatLine::Info("  /model             show provider & model".into()));
            app.push(ChatLine::Info("  /session           show current session ID".into()));
            app.push(ChatLine::Info("  /sessions          list & resume recent sessions".into()));
            app.push(ChatLine::Info("  /workdir           show working directory".into()));
            app.push(ChatLine::Info("  /cost              show session cost so far".into()));
            app.push(ChatLine::Info("  /tokens            show token usage".into()));
            app.push(ChatLine::Info("  /memory <query>    search long-term memory".into()));
            app.push(ChatLine::Info("  /plan              show current task plan".into()));
            app.push(ChatLine::Info("  /index             show repo index stats".into()));
            app.push(ChatLine::Info("  /quit              exit".into()));
            app.push(ChatLine::Info("".into()));
            app.push(ChatLine::Info("  Agent Controls".into()));
            app.push(ChatLine::Info("  Ctrl+C             quit".into()));
            app.push(ChatLine::Info("  Ctrl+Enter         interrupt current run & send".into()));
            app.push(ChatLine::Info("  Queue              type while agent runs, sends on finish".into()));
            app.push(ChatLine::Info("".into()));
        }
        "/model" => {
            app.push(ChatLine::Info(format!(
                "  provider: {}   model: {}",
                app.provider, app.model
            )));
        }
        "/session" => {
            match &app.current_session_id {
                Some(id) => app.push(ChatLine::Info(format!("  session: {}", id))),
                None => app.push(ChatLine::Info("  no active session yet".into())),
            }
        }
        "/sessions" => {
            use clido_storage::list_sessions;
            match list_sessions(&app.workspace_root) {
                Err(e) => app.push(ChatLine::Info(format!("  error listing sessions: {}", e))),
                Ok(sessions) if sessions.is_empty() => {
                    app.push(ChatLine::Info("  no sessions found for this project".into()));
                }
                Ok(sessions) => {
                    let selected = sessions
                        .iter()
                        .position(|s| app.current_session_id.as_deref() == Some(&s.session_id))
                        .unwrap_or(0);
                    app.session_picker = Some(SessionPickerState {
                        sessions,
                        selected,
                        scroll_offset: 0,
                    });
                }
            }
        }
        "/workdir" => {
            let wd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into());
            app.push(ChatLine::Info(format!("  workdir: {}", wd)));
        }
        "/quit" => {
            app.quit = true;
        }
        _ if cmd.starts_with("/memory") => {
            let query = cmd.trim_start_matches("/memory").trim();
            if query.is_empty() {
                app.push(ChatLine::Info(
                    "  memory: use /memory <query> to search, or `clido memory list` in a new terminal".into(),
                ));
            } else {
                app.push(ChatLine::Info(format!(
                    "  memory search: the agent uses memory automatically. Run `clido memory list` or ask the agent to recall \"{}\".",
                    query
                )));
            }
        }
        "/cost" => {
            if app.session_cost_usd == 0.0 {
                app.push(ChatLine::Info("  cost: $0.0000 (no turns yet)".into()));
            } else {
                app.push(ChatLine::Info(format!(
                    "  cost: ${:.4}",
                    app.session_cost_usd
                )));
            }
        }
        "/tokens" => {
            app.push(ChatLine::Info(format!(
                "  tokens: {} input  {} output",
                app.session_input_tokens, app.session_output_tokens
            )));
        }
        "/plan" => {
            let plan_snapshot = app.last_plan.clone();
            match plan_snapshot {
                Some(tasks) if !tasks.is_empty() => {
                    app.push(ChatLine::Info("  plan  ┌─ Current plan:".into()));
                    let count = tasks.len();
                    for (i, task) in tasks.iter().enumerate() {
                        let prefix = if i + 1 == count { "        └─" } else { "        ├─" };
                        app.push(ChatLine::Info(format!("{} {}", prefix, task)));
                    }
                }
                _ => {
                    app.push(ChatLine::Info(
                        "  no plan — run with --planner to enable task decomposition".into(),
                    ));
                }
            }
        }
        "/index" => {
            app.push(ChatLine::Info(
                "  index: run `clido index build` to build the repo index, then the agent can use SemanticSearch.".into(),
            ));
        }
        _ => {}
    }
}

fn build_lines(app: &App) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                out.push(Line::from(vec![
                    Span::styled(
                        "you",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )
                ]));
                out.extend(render_markdown(text));
                out.push(Line::raw(""));
            }
            ChatLine::Assistant(text) => {
                out.push(Line::from(vec![Span::styled(
                    "clido",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]));
                out.extend(render_markdown(text));
                out.push(Line::raw(""));
            }
            ChatLine::Thinking(text) => {
                for part in text.lines() {
                    out.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled(
                            part.to_string(),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            }
            ChatLine::ToolCall { name, detail, done, is_error } => {
                let color = tool_color(name, *done, *is_error);
                let style = Style::default().fg(color);
                let icon = if *is_error {
                    "✗"
                } else if *done {
                    "✓"
                } else {
                    "↻"
                };
                let dim = Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM);
                if detail.is_empty() {
                    out.push(Line::from(vec![Span::styled(
                        format!("  {} {}", icon, name.clone()),
                        style,
                    )]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled(format!("  {} {}", icon, name.clone()), style),
                        Span::styled(format!("  {}", detail.clone()), dim),
                    ]));
                }
            }
            ChatLine::Diff(text) => {
                let mut old_lineno: u32 = 0;
                let mut new_lineno: u32 = 0;
                for line in text.lines() {
                    if line.starts_with("@@") {
                        if let Some((o, n)) = parse_hunk_header(line) {
                            old_lineno = o;
                            new_lineno = n;
                        }
                        out.push(Line::from(vec![Span::styled(
                            format!("  {}", line),
                            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with("---") || line.starts_with("+++") {
                        out.push(Line::from(vec![Span::styled(
                            format!("  {}", line),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                        )]));
                    } else if line.starts_with('+') {
                        let lineno = new_lineno;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else if line.starts_with('-') {
                        let lineno = old_lineno;
                        old_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    } else {
                        // context line — belongs to both
                        let lineno = new_lineno;
                        old_lineno += 1;
                        new_lineno += 1;
                        out.push(Line::from(vec![
                            Span::styled(
                                format!("  {:>4} ", lineno),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                            ),
                            Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                            ),
                        ]));
                    }
                }
                out.push(Line::raw(""));
            }
            ChatLine::Info(text) => {
                out.push(Line::from(vec![Span::styled(
                    if text.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", text)
                    },
                    Style::default().fg(Color::DarkGray),
                )]));
            }
        }
    }
    out
}

/// Parse `@@ -old_start[,len] +new_start[,len] @@` → (old_start, new_start).
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let inner = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old_start: u32 = old_part.trim_start_matches('-').split(',').next()?.parse().ok()?;
    let new_start: u32 = new_part.trim_start_matches('+').split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// Render markdown text into a series of tui `Line`s with appropriate styling.
fn render_markdown(text: &str) -> Vec<Line<'static>> {
    use pulldown_cmark::{Event, Tag};

    let mut out = Vec::new();
    let parser = Parser::new(text);

    // Stack to keep track of list indentation levels
    let mut list_stack: Vec<usize> = Vec::new();
    // Whether we're currently inside a list item (so text may start with a redundant marker)
    let mut in_list_item = false;
    // Current line buffer being built
    let mut current_line_spans: Vec<Span<'static>> = Vec::new();
    // Whether we're in a code block (and its indent)
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(tag) => {
                match tag {
                    Tag::Emphasis => {
                        current_line_spans.push(Span::styled(
                            String::new(),
                            Style::default().add_modifier(Modifier::ITALIC),
                        ));
                    }
                    Tag::Strong => {
                        current_line_spans.push(Span::styled(
                            String::new(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                    }
                    Tag::CodeBlock(_) => {
                        in_code_block = true;
                        // Code block starts on its own line
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                    }
                    Tag::List(_) => {
                        let indent = list_stack.last().copied().unwrap_or(0);
                        list_stack.push(indent + 2);
                    }
                    Tag::Item => {
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                        let indent = list_stack.last().copied().unwrap_or(0);
                        let bullet = "- ";
                        current_line_spans.push(Span::raw(" ".repeat(indent)));
                        current_line_spans.push(Span::styled(
                            bullet.to_string(),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ));
                        in_list_item = true;
                    }
                    Tag::Paragraph => {}
                    Tag::Heading(..) => {
                        current_line_spans.push(Span::styled(
                            String::new(),
                            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
                        ));
                    }
                    Tag::Link(..) => {
                        current_line_spans.push(Span::styled(
                            String::new(),
                            Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
                        ));
                    }
                    // Note: Inline code is Event::Code, not a tag pair
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    Tag::Emphasis | Tag::Strong | Tag::Link(..) => {
                        if !current_line_spans.is_empty() {
                            current_line_spans.pop();
                        }
                    }
                    Tag::CodeBlock(_) => {
                        in_code_block = false;
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                    }
                    Tag::List(_) => {
                        list_stack.pop();
                    }
                    Tag::Item => {
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                        in_list_item = false;
                    }
                    Tag::Paragraph => {
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                        out.push(Line::raw(""));
                    }
                    Tag::Heading(..) => {
                        if !current_line_spans.is_empty() {
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                        out.push(Line::raw(""));
                    }
                    _ => {}
                }
            }
            Event::Text(text) => {
                if in_code_block {
                    let lines = text.split('\n');
                    for (i, line) in lines.enumerate() {
                        if i > 0 {
                            if !current_line_spans.is_empty() {
                                out.push(Line::from(current_line_spans));
                                current_line_spans = Vec::new();
                            }
                        }
                        if !line.is_empty() {
                            current_line_spans.push(Span::styled(
                                line.to_string(),
                                Style::default().fg(Color::White).add_modifier(Modifier::DIM),
                            ));
                            out.push(Line::from(current_line_spans));
                            current_line_spans = Vec::new();
                        }
                    }
                } else {
                    // If we're inside a list item, strip leading list markers that the model may have included
                    let mut content = text.to_string();
                    if in_list_item {
                        // Strip leading "- ", "* ", "+ ", or "1. " etc.
                        let trimmed = content.trim_start();
                        if trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.starts_with('+') {
                            // Find first non-marker char after optional whitespace and numbers/dots
                            let mut chars = trimmed.chars();
                            // Skip the bullet char
                            chars.next();
                            // Skip following whitespace
                            let rest: String = chars.collect();
                            content = rest.trim_start().to_string();
                        }
                    }

                    if current_line_spans.is_empty() {
                        current_line_spans.push(Span::raw(content));
                    } else {
                        let last = current_line_spans.pop().unwrap();
                        let combined = format!("{}{}", last.content, content);
                        current_line_spans.push(Span::styled(combined, last.style));
                    }
                }
            }
            Event::Code(text) => {
                let span = Span::styled(
                    text.to_string(),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM),
                );
                if current_line_spans.is_empty() {
                    current_line_spans.push(span);
                } else {
                    let last = current_line_spans.pop().unwrap();
                    let combined = format!("{}{}", last.content, text);
                    current_line_spans.push(Span::styled(combined, last.style));
                }
            }
            Event::Html(_) => {}
            Event::SoftBreak => {
                if current_line_spans.is_empty() {
                    current_line_spans.push(Span::raw(" "));
                } else {
                    let last = current_line_spans.pop().unwrap();
                    let combined = format!("{} ", last.content);
                    current_line_spans.push(Span::styled(combined, last.style));
                }
            }
            Event::HardBreak => {
                if !current_line_spans.is_empty() {
                    out.push(Line::from(current_line_spans));
                    current_line_spans = Vec::new();
                }
                out.push(Line::raw(""));
            }
            Event::FootnoteReference(_) => {}
            Event::Rule => {}
            Event::TaskListMarker(_) => {}
        }
    }

    if !current_line_spans.is_empty() {
        out.push(Line::from(current_line_spans));
    }

    out
}

// ── Scroll helpers ────────────────────────────────────────────────────────────

fn scroll_up(app: &mut App, lines: u16) {
    if app.following {
        app.scroll = app.max_scroll;
    }
    app.scroll = app.scroll.saturating_sub(lines);
    app.following = false;
}

fn scroll_down(app: &mut App, lines: u16) {
    let new_scroll = app.scroll.saturating_add(lines);
    if new_scroll >= app.max_scroll {
        app.following = true;
    } else {
        app.scroll = new_scroll;
        app.following = false;
    }
}

// ── Input handling ────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, event: crossterm::event::KeyEvent) {
    use KeyCode::*;
    use KeyModifiers as Km;

    // Ctrl+C / Ctrl+D always quits
    if matches!(
        (event.modifiers, event.code),
        (Km::CONTROL, Char('c')) | (Km::CONTROL, Char('d'))
    ) {
        app.quit = true;
        return;
    }

    // ── Error modal (dismiss with Enter / Esc / Space) ───────────────────────
    if app.pending_error.is_some() {
        match event.code {
            Enter | Esc | Char(' ') => {
                app.pending_error = None;
            }
            _ => {}
        }
        return;
    }

    // ── Session picker (modal) ────────────────────────────────────────────────
    if app.session_picker.is_some() {
        const VISIBLE: usize = 12;
        match event.code {
            Up => {
                if let Some(picker) = &mut app.session_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                        if picker.selected < picker.scroll_offset {
                            picker.scroll_offset = picker.selected;
                        }
                    }
                }
            }
            Down => {
                if let Some(picker) = &mut app.session_picker {
                    if picker.selected + 1 < picker.sessions.len() {
                        picker.selected += 1;
                        if picker.selected >= picker.scroll_offset + VISIBLE {
                            picker.scroll_offset = picker.selected - VISIBLE + 1;
                        }
                    }
                }
            }
            Enter => {
                if let Some(picker) = app.session_picker.take() {
                    let id = picker.sessions[picker.selected].session_id.clone();
                    if app.current_session_id.as_deref() == Some(&id) {
                        app.push(ChatLine::Info("  already in this session".into()));
                    } else {
                        let _ = app.resume_tx.send(id);
                    }
                }
            }
            Esc => {
                app.session_picker = None;
            }
            _ => {}
        }
        return;
    }

    // ── Permission popup (modal — arrow keys select, Enter confirms) ─────────
    if app.pending_perm.is_some() {
        const PERM_OPTIONS: usize = 4;
        match event.code {
            Up => {
                if app.perm_selected == 0 {
                    app.perm_selected = PERM_OPTIONS - 1;
                } else {
                    app.perm_selected -= 1;
                }
            }
            Down => {
                app.perm_selected = (app.perm_selected + 1) % PERM_OPTIONS;
            }
            Enter => {
                if let Some(perm) = app.pending_perm.take() {
                    let grant = match app.perm_selected {
                        0 => PermGrant::Once,
                        1 => PermGrant::Session,
                        2 => PermGrant::Workdir,
                        _ => PermGrant::Deny,
                    };
                    let _ = perm.reply.send(grant);
                    app.perm_selected = 0;
                }
            }
            Esc => {
                if let Some(perm) = app.pending_perm.take() {
                    let _ = perm.reply.send(PermGrant::Deny);
                    app.perm_selected = 0;
                }
            }
            _ => {} // all other keys ignored while popup is active
        }
        return;
    }

    // ── Slash-command popup navigation ──────────────────────────────────────
    let completions = slash_completions(&app.input);
    if !completions.is_empty() {
        // Clamp selection in case completions shrunk.
        if let Some(sel) = app.selected_cmd {
            if sel >= completions.len() {
                app.selected_cmd = Some(completions.len() - 1);
            }
        }
        match (event.modifiers, event.code) {
            (_, Up) => {
                let sel = match app.selected_cmd {
                    None | Some(0) => completions.len() - 1,
                    Some(i) => i - 1,
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Down) => {
                let sel = match app.selected_cmd {
                    None => 0,
                    Some(i) => (i + 1) % completions.len(),
                };
                app.selected_cmd = Some(sel);
                return;
            }
            (_, Tab) => {
                let idx = app.selected_cmd.unwrap_or(0);
                if let Some((cmd, _)) = completions.get(idx) {
                    app.input = cmd.to_string();
                    app.cursor = app.input.chars().count();
                }
                app.selected_cmd = None;
                return;
            }
            (_, Enter) => {
                if let Some(idx) = app.selected_cmd {
                    let cmd = completions[idx].0.to_string();
                    app.input.clear();
                    app.cursor = 0;
                    app.selected_cmd = None;
                    execute_slash(app, &cmd);
                    return;
                }
                // No item selected → fall through to normal Enter handling.
            }
            (_, Esc) => {
                app.selected_cmd = None;
                return;
            }
            _ => {}
        }
    } else {
        app.selected_cmd = None;
    }

    match (event.modifiers, event.code) {
        // Ctrl+Enter: interrupt current run and send immediately.
        (Km::CONTROL, Enter) => app.force_send(),
        (_, Enter) => {
            // Execute slash command if input starts with a known command prefix; otherwise normal send.
            let trimmed = app.input.trim().to_string();
            let is_slash_cmd = trimmed.starts_with('/')
                && SLASH_COMMANDS.iter().any(|(cmd, _)| {
                    // Exact match or prefix match (for commands that accept arguments like /memory <query>).
                    trimmed == *cmd || trimmed.starts_with(&format!("{} ", cmd))
                });
            if is_slash_cmd {
                app.input.clear();
                app.cursor = 0;
                execute_slash(app, &trimmed);
            } else {
                app.submit();
            }
        }
        (_, Backspace) => {
            if app.cursor > 0 {
                let byte_pos = char_byte_pos(&app.input, app.cursor - 1);
                app.input.remove(byte_pos);
                app.cursor -= 1;
                app.selected_cmd = None;
                app.history_idx = None;
            }
        }
        (_, Delete) => {
            if app.cursor < app.input.chars().count() {
                let byte_pos = char_byte_pos(&app.input, app.cursor);
                app.input.remove(byte_pos);
                app.selected_cmd = None;
                app.history_idx = None;
            }
        }
        (_, Left) => {
            if app.cursor > 0 {
                app.cursor -= 1;
            }
        }
        (_, Right) => {
            if app.cursor < app.input.chars().count() {
                app.cursor += 1;
            }
        }
        (_, Home) => app.cursor = 0,
        (_, End) => app.cursor = app.input.chars().count(),
        // ── Up: scroll chat (empty input) or history navigation (with input) ──
        (_, Up) if app.pending_perm.is_none() && slash_completions(&app.input).is_empty() => {
            if app.input.is_empty() && app.history_idx.is_none() {
                // Scroll chat up.
                scroll_up(app, 2);
            } else {
                // Input history navigation.
                if app.input_history.is_empty() {
                    return;
                }
                let new_idx = match app.history_idx {
                    None => {
                        app.history_draft = app.input.clone();
                        app.input_history.len() - 1
                    }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                app.history_idx = Some(new_idx);
                app.input = app.input_history[new_idx].clone();
                app.cursor = app.input.chars().count();
                app.selected_cmd = None;
            }
        }
        // ── Down: scroll chat (empty input, not in history) or history nav ───
        (_, Down) if app.pending_perm.is_none() && slash_completions(&app.input).is_empty() => {
            if app.input.is_empty() && app.history_idx.is_none() {
                scroll_down(app, 2);
            } else {
                match app.history_idx {
                    None => {}
                    Some(i) if i + 1 >= app.input_history.len() => {
                        app.history_idx = None;
                        app.input = app.history_draft.clone();
                        app.cursor = app.input.chars().count();
                        app.selected_cmd = None;
                    }
                    Some(i) => {
                        let new_idx = i + 1;
                        app.history_idx = Some(new_idx);
                        app.input = app.input_history[new_idx].clone();
                        app.cursor = app.input.chars().count();
                        app.selected_cmd = None;
                    }
                }
            }
        }
        // ── Chat scroll (PageUp/PageDown — larger jumps) ─────────────────────
        (_, PageUp) => {
            scroll_up(app, 10);
        }
        (_, PageDown) => {
            scroll_down(app, 10);
        }
        (Km::CONTROL, Char('u')) => {
            app.input.clear();
            app.cursor = 0;
            app.selected_cmd = None;
            app.history_idx = None;
        }
        // Allow typing at all times (even while busy) for queue/force-send.
        (_, Char(c)) => {
            let byte_pos = char_byte_pos(&app.input, app.cursor);
            app.input.insert(byte_pos, c);
            app.cursor += 1;
            app.selected_cmd = None;
            // Any manual edit breaks out of history navigation.
            app.history_idx = None;
        }
        _ => {}
    }
}

/// Return the byte position of the n-th character boundary in `s`.
fn char_byte_pos(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

// ── Agent background task ─────────────────────────────────────────────────────

enum AgentAction {
    Run(String),
    Resume(String),
}

async fn agent_task(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    mut prompt_rx: mpsc::UnboundedReceiver<String>,
    mut resume_rx: mpsc::UnboundedReceiver<String>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    perm_tx: mpsc::UnboundedSender<PermRequest>,
    cancel: std::sync::Arc<AtomicBool>,
) {
    let mut setup = match AgentSetup::build(&cli, &workspace_root) {
        Ok(s) => s,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };

    let perms = Arc::new(Mutex::new(PermsState::default()));
    setup.ask_user = Some(Arc::new(TuiAskUser { perm_tx, perms }));

    let session_id = uuid::Uuid::new_v4().to_string();
    let mut writer = match SessionWriter::create(&workspace_root, &session_id) {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };
    let _ = event_tx.send(AgentEvent::SessionStarted(session_id.clone()));

    let emitter: Arc<dyn EventEmitter> = Arc::new(TuiEmitter {
        tx: event_tx.clone(),
    });

    let planner_mode = cli.planner;
    let mut agent = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
        .with_emitter(emitter)
        .with_planner(planner_mode);

    let mut first_turn = true;
    // Auto-continue counter: when the turn limit is hit mid-task, clido automatically
    // injects "please continue" so the agent never stops mid-work. We cap this at
    // MAX_AUTO_CONTINUES to avoid infinite loops on genuinely stuck agents.
    const MAX_AUTO_CONTINUES: u32 = 5;
    let mut auto_continue_count: u32 = 0;

    loop {
        let action = tokio::select! {
            msg = prompt_rx.recv() => {
                match msg {
                    Some(prompt) => AgentAction::Run(prompt),
                    None => break,
                }
            }
            resume_id = resume_rx.recv() => {
                match resume_id {
                    Some(id) => AgentAction::Resume(id),
                    None => break,
                }
            }
        };

        match action {
            AgentAction::Run(prompt) => {
                cancel.store(false, std::sync::atomic::Ordering::Relaxed);

                // When --planner is active and this is the first turn, attempt to parse
                // a plan from the prompt. On success, emit PlanCreated so the TUI can
                // display the planned steps. On any failure, fall back to the reactive loop
                // transparently (no error shown to the user).
                if planner_mode && first_turn {
                    // Make a real LLM call to decompose the task into a JSON plan.
                    // On any failure (network, parse, invalid graph), silently fall back
                    // to the reactive loop — no error is shown to the user.
                    let planning_prompt = format!(
                        "You are a task planner. Decompose the following task into a JSON task graph.\n\
                         Format: {{\"goal\":\"<goal>\",\"tasks\":[{{\"id\":\"t1\",\"description\":\"<description>\",\"depends_on\":[]}},...]}}\n\
                         Tasks that can run in parallel should have no shared dependencies.\n\
                         Keep it to 2-5 tasks maximum. Respond with ONLY the JSON, no explanation.\n\n\
                         Task: {}",
                        prompt
                    );
                    if let Ok(plan_text) = agent.complete_simple(&planning_prompt).await {
                        if let Ok(graph) = clido_planner::parse_plan(&plan_text) {
                            let task_descriptions: Vec<String> = graph
                                .tasks
                                .iter()
                                .map(|t| {
                                    if t.depends_on.is_empty() {
                                        format!("{}: {}", t.id, t.description)
                                    } else {
                                        format!(
                                            "{}: {}  (depends: {})",
                                            t.id,
                                            t.description,
                                            t.depends_on.join(", ")
                                        )
                                    }
                                })
                                .collect();
                            let _ = event_tx.send(AgentEvent::PlanCreated {
                                tasks: task_descriptions,
                            });
                        }
                        // If parse fails, silently proceed (fallback to reactive)
                    }
                }

                let result = if first_turn {
                    agent
                        .run(
                            &prompt,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                } else {
                    agent
                        .run_next_turn(
                            &prompt,
                            Some(&mut writer),
                            Some(&setup.pricing_table),
                            Some(cancel.clone()),
                        )
                        .await
                };
                first_turn = false;

                // Emit token usage before response/error so TUI updates cost display.
                let _ = event_tx.send(AgentEvent::TokenUsage {
                    input_tokens: agent.cumulative_input_tokens,
                    output_tokens: agent.cumulative_output_tokens,
                    cost_usd: agent.cumulative_cost_usd,
                });

                match result {
                    Ok(text) => {
                        auto_continue_count = 0; // reset on clean completion
                        let _ = event_tx.send(AgentEvent::Response(text));
                    }
                    Err(ClidoError::Interrupted) => {
                        auto_continue_count = 0;
                        let _ = event_tx.send(AgentEvent::Interrupted);
                    }
                    Err(ClidoError::MaxTurnsExceeded) => {
                        auto_continue_count += 1;
                        if auto_continue_count <= MAX_AUTO_CONTINUES {
                            // Silently inject a continue prompt — the agent picks up from
                            // exactly where it left off since history is intact.
                            let _ = event_tx.send(AgentEvent::Thinking(
                                "↻ Continuing (turn limit reached)…".to_string(),
                            ));
                            // Call run_next_turn directly with a continue message.
                            let continue_result = agent
                                .run_next_turn(
                                    "Please continue where you left off.",
                                    Some(&mut writer),
                                    Some(&setup.pricing_table),
                                    Some(cancel.clone()),
                                )
                                .await;
                            let _ = event_tx.send(AgentEvent::TokenUsage {
                                input_tokens: agent.cumulative_input_tokens,
                                output_tokens: agent.cumulative_output_tokens,
                                cost_usd: agent.cumulative_cost_usd,
                            });
                            match continue_result {
                                Ok(text) => {
                                    auto_continue_count = 0;
                                    let _ = event_tx.send(AgentEvent::Response(text));
                                }
                                Err(ClidoError::Interrupted) => {
                                    auto_continue_count = 0;
                                    let _ = event_tx.send(AgentEvent::Interrupted);
                                }
                                Err(e) => {
                                    let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                                }
                            }
                        } else {
                            // Hard cap hit: surface a friendly, actionable message.
                            let _ = event_tx.send(AgentEvent::Err(format!(
                                "Reached the turn limit {} times without finishing.\n\
                                 History is intact — type \"continue\" to keep going,\n\
                                 or start a new task.",
                                MAX_AUTO_CONTINUES
                            )));
                            auto_continue_count = 0; // reset so next message works
                        }
                    }
                    Err(ClidoError::BudgetExceeded) => {
                        // Budget limit is a real stop — always show it.
                        let _ = event_tx.send(AgentEvent::Err(
                            "Budget limit reached. Increase --max-budget-usd or check your config."
                                .to_string(),
                        ));
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(e.to_string()));
                    }
                }

                let _ = writer.write_line(&clido_storage::SessionLine::Result {
                    exit_status: "done".to_string(),
                    total_cost_usd: agent.cumulative_cost_usd,
                    num_turns: agent.turn_count(),
                    duration_ms: 0,
                });
            }
            AgentAction::Resume(resume_session_id) => {
                match clido_storage::SessionReader::load(&workspace_root, &resume_session_id) {
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Err(format!("resume failed: {}", e)));
                    }
                    Ok(lines) => {
                        let new_history = clido_agent::session_lines_to_messages(&lines);
                        agent.replace_history(new_history);
                        // Switch the writer to append to the resumed session.
                        match SessionWriter::append(&workspace_root, &resume_session_id) {
                            Ok(new_writer) => {
                                writer = new_writer;
                            }
                            Err(e) => {
                                let _ = event_tx
                                    .send(AgentEvent::Err(format!("resume writer: {}", e)));
                            }
                        }
                        // Collect display messages for the TUI (user + assistant).
                        let mut msgs: Vec<(String, String)> = Vec::new();
                        for line in &lines {
                            match line {
                                clido_storage::SessionLine::UserMessage { content, .. } => {
                                    if let Some(t) = content
                                        .first()
                                        .and_then(|c| c.get("text"))
                                        .and_then(|v| v.as_str())
                                    {
                                        msgs.push(("user".to_string(), t.to_string()));
                                    }
                                }
                                clido_storage::SessionLine::AssistantMessage { content } => {
                                    // Concatenate all text blocks into one string.
                                    let text: String = content
                                        .iter()
                                        .filter_map(|c| {
                                            if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                                                c.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("");
                                    if !text.trim().is_empty() {
                                        msgs.push(("assistant".to_string(), text));
                                    }
                                }
                                _ => {}
                            }
                        }
                        first_turn = false; // already have history
                        let _ = event_tx.send(AgentEvent::ResumedSession { messages: msgs });
                    }
                }
            }
        }
    }

    let _ = writer.flush();
}

fn read_provider_model(cli: &Cli, workspace_root: &std::path::Path) -> (String, String) {
    use clido_core::load_config;
    let Ok(loaded) = load_config(workspace_root) else {
        return ("?".into(), "?".into());
    };
    let profile_name = cli
        .profile
        .as_deref()
        .unwrap_or(loaded.default_profile.as_str());
    let Ok(profile) = loaded.get_profile(profile_name) else {
        return ("?".into(), "?".into());
    };
    let model = cli.model.clone().unwrap_or_else(|| profile.model.clone());
    let provider = cli
        .provider
        .clone()
        .unwrap_or_else(|| profile.provider.clone());
    (provider, model)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run_tui(cli: Cli) -> Result<(), anyhow::Error> {
    let workspace_root = cli
        .workdir
        .clone()
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    let (provider, model) = read_provider_model(&cli, &workspace_root);

    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<String>();
    let (resume_tx, resume_rx) = mpsc::unbounded_channel::<String>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (perm_tx, mut perm_rx) = mpsc::unbounded_channel::<PermRequest>();

    let cancel = std::sync::Arc::new(AtomicBool::new(false));

    tokio::spawn(agent_task(
        cli,
        workspace_root.clone(),
        prompt_rx,
        resume_rx,
        event_tx,
        perm_tx,
        cancel.clone(),
    ));

    // Install a panic hook so the terminal is always restored even on crash.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stderr(), LeaveAlternateScreen);
        #[cfg(unix)]
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) == 0 && t.c_lflag & libc::ICANON == 0 {
                t.c_iflag |= (libc::ICRNL | libc::IXON) as libc::tcflag_t;
                t.c_oflag |= (libc::OPOST | libc::ONLCR) as libc::tcflag_t;
                t.c_lflag |= (libc::ICANON
                    | libc::ECHO
                    | libc::ECHOE
                    | libc::ECHOK
                    | libc::ISIG
                    | libc::IEXTEN) as libc::tcflag_t;
                libc::tcsetattr(0, libc::TCSAFLUSH, &t);
            }
        }
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(prompt_tx, resume_tx, cancel, provider, model, workspace_root.clone());
    let result = event_loop(&mut app, &mut terminal, &mut event_rx, &mut perm_rx).await;

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), crossterm::event::DisableMouseCapture, LeaveAlternateScreen);
    result
}

async fn event_loop(
    app: &mut App,
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    event_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
    perm_rx: &mut mpsc::UnboundedReceiver<PermRequest>,
) -> Result<(), anyhow::Error> {
    let mut crossterm_events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(80));

    loop {
        terminal.draw(|f| render(f, app))?;

        tokio::select! {
            _ = tick.tick() => {
                app.tick_spinner();
            }
            maybe = crossterm_events.next() => {
                match maybe {
                    Some(Ok(Event::Key(key))) => handle_key(app, key),
                    Some(Ok(Event::Mouse(m))) => match m.kind {
                        MouseEventKind::ScrollUp => scroll_up(app, 1),
                        MouseEventKind::ScrollDown => scroll_down(app, 1),
                        _ => {}
                    },
                    Some(Ok(Event::Resize(_, _))) => {}
                    None => break,
                    _ => {}
                }
            }
            maybe = event_rx.recv() => {
                match maybe {
                    Some(AgentEvent::ToolStart { name, detail }) => {
                        app.push_status(name.clone(), detail.clone());
                        app.push(ChatLine::ToolCall { name, detail, done: false, is_error: false });
                    }
                    Some(AgentEvent::ToolDone { name, is_error, diff }) => {
                        app.finish_status(&name, is_error);
                        for line in app.messages.iter_mut().rev() {
                            if let ChatLine::ToolCall { name: n, done, is_error: e, .. } = line {
                                if n == &name && !*done {
                                    *done = true;
                                    *e = is_error;
                                    break;
                                }
                            }
                        }
                        if let Some(d) = diff {
                            if !d.is_empty() {
                                app.push(ChatLine::Diff(d));
                            }
                        }
                    }
                    Some(AgentEvent::Thinking(text)) => {
                        app.push(ChatLine::Thinking(text));
                        // Don't call on_agent_done — the agent is still running.
                    }
                    Some(AgentEvent::Response(text)) => {
                        app.push(ChatLine::Assistant(text));
                        app.on_agent_done();
                    }
                    Some(AgentEvent::SessionStarted(id)) => {
                        app.current_session_id = Some(id);
                    }
                    Some(AgentEvent::Interrupted) => {
                        app.on_agent_done();
                    }
                    Some(AgentEvent::Err(msg)) => {
                        app.pending_error = Some(msg);
                        app.on_agent_done();
                    }
                    Some(AgentEvent::ResumedSession { messages }) => {
                        app.messages.clear();
                        for line in crate::ui::BANNER.lines() {
                            app.messages.push(ChatLine::Info(line.to_string()));
                        }
                        app.messages.push(ChatLine::Info(String::new()));
                        app.messages.push(ChatLine::Info("  — resumed session —".into()));
                        for (role, text) in messages {
                            if role == "user" {
                                app.push(ChatLine::User(text));
                            } else if role == "assistant" {
                                app.push(ChatLine::Assistant(text));
                            }
                        }
                        app.busy = false;
                    }
                    Some(AgentEvent::TokenUsage { input_tokens, output_tokens, cost_usd }) => {
                        app.session_input_tokens = input_tokens;
                        app.session_output_tokens = output_tokens;
                        app.session_cost_usd = cost_usd;
                    }
                    Some(AgentEvent::PlanCreated { tasks }) => {
                        // Display the plan in the chat as an info block.
                        app.push(ChatLine::Info("  plan  ┌─ Planned tasks:".to_string()));
                        let count = tasks.len();
                        for (i, task) in tasks.iter().enumerate() {
                            let prefix = if i + 1 == count { "        └─" } else { "        ├─" };
                            app.push(ChatLine::Info(format!("{} {}", prefix, task)));
                        }
                        // Store last plan so /plan command can show it later.
                        app.last_plan = Some(tasks);
                    }
                    None => {}
                }
            }
            maybe = perm_rx.recv() => {
                if let Some(req) = maybe {
                    app.pending_perm = Some(PendingPerm {
                        tool_name: req.tool_name,
                        preview: req.preview,
                        reply: req.reply,
                    });
                    // Don't clear busy — agent is still running, awaiting our reply.
                }
            }
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}
