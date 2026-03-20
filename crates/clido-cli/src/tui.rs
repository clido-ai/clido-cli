//! Full-screen ratatui TUI: scrollable conversation + persistent input bar.

use std::env;
use std::io::{stdout, Write as _};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clido_agent::{AgentLoop, AskUser, EventEmitter};
use clido_core::ClidoError;
use clido_storage::SessionWriter;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
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
    ("/help", "show key bindings"),
    ("/model", "show current model and provider"),
    ("/workdir", "show working directory"),
    ("/quit", "exit clido"),
];

// ── Agent → TUI events ────────────────────────────────────────────────────────

enum AgentEvent {
    ToolStart { name: String, detail: String },
    ToolDone { name: String, is_error: bool },
    Response(String),
    Interrupted,
    Err(String),
}

// ── Permission request (agent → TUI, reply via oneshot) ───────────────────────

struct PermRequest {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<bool>,
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
    async fn on_tool_done(&self, name: &str, is_error: bool) {
        let _ = self.tx.send(AgentEvent::ToolDone {
            name: name.to_string(),
            is_error,
        });
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
}

#[async_trait]
impl AskUser for TuiAskUser {
    async fn ask(&self, tool_name: &str, input: &serde_json::Value) -> bool {
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
        reply_rx.await.unwrap_or(false)
    }
}

// ── Status strip ─────────────────────────────────────────────────────────────

struct StatusEntry {
    name: String,
    detail: String,
    done: bool,
    is_error: bool,
}

// ── Chat lines ────────────────────────────────────────────────────────────────

enum ChatLine {
    User(String),
    Assistant(String),
    ToolCall {
        name: String,
        done: bool,
        is_error: bool,
    },
    Error(String),
    Info(String),
}

// ── App state ─────────────────────────────────────────────────────────────────

struct PendingPerm {
    tool_name: String,
    preview: String,
    reply: oneshot::Sender<bool>,
}

struct App {
    messages: Vec<ChatLine>,
    /// Live activity log shown in the status strip (last 2 entries).
    status_log: std::collections::VecDeque<StatusEntry>,
    input: String,
    cursor: usize,
    scroll: u16,
    following: bool,
    busy: bool,
    spinner_tick: usize,
    pending_perm: Option<PendingPerm>,
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Queued input typed while agent was busy — sent automatically when agent finishes.
    queued: Option<String>,
    /// Signal to cancel the current agent run (force send).
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Selected index in the slash-command popup (None = no popup).
    selected_cmd: Option<usize>,
    quit: bool,
    provider: String,
    model: String,
}

impl App {
    fn new(
        prompt_tx: mpsc::UnboundedSender<String>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        provider: String,
        model: String,
    ) -> Self {
        let mut app = Self {
            messages: Vec::new(),
            status_log: std::collections::VecDeque::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            following: true,
            busy: false,
            spinner_tick: 0,
            pending_perm: None,
            prompt_tx,
            queued: None,
            cancel,
            selected_cmd: None,
            quit: false,
            provider,
            model,
        };
        for line in crate::ui::BANNER.lines() {
            app.messages.push(ChatLine::Info(line.to_string()));
        }
        app.messages.push(ChatLine::Info(String::new()));
        app
    }

    fn push(&mut self, line: ChatLine) {
        self.messages.push(line);
        // scroll position is computed at render time when following=true
    }

    /// Send immediately (not busy). Moves input → chat + agent.
    fn send_now(&mut self, text: String) {
        self.push(ChatLine::User(text.clone()));
        let _ = self.prompt_tx.send(text);
        self.input.clear();
        self.cursor = 0;
        self.busy = true;
        self.following = true;
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
            self.push(ChatLine::Info(
                "  (queued — will send when agent is done)".into(),
            ));
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

/// Estimate the number of visual rows a set of lines takes after word-wrap at `width`.
fn visual_height(lines: &[Line], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    lines
        .iter()
        .map(|line| {
            let chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            if chars == 0 {
                1u16
            } else {
                chars.div_ceil(width as usize) as u16
            }
        })
        .fold(0u16, |a, b| a.saturating_add(b))
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, chat_area, status_area, hint_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2), // live activity strip
        Constraint::Length(1), // key hint line
        Constraint::Length(3),
    ])
    .areas(area);

    // ── Header ──
    let version = env!("CARGO_PKG_VERSION");
    let header = Paragraph::new(Line::from(vec![
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
    ]));
    frame.render_widget(header, header_area);

    // ── Chat ──
    let lines = build_lines(app);
    // Compute visual row count accounting for word-wrap so autoscroll reaches the real bottom.
    let total_height = visual_height(&lines, chat_area.width);
    let max_scroll = total_height.saturating_sub(chat_area.height);
    let scroll = if app.following {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(para, chat_area);

    // ── Status strip ──
    {
        let status_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let spinner = SPINNER[app.spinner_tick];
        let mut slines: Vec<Line<'static>> = Vec::new();
        for entry in &app.status_log {
            let (icon, style) = if entry.done {
                if entry.is_error {
                    (
                        "✗",
                        Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                    )
                } else {
                    ("✓", status_style)
                }
            } else {
                (
                    spinner,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                )
            };
            slines.push(Line::from(vec![
                Span::styled(format!("  {} ", icon), style),
                Span::styled(entry.name.clone(), style),
                Span::styled(format!("  {}", entry.detail), status_style),
            ]));
        }
        // Pad to 2 lines so layout is stable.
        while slines.len() < 2 {
            slines.push(Line::raw(""));
        }
        frame.render_widget(Paragraph::new(slines), status_area);
    }

    // ── Input / spinner / permission ──
    if let Some(perm) = &app.pending_perm {
        let content = format!(
            " Allow {}?\n  {}\n [y] allow  [n] deny",
            perm.tool_name, perm.preview
        );
        let block = Block::default()
            .title(" Permission ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let para = Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(para, input_area);
    } else if app.busy {
        let spinner = SPINNER[app.spinner_tick];
        // While busy: show spinner but also allow typing (for queue/force-send).
        let title = if app.queued.is_some() {
            format!(" {} queued — Ctrl+Enter to interrupt ", spinner)
        } else if app.input.is_empty() {
            format!(
                " {} thinking…  (type to queue, Ctrl+Enter to interrupt) ",
                spinner
            )
        } else {
            format!(" {} thinking…  Enter=queue  Ctrl+Enter=interrupt ", spinner)
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        let para = Paragraph::new(format!(" {}", app.input)).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((input_area.x + 2 + app.cursor as u16, input_area.y + 1));
    } else {
        let block = Block::default()
            .title(" Message  (Enter to send) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let para = Paragraph::new(format!(" {}", app.input)).block(block);
        frame.render_widget(para, input_area);
        frame.set_cursor_position((input_area.x + 2 + app.cursor as u16, input_area.y + 1));
    }

    // ── Hint line ──
    let hint_dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(" send  ", hint_dim),
        Span::styled("Ctrl+Enter", Style::default().fg(Color::DarkGray)),
        Span::styled(" interrupt  ", hint_dim),
        Span::styled("↑↓", Style::default().fg(Color::DarkGray)),
        Span::styled(" scroll  ", hint_dim),
        Span::styled("/", Style::default().fg(Color::DarkGray)),
        Span::styled(" commands  ", hint_dim),
        Span::styled("Ctrl+C", Style::default().fg(Color::DarkGray)),
        Span::styled(" quit", hint_dim),
    ]));
    frame.render_widget(hint, hint_area);

    // ── Slash command popup ──
    let completions = slash_completions(&app.input);
    if !completions.is_empty() {
        let popup_h = completions.len() as u16 + 2;
        let popup_y = hint_area.y.saturating_sub(popup_h);
        let popup = Rect {
            x: input_area.x,
            y: popup_y,
            width: input_area.width.min(52),
            height: popup_h,
        };
        frame.render_widget(Clear, popup);
        let items: Vec<Line<'static>> = completions
            .iter()
            .enumerate()
            .map(|(i, (cmd, desc))| {
                let selected = app.selected_cmd == Some(i);
                let bg = if selected { Color::Blue } else { Color::Reset };
                Line::from(vec![
                    Span::styled(
                        format!(" {:<12}", cmd),
                        Style::default()
                            .fg(Color::Cyan)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {}", desc),
                        Style::default().fg(Color::DarkGray).bg(bg),
                    ),
                ])
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        frame.render_widget(Paragraph::new(items).block(block), popup);
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
            app.push(ChatLine::Info("  Enter          send message".into()));
            app.push(ChatLine::Info("  Ctrl+Enter     interrupt & send".into()));
            app.push(ChatLine::Info("  ↑↓ / PgUp/Dn  scroll conversation".into()));
            app.push(ChatLine::Info("  Ctrl+U         clear input".into()));
            app.push(ChatLine::Info("  /command       slash commands".into()));
            app.push(ChatLine::Info("  Ctrl+C         quit".into()));
        }
        "/model" => {
            app.push(ChatLine::Info(format!(
                "  provider: {}   model: {}",
                app.provider, app.model
            )));
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
                        "you  ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(text.clone()),
                ]));
                out.push(Line::raw(""));
            }
            ChatLine::Assistant(text) => {
                out.push(Line::from(vec![Span::styled(
                    "clido",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )]));
                for part in text.lines() {
                    out.push(Line::from(vec![
                        Span::raw("      "),
                        Span::raw(part.to_string()),
                    ]));
                }
                out.push(Line::raw(""));
            }
            ChatLine::ToolCall {
                name,
                done,
                is_error,
            } => {
                let (icon, style) = if *done {
                    if *is_error {
                        ("✗", Style::default().fg(Color::Red))
                    } else {
                        ("✓", Style::default().fg(Color::DarkGray))
                    }
                } else {
                    ("↻", Style::default().fg(Color::Yellow))
                };
                out.push(Line::from(vec![Span::styled(
                    format!("  {} {}", icon, name),
                    style,
                )]));
            }
            ChatLine::Error(text) => {
                out.push(Line::from(vec![Span::styled(
                    format!("  error: {}", text),
                    Style::default().fg(Color::Red),
                )]));
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

    // Permission overlay
    if app.pending_perm.is_some() {
        match event.code {
            Char('y') | Char('Y') | Enter => {
                if let Some(perm) = app.pending_perm.take() {
                    let _ = perm.reply.send(true);
                }
            }
            Char('n') | Char('N') | Esc => {
                if let Some(perm) = app.pending_perm.take() {
                    let _ = perm.reply.send(false);
                }
            }
            _ => {}
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
                // Fill input with selected (or first) command and close popup.
                let idx = app.selected_cmd.unwrap_or(0);
                if let Some((cmd, _)) = completions.get(idx) {
                    app.input = cmd.to_string();
                    app.cursor = app.input.chars().count();
                }
                app.selected_cmd = None;
                return;
            }
            (_, Enter) => {
                // Execute selected command if one is highlighted.
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
            // Execute slash command if input matches exactly; otherwise normal send.
            let trimmed = app.input.trim().to_string();
            if trimmed.starts_with('/') && SLASH_COMMANDS.iter().any(|(cmd, _)| *cmd == trimmed) {
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
            }
        }
        (_, Delete) => {
            if app.cursor < app.input.chars().count() {
                let byte_pos = char_byte_pos(&app.input, app.cursor);
                app.input.remove(byte_pos);
                app.selected_cmd = None;
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
        (_, Up) => {
            app.scroll = app.scroll.saturating_sub(1);
            app.following = false;
        }
        (_, Down) => {
            app.scroll = app.scroll.saturating_add(1);
        }
        (_, PageUp) => {
            app.scroll = app.scroll.saturating_sub(10);
            app.following = false;
        }
        (_, PageDown) => {
            app.scroll = app.scroll.saturating_add(10);
        }
        (Km::CONTROL, Char('u')) => {
            app.input.clear();
            app.cursor = 0;
            app.selected_cmd = None;
        }
        // Allow typing at all times (even while busy) for queue/force-send.
        (_, Char(c)) => {
            let byte_pos = char_byte_pos(&app.input, app.cursor);
            app.input.insert(byte_pos, c);
            app.cursor += 1;
            // Reset selection when input changes so popup re-opens fresh.
            app.selected_cmd = None;
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

async fn agent_task(
    cli: Cli,
    workspace_root: std::path::PathBuf,
    mut prompt_rx: mpsc::UnboundedReceiver<String>,
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

    setup.ask_user = Some(Arc::new(TuiAskUser { perm_tx }));

    let session_id = uuid::Uuid::new_v4().to_string();
    let mut writer = match SessionWriter::create(&workspace_root, &session_id) {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Err(e.to_string()));
            return;
        }
    };

    let emitter: Arc<dyn EventEmitter> = Arc::new(TuiEmitter {
        tx: event_tx.clone(),
    });

    let mut agent = AgentLoop::new(setup.provider, setup.registry, setup.config, setup.ask_user)
        .with_emitter(emitter);

    let mut first_turn = true;

    while let Some(prompt) = prompt_rx.recv().await {
        // Reset cancel flag at the start of each turn.
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);

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

        match result {
            Ok(text) => {
                let _ = event_tx.send(AgentEvent::Response(text));
            }
            Err(ClidoError::Interrupted) => {
                // Interrupted by force-send — signal done so TUI drains queue.
                let _ = event_tx.send(AgentEvent::Interrupted);
            }
            Err(e) => {
                let _ = event_tx.send(AgentEvent::Err(e.to_string()));
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

    // Read provider/model from config for the header (best-effort).
    let (provider, model) = read_provider_model(&cli, &workspace_root);

    let (prompt_tx, prompt_rx) = mpsc::unbounded_channel::<String>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (perm_tx, mut perm_rx) = mpsc::unbounded_channel::<PermRequest>();

    // Shared cancel token between TUI (force-send) and agent task.
    let cancel = std::sync::Arc::new(AtomicBool::new(false));

    tokio::spawn(agent_task(
        cli,
        workspace_root,
        prompt_rx,
        event_tx,
        perm_tx,
        cancel.clone(),
    ));

    // ── Terminal setup ──
    // Install a panic hook so the terminal is always restored even on crash.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort: spawn stty sane so the shell the user returns to is in cooked mode.
        let _ = std::process::Command::new("stty")
            .arg("sane")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stderr(), LeaveAlternateScreen);
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(prompt_tx, cancel, provider, model);
    let result = event_loop(&mut app, &mut terminal, &mut event_rx, &mut perm_rx).await;

    // ── Cleanup — always runs regardless of how the loop exits ──
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
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
                    Some(Ok(Event::Resize(_, _))) => {} // just re-render
                    None => break,
                    _ => {}
                }
            }
            maybe = event_rx.recv() => {
                match maybe {
                    Some(AgentEvent::ToolStart { name, detail }) => {
                        app.push_status(name.clone(), detail);
                        app.push(ChatLine::ToolCall { name, done: false, is_error: false });
                    }
                    Some(AgentEvent::ToolDone { name, is_error }) => {
                        app.finish_status(&name, is_error);
                        for line in app.messages.iter_mut().rev() {
                            if let ChatLine::ToolCall { name: n, done, is_error: e } = line {
                                if n == &name && !*done {
                                    *done = true;
                                    *e = is_error;
                                    break;
                                }
                            }
                        }
                    }
                    Some(AgentEvent::Response(text)) => {
                        app.push(ChatLine::Assistant(text));
                        app.on_agent_done();
                    }
                    Some(AgentEvent::Interrupted) => {
                        app.on_agent_done();
                    }
                    Some(AgentEvent::Err(msg)) => {
                        app.push(ChatLine::Error(msg));
                        app.on_agent_done();
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
                    app.busy = false;
                }
            }
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}
