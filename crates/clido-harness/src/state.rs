//! Canonical harness task model: externalized, append-only order, fail until verified pass.

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Top-level file: `.clido/harness/tasks.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessState {
    pub schema_version: u32,
    #[serde(default)]
    pub tasks: Vec<HarnessTask>,
    /// Stable ordering: append-only ids. Never remove or reorder entries.
    #[serde(default)]
    pub task_order: Vec<String>,
    #[serde(default)]
    pub meta: HarnessMeta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessMeta {
    /// Single task the executor may work on (must be `fail`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_focus_task_id: Option<String>,
    #[serde(default)]
    pub updated_at_rfc3339: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskPassState {
    /// Default — not verified.
    #[default]
    Fail,
    /// Only after structured verification.
    Pass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessTask {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub status: TaskPassState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<StoredVerification>,
    /// Last N approach fingerprints (executor loop detection).
    #[serde(default)]
    pub attempt_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVerification {
    pub recorded_at_rfc3339: String,
    pub commands_executed: Vec<String>,
    pub acceptance_results: Vec<AcceptanceResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceResult {
    pub criterion: String,
    pub passed: bool,
    pub evidence: String,
}

/// Input from evaluator tool call (before persistence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationPayload {
    pub commands_executed: Vec<String>,
    pub acceptance_results: Vec<AcceptanceResult>,
    #[serde(default)]
    pub reviewer_summary: Option<String>,
}

impl HarnessState {
    pub fn empty() -> Self {
        Self {
            schema_version: 1,
            tasks: Vec::new(),
            task_order: Vec::new(),
            meta: HarnessMeta::default(),
        }
    }

    pub fn task_map(&self) -> std::collections::HashMap<&str, usize> {
        self.tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.as_str(), i))
            .collect()
    }

    /// Append new tasks (planner). Ids must be unique.
    pub fn planner_append_tasks(&mut self, new_tasks: Vec<HarnessTask>) -> Result<()> {
        let existing: std::collections::HashSet<&str> =
            self.tasks.iter().map(|t| t.id.as_str()).collect();
        let mut batch_ids = std::collections::HashSet::new();
        for t in &new_tasks {
            if existing.contains(t.id.as_str()) || !batch_ids.insert(t.id.clone()) {
                return Err(HarnessError::DuplicateTaskId(t.id.clone()));
            }
        }
        for mut t in new_tasks {
            if t.status != TaskPassState::Fail {
                return Err(HarnessError::Protocol(format!(
                    "new task {} must start with status fail",
                    t.id
                )));
            }
            t.verification = None;
            t.attempt_fingerprints.clear();
            self.task_order.push(t.id.clone());
            self.tasks.push(t);
        }
        Ok(())
    }

    pub fn executor_set_focus(&mut self, task_id: &str) -> Result<()> {
        self.require_fail_task(task_id)?;
        self.meta.current_focus_task_id = Some(task_id.to_string());
        Ok(())
    }

    pub fn executor_clear_focus(&mut self) {
        self.meta.current_focus_task_id = None;
    }

    /// Record an approach fingerprint; tripping the guard returns Err.
    pub fn executor_register_attempt(&mut self, task_id: &str, fingerprint: &str) -> Result<()> {
        self.require_fail_task(task_id)?;
        let idx = self.task_index(task_id)?;
        let fp = fingerprint.trim();
        if fp.len() < 4 {
            return Err(HarnessError::Protocol(
                "fingerprint must be at least 4 characters".into(),
            ));
        }
        let t = &mut self.tasks[idx];
        t.attempt_fingerprints.push(fp.to_string());
        if t.attempt_fingerprints.len() > 20 {
            let drain = t.attempt_fingerprints.len() - 20;
            t.attempt_fingerprints.drain(0..drain);
        }
        let n = t.attempt_fingerprints.len();
        if n >= 3 {
            let a = &t.attempt_fingerprints[n - 1];
            let b = &t.attempt_fingerprints[n - 2];
            let c = &t.attempt_fingerprints[n - 3];
            if a == b && b == c {
                return Err(HarnessError::LoopGuard(format!(
                    "same approach fingerprint repeated 3 times for task {task_id} — change strategy or escalate"
                )));
            }
        }
        Ok(())
    }

    pub fn evaluator_mark_pass(
        &mut self,
        task_id: &str,
        payload: VerificationPayload,
    ) -> Result<()> {
        let idx = self.task_index(task_id)?;
        let task = &self.tasks[idx];
        if task.status == TaskPassState::Pass {
            return Err(HarnessError::AlreadyPass(task_id.to_string()));
        }
        if task.acceptance_criteria.is_empty() {
            return Err(HarnessError::VerificationRejected(
                "task has no acceptance_criteria — planner must define them before pass".into(),
            ));
        }
        if payload.commands_executed.is_empty() {
            return Err(HarnessError::VerificationRejected(
                "commands_executed must list real commands you ran (tests, build, etc.)".into(),
            ));
        }
        if payload.acceptance_results.len() != task.acceptance_criteria.len() {
            return Err(HarnessError::VerificationRejected(format!(
                "expected {} acceptance_results (one per criterion), got {}",
                task.acceptance_criteria.len(),
                payload.acceptance_results.len()
            )));
        }
        for (i, crit) in task.acceptance_criteria.iter().enumerate() {
            let r = &payload.acceptance_results[i];
            if r.criterion.trim() != crit.trim() {
                return Err(HarnessError::VerificationRejected(format!(
                    "acceptance_results[{i}].criterion must exactly match tasks.json criterion {:?}, got {:?}",
                    crit, r.criterion
                )));
            }
            if !r.passed {
                return Err(HarnessError::VerificationRejected(format!(
                    "criterion {:?} not passed",
                    crit
                )));
            }
            if r.evidence.trim().len() < 24 {
                return Err(HarnessError::VerificationRejected(format!(
                    "criterion {:?}: evidence too short (min 24 chars) — cite concrete command output or file:line",
                    crit
                )));
            }
        }
        let recorded = chrono::Utc::now().to_rfc3339();
        let verification = StoredVerification {
            recorded_at_rfc3339: recorded.clone(),
            commands_executed: payload.commands_executed,
            acceptance_results: payload.acceptance_results,
            reviewer_summary: payload.reviewer_summary,
        };
        self.tasks[idx].status = TaskPassState::Pass;
        self.tasks[idx].verification = Some(verification);
        self.tasks[idx].attempt_fingerprints.clear();
        if self.meta.current_focus_task_id.as_deref() == Some(task_id) {
            self.meta.current_focus_task_id = None;
        }
        Ok(())
    }

    fn task_index(&self, task_id: &str) -> Result<usize> {
        self.tasks
            .iter()
            .position(|t| t.id == task_id)
            .ok_or_else(|| HarnessError::UnknownTaskId(task_id.to_string()))
    }

    fn require_fail_task(&self, task_id: &str) -> Result<()> {
        let idx = self.task_index(task_id)?;
        if self.tasks[idx].status != TaskPassState::Fail {
            return Err(HarnessError::Protocol(format!(
                "task {task_id} is not in fail state"
            )));
        }
        Ok(())
    }

    /// Next recommended fail task: first fail in `task_order`, by priority order.
    pub fn next_fail_task_id(&self) -> Option<&str> {
        for id in &self.task_order {
            if let Some(t) = self.tasks.iter().find(|x| x.id == *id) {
                if t.status == TaskPassState::Fail {
                    return Some(t.id.as_str());
                }
            }
        }
        None
    }
}
