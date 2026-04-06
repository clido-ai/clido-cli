//! `HarnessControl` — read/update `.clido/harness/tasks.json` and `progress.ndjson` with strict merge rules.

use async_trait::async_trait;
use serde_json::json;

use clido_harness::{
    append_progress, git_log_snippet, read_progress_tail, read_state, reconcile_order,
    touch_meta_timestamp, write_state, HarnessTask, TaskPassState, VerificationPayload,
};

use crate::{Tool, ToolOutput};

pub struct HarnessControlTool {
    workspace: std::path::PathBuf,
}

impl HarnessControlTool {
    pub fn new(workspace: std::path::PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for HarnessControlTool {
    fn name(&self) -> &str {
        "HarnessControl"
    }

    fn description(&self) -> &str {
        "Structured harness: durable JSON tasks under `.clido/harness/tasks.json` and append-only \
         `progress.ndjson`. Task order is never reordered or deleted — only status fail→pass after \
         verification. Operations: read | planner_append_tasks | executor_set_focus | executor_clear_focus | \
         executor_register_attempt | progress_append | evaluator_mark_pass. \
         Planner appends new tasks (all start fail). Executor sets focus to ONE fail task, implements it, \
         runs real tests/commands, then SpawnReviewer (or human) must verify. Evaluator (reviewer sub-agent \
         when harness is on) calls evaluator_mark_pass with commands_executed + per-criterion evidence. \
         Do not mark pass without running checks listed in acceptance_criteria."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": [
                        "read",
                        "planner_append_tasks",
                        "executor_set_focus",
                        "executor_clear_focus",
                        "executor_register_attempt",
                        "progress_append",
                        "evaluator_mark_pass"
                    ],
                    "description": "read = snapshot state + progress tail + git log; planner_append_tasks = add tasks; executor_* = session focus / loop guard; progress_append = append NDJSON event; evaluator_mark_pass = verified pass only"
                },
                "tasks": {
                    "type": "array",
                    "description": "For planner_append_tasks: new tasks only. Each: id, description, steps[], acceptance_criteria[] (all start as fail).",
                    "items": { "type": "object" }
                },
                "task_id": { "type": "string", "description": "Target task id for focus / attempt / mark_pass" },
                "fingerprint": { "type": "string", "description": "Short stable label for executor_register_attempt (e.g. edit-strategy-v2)" },
                "progress": { "type": "object", "description": "For progress_append: JSON object (merged with ts, op)." },
                "verification": {
                    "type": "object",
                    "description": "For evaluator_mark_pass: { commands_executed: string[], acceptance_results: [{criterion, passed, evidence}], reviewer_summary?: string }",
                    "properties": {
                        "commands_executed": { "type": "array", "items": { "type": "string" } },
                        "acceptance_results": { "type": "array" },
                        "reviewer_summary": { "type": "string" }
                    }
                }
            },
            "required": ["op"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let op = match input.get("op").and_then(|v| v.as_str()) {
            Some(o) => o,
            None => return ToolOutput::err("HarnessControl: missing op"),
        };

        let mut state = match read_state(&self.workspace) {
            Ok(s) => s,
            Err(e) => return ToolOutput::err(format!("harness read: {e}")),
        };
        reconcile_order(&mut state);

        match op {
            "read" => {
                let progress = match read_progress_tail(&self.workspace, 60) {
                    Ok(p) => p,
                    Err(e) => return ToolOutput::err(format!("progress read: {e}")),
                };
                let git = git_log_snippet(&self.workspace, 25);
                let next = state
                    .next_fail_task_id()
                    .unwrap_or("(none — all pass or empty)");
                let focus = state
                    .meta
                    .current_focus_task_id
                    .clone()
                    .unwrap_or_else(|| "(none)".to_string());
                let body = json!({
                    "tasks_path": clido_harness::tasks_path(&self.workspace).display().to_string(),
                    "state": state,
                    "progress_tail": progress,
                    "git_log_oneline": git,
                    "hint_next_fail_task": next,
                    "current_focus": focus,
                });
                ToolOutput::ok(serde_json::to_string_pretty(&body).unwrap_or_default())
            }
            "planner_append_tasks" => {
                let arr = match input.get("tasks").and_then(|v| v.as_array()) {
                    Some(a) if !a.is_empty() => a,
                    _ => {
                        return ToolOutput::err(
                            "planner_append_tasks requires non-empty tasks array",
                        )
                    }
                };
                let appended_count = arr.len();
                let mut new_tasks: Vec<HarnessTask> = Vec::new();
                for (i, v) in arr.iter().enumerate() {
                    let id = match v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        Some(s) => s.to_string(),
                        None => return ToolOutput::err(format!("tasks[{i}]: id required")),
                    };
                    let description = match v
                        .get("description")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        Some(s) => s.to_string(),
                        None => {
                            return ToolOutput::err(format!("tasks[{i}]: description required"))
                        }
                    };
                    let steps: Vec<String> = v
                        .get("steps")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let acceptance_criteria: Vec<String> = v
                        .get("acceptance_criteria")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    if acceptance_criteria.is_empty() {
                        return ToolOutput::err(format!(
                            "tasks[{i}]: acceptance_criteria must be non-empty (explicit verification)"
                        ));
                    }
                    new_tasks.push(HarnessTask {
                        id,
                        description,
                        steps,
                        acceptance_criteria,
                        status: TaskPassState::Fail,
                        verification: None,
                        attempt_fingerprints: Vec::new(),
                    });
                }
                if let Err(e) = state.planner_append_tasks(new_tasks) {
                    return ToolOutput::err(e.to_string());
                }
                touch_meta_timestamp(&mut state);
                if let Err(e) = write_state(&self.workspace, &state) {
                    return ToolOutput::err(e.to_string());
                }
                let _ = append_progress(
                    &self.workspace,
                    &json!({
                        "ts": chrono::Utc::now().to_rfc3339(),
                        "kind": "planner_append",
                        "task_count": state.tasks.len(),
                    }),
                );
                ToolOutput::ok(format!(
                    "Appended {} task(s). Total tasks: {}.",
                    appended_count,
                    state.tasks.len()
                ))
            }
            "executor_set_focus" => {
                let tid = match input.get("task_id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s,
                    Some(_) => {
                        return ToolOutput::err("executor_set_focus requires non-empty task_id")
                    }
                    None => return ToolOutput::err("executor_set_focus requires task_id"),
                };
                if let Err(e) = state.executor_set_focus(tid) {
                    return ToolOutput::err(e.to_string());
                }
                touch_meta_timestamp(&mut state);
                if let Err(e) = write_state(&self.workspace, &state) {
                    return ToolOutput::err(e.to_string());
                }
                let _ = append_progress(
                    &self.workspace,
                    &json!({
                        "ts": chrono::Utc::now().to_rfc3339(),
                        "kind": "executor_set_focus",
                        "task_id": tid,
                    }),
                );
                ToolOutput::ok(format!(
                    "Focus set to task {tid}. Work only this task until verified."
                ))
            }
            "executor_clear_focus" => {
                state.executor_clear_focus();
                touch_meta_timestamp(&mut state);
                if let Err(e) = write_state(&self.workspace, &state) {
                    return ToolOutput::err(e.to_string());
                }
                ToolOutput::ok("Focus cleared.".to_string())
            }
            "executor_register_attempt" => {
                let tid = match input.get("task_id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s,
                    Some(_) => {
                        return ToolOutput::err(
                            "executor_register_attempt requires non-empty task_id",
                        )
                    }
                    None => return ToolOutput::err("executor_register_attempt requires task_id"),
                };
                let fp = match input.get("fingerprint").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s,
                    Some(_) => {
                        return ToolOutput::err(
                            "executor_register_attempt requires non-empty fingerprint",
                        )
                    }
                    None => {
                        return ToolOutput::err("executor_register_attempt requires fingerprint")
                    }
                };
                if let Err(e) = state.executor_register_attempt(tid, fp) {
                    return ToolOutput::err(e.to_string());
                }
                touch_meta_timestamp(&mut state);
                if let Err(e) = write_state(&self.workspace, &state) {
                    return ToolOutput::err(e.to_string());
                }
                ToolOutput::ok("Attempt recorded.".to_string())
            }
            "progress_append" => {
                let mut obj = match input.get("progress").cloned() {
                    Some(serde_json::Value::Object(m)) => serde_json::Value::Object(m),
                    _ => return ToolOutput::err("progress_append requires progress object"),
                };
                if let serde_json::Value::Object(ref mut m) = obj {
                    m.insert(
                        "ts".to_string(),
                        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
                    );
                    m.insert(
                        "harness_op".to_string(),
                        serde_json::Value::String("user_event".into()),
                    );
                }
                if let Err(e) = append_progress(&self.workspace, &obj) {
                    return ToolOutput::err(e.to_string());
                }
                ToolOutput::ok("Progress line appended.".to_string())
            }
            "evaluator_mark_pass" => {
                let tid = match input.get("task_id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s,
                    Some(_) => {
                        return ToolOutput::err("evaluator_mark_pass requires non-empty task_id")
                    }
                    None => return ToolOutput::err("evaluator_mark_pass requires task_id"),
                };
                let ver = match input.get("verification") {
                    Some(v) => v,
                    None => {
                        return ToolOutput::err("evaluator_mark_pass requires verification object")
                    }
                };
                let payload: VerificationPayload = match serde_json::from_value(ver.clone()) {
                    Ok(p) => p,
                    Err(e) => return ToolOutput::err(format!("verification JSON: {e}")),
                };
                if let Err(e) = state.evaluator_mark_pass(tid, payload) {
                    return ToolOutput::err(e.to_string());
                }
                touch_meta_timestamp(&mut state);
                if let Err(e) = write_state(&self.workspace, &state) {
                    return ToolOutput::err(e.to_string());
                }
                let _ = append_progress(
                    &self.workspace,
                    &json!({
                        "ts": chrono::Utc::now().to_rfc3339(),
                        "kind": "evaluator_mark_pass",
                        "task_id": tid,
                    }),
                );
                ToolOutput::ok(format!(
                    "Task {tid} marked PASS with recorded verification. Commit changes in git separately."
                ))
            }
            other => ToolOutput::err(format!("unknown op: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_harness::AcceptanceResult;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn harness_append_and_pass() {
        let dir = tempdir().unwrap();
        let tool = HarnessControlTool::new(dir.path().to_path_buf());
        let r = tool
            .execute(json!({
                "op": "planner_append_tasks",
                "tasks": [{
                    "id": "t1",
                    "description": "Add feature",
                    "steps": ["code", "test"],
                    "acceptance_criteria": ["tests pass", "no clippy warnings"]
                }]
            }))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let r2 = tool.execute(json!({ "op": "read" })).await;
        assert!(r2.content.contains("t1"));
        let ver = VerificationPayload {
            commands_executed: vec!["cargo test -p x".into()],
            acceptance_results: vec![
                AcceptanceResult {
                    criterion: "tests pass".into(),
                    passed: true,
                    evidence: "cargo test -p x: ok 12 passed".into(),
                },
                AcceptanceResult {
                    criterion: "no clippy warnings".into(),
                    passed: true,
                    evidence: "cargo clippy -p x -- -D warnings: exit 0".into(),
                },
            ],
            reviewer_summary: Some("LGTM".into()),
        };
        let r3 = tool
            .execute(json!({
                "op": "evaluator_mark_pass",
                "task_id": "t1",
                "verification": serde_json::to_value(&ver).unwrap()
            }))
            .await;
        assert!(!r3.is_error, "{}", r3.content);
    }
}
