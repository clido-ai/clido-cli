use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clido_agent::{
    AskUser, EventEmitter, PermGrant as AgentPermGrant, PermRequest as AgentPermRequest,
};
use tokio::sync::{mpsc, oneshot};

use super::AgentEvent;

// ── Permission grant options ───────────────────────────────────────────────────

#[derive(Debug)]
pub(super) enum PermGrant {
    /// Allow this single invocation.
    Once,
    /// Allow this tool for the rest of the session.
    Session,
    /// Allow all tools for the rest of the session (workdir-wide).
    Workdir,
    /// Deny.
    Deny,
    /// Deny with feedback message sent back to the agent.
    DenyWithFeedback(String),
}

// ── Session-level permission state (shared between TuiAskUser calls) ──────────

#[derive(Default)]
pub(super) struct PermsState {
    /// Tool names granted for the whole session.
    pub(super) session_allowed: HashSet<String>,
    /// All tools open for this session (workdir-wide grant).
    pub(super) workdir_open: bool,
}

impl PermsState {
    pub(super) fn clear_all_grants(&mut self) {
        self.session_allowed.clear();
        self.workdir_open = false;
    }
}

// ── Permission request (agent → TUI, reply via oneshot) ───────────────────────

pub(super) struct PermRequest {
    pub(super) tool_name: String,
    pub(super) preview: String,
    pub(super) reply: oneshot::Sender<PermGrant>,
}

// ── TuiEmitter ────────────────────────────────────────────────────────────────

pub(super) struct TuiEmitter {
    pub(super) tx: mpsc::Sender<AgentEvent>,
    pub(super) unhealthy: Arc<AtomicBool>,
}

impl TuiEmitter {
    async fn send_ev(&self, ev: AgentEvent) {
        if self.tx.send(ev).await.is_err() {
            tracing::warn!(
                target: "clido::tui",
                "failed to send AgentEvent to TUI (channel closed)"
            );
            self.unhealthy.store(true, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl EventEmitter for TuiEmitter {
    async fn on_tool_start(&self, tool_use_id: &str, name: &str, input: &serde_json::Value) {
        let detail = format_tool_input(name, input);
        self.send_ev(AgentEvent::RunState(
            super::app_state::AppRunState::RunningTools,
        ))
        .await;
        self.send_ev(AgentEvent::ToolStart {
            tool_use_id: tool_use_id.to_string(),
            name: name.to_string(),
            detail,
        })
        .await;
    }
    async fn on_tool_done(
        &self,
        tool_use_id: &str,
        _name: &str,
        is_error: bool,
        diff: Option<String>,
    ) {
        self.send_ev(AgentEvent::ToolDone {
            tool_use_id: tool_use_id.to_string(),
            is_error,
            diff,
        })
        .await;
        self.send_ev(AgentEvent::RunState(
            super::app_state::AppRunState::Generating,
        ))
        .await;
    }
    async fn on_assistant_text(&self, text: &str) {
        if !text.trim().is_empty() {
            self.send_ev(AgentEvent::Thinking(text.to_string())).await;
        }
    }

    async fn on_budget_warning(&self, pct: u8, spent_usd: f64, limit_usd: f64) {
        self.send_ev(AgentEvent::BudgetWarning {
            percent: pct,
            cost: spent_usd,
            limit: limit_usd,
        })
        .await;
    }

    async fn on_path_permission_request(&self, path: &std::path::Path, tool_name: &str) {
        self.send_ev(AgentEvent::PathPermissionRequest {
            path: path.to_path_buf(),
            tool_name: tool_name.to_string(),
        })
        .await;
    }
}

pub(super) fn format_tool_input(name: &str, input: &serde_json::Value) -> String {
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
        "MultiEdit" => {
            let n = input["edits"].as_array().map(|a| a.len()).unwrap_or(0);
            let first = input["edits"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|o| o.get("file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if n == 0 {
                "multi-edit".to_string()
            } else if n == 1 {
                format!("{}  ·  {}", n, first)
            } else {
                format!("{} edits  ·  {} …", n, first)
            }
        }
        _ => input.to_string(),
    };
    if s.chars().count() > 72 {
        format!("{}…", s.chars().take(72).collect::<String>())
    } else {
        s
    }
}

// ── TuiAskUser ────────────────────────────────────────────────────────────────

pub(super) struct TuiAskUser {
    pub(super) perm_tx: mpsc::UnboundedSender<PermRequest>,
    pub(super) perms: Arc<Mutex<PermsState>>,
}

#[async_trait]
impl AskUser for TuiAskUser {
    async fn ask(&self, req: AgentPermRequest) -> AgentPermGrant {
        let tool_name = &req.tool_name;
        // Fast-path: check session/workdir grants before going to the TUI.
        {
            let state = self.perms.lock().unwrap();
            if state.workdir_open || state.session_allowed.contains(tool_name) {
                return AgentPermGrant::Allow;
            }
        }

        let preview = if req.description.chars().count() > 120 {
            format!("{}…", req.description.chars().take(120).collect::<String>())
        } else {
            req.description.clone()
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .perm_tx
            .send(PermRequest {
                tool_name: tool_name.clone(),
                preview,
                reply: reply_tx,
            })
            .is_err()
        {
            return AgentPermGrant::Deny;
        }
        match reply_rx.await.unwrap_or(PermGrant::Deny) {
            PermGrant::Once => AgentPermGrant::Allow,
            PermGrant::Session => {
                self.perms
                    .lock()
                    .unwrap()
                    .session_allowed
                    .insert(tool_name.clone());
                AgentPermGrant::AllowAll
            }
            PermGrant::Workdir => {
                self.perms.lock().unwrap().workdir_open = true;
                AgentPermGrant::AllowAll
            }
            PermGrant::Deny => AgentPermGrant::Deny,
            PermGrant::DenyWithFeedback(fb) => AgentPermGrant::DenyWithFeedback(fb),
        }
    }
}
