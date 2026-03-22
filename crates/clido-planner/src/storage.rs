//! Plan persistence: save/load/list/delete plans in `.clido/plans/`.

use std::path::{Path, PathBuf};

use crate::graph::{Plan, TaskStatus};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plan not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct PlanSummary {
    pub id: String,
    pub goal: String,
    pub task_count: usize,
    pub created_at: String,
    pub pending: usize,
    pub done: usize,
    pub failed: usize,
}

fn plans_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".clido").join("plans")
}

/// Save a plan to `<workspace>/.clido/plans/<id>.json`. Creates the directory if needed.
pub fn save_plan(workspace_root: &Path, plan: &Plan) -> Result<PathBuf, StorageError> {
    let dir = plans_dir(workspace_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", plan.meta.id));
    let json = serde_json::to_string_pretty(plan)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Load a plan by ID from `<workspace>/.clido/plans/<id>.json`.
pub fn load_plan(workspace_root: &Path, id: &str) -> Result<Plan, StorageError> {
    let path = plans_dir(workspace_root).join(format!("{}.json", id));
    if !path.exists() {
        return Err(StorageError::NotFound(id.to_string()));
    }
    let contents = std::fs::read_to_string(&path)?;
    let plan: Plan = serde_json::from_str(&contents)?;
    Ok(plan)
}

/// List all plans in `<workspace>/.clido/plans/`, sorted by `created_at` descending.
pub fn list_plans(workspace_root: &Path) -> Result<Vec<PlanSummary>, StorageError> {
    let dir = plans_dir(workspace_root);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut summaries: Vec<PlanSummary> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)?;
        let plan: Plan = match serde_json::from_str(&contents) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let pending = plan
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let done = plan
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count();
        let failed = plan
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Failed)
            .count();
        summaries.push(PlanSummary {
            id: plan.meta.id.clone(),
            goal: plan.meta.goal.clone(),
            task_count: plan.tasks.len(),
            created_at: plan.meta.created_at.clone(),
            pending,
            done,
            failed,
        });
    }
    // Sort by created_at descending
    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(summaries)
}

/// Delete a plan by ID.
pub fn delete_plan(workspace_root: &Path, id: &str) -> Result<(), StorageError> {
    let path = plans_dir(workspace_root).join(format!("{}.json", id));
    if !path.exists() {
        return Err(StorageError::NotFound(id.to_string()));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Complexity, PlanMeta, TaskNode, TaskStatus};

    fn make_plan(id: &str, goal: &str, created_at: &str) -> Plan {
        Plan {
            meta: PlanMeta {
                id: id.to_string(),
                goal: goal.to_string(),
                created_at: created_at.to_string(),
            },
            tasks: vec![
                TaskNode {
                    id: "t1".to_string(),
                    description: "task one".to_string(),
                    depends_on: vec![],
                    tools: None,
                    complexity: Complexity::default(),
                    skip: false,
                    notes: String::new(),
                    status: TaskStatus::default(),
                },
                TaskNode {
                    id: "t2".to_string(),
                    description: "task two".to_string(),
                    depends_on: vec!["t1".to_string()],
                    tools: None,
                    complexity: Complexity::default(),
                    skip: false,
                    notes: String::new(),
                    status: TaskStatus::Done,
                },
            ],
        }
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let plan = make_plan("plan-001", "build a widget", "2026-03-01T10:00:00Z");
        let path = save_plan(dir.path(), &plan).unwrap();
        assert!(path.exists());
        let loaded = load_plan(dir.path(), "plan-001").unwrap();
        assert_eq!(loaded.meta.id, "plan-001");
        assert_eq!(loaded.meta.goal, "build a widget");
        assert_eq!(loaded.tasks.len(), 2);
        assert_eq!(loaded.tasks[1].status, TaskStatus::Done);
    }

    #[test]
    fn test_list_plans() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = make_plan("plan-aaa", "goal aaa", "2026-01-01T00:00:00Z");
        let p2 = make_plan("plan-bbb", "goal bbb", "2026-03-01T00:00:00Z");
        save_plan(dir.path(), &p1).unwrap();
        save_plan(dir.path(), &p2).unwrap();
        let summaries = list_plans(dir.path()).unwrap();
        assert_eq!(summaries.len(), 2);
        // Sorted descending by created_at — plan-bbb first
        assert_eq!(summaries[0].id, "plan-bbb");
        assert_eq!(summaries[1].id, "plan-aaa");
        // Check counts
        let bbb = &summaries[0];
        assert_eq!(bbb.task_count, 2);
        assert_eq!(bbb.pending, 1);
        assert_eq!(bbb.done, 1);
        assert_eq!(bbb.failed, 0);
    }

    #[test]
    fn test_delete_plan() {
        let dir = tempfile::tempdir().unwrap();
        let plan = make_plan("plan-del", "delete me", "2026-01-01T00:00:00Z");
        save_plan(dir.path(), &plan).unwrap();
        delete_plan(dir.path(), "plan-del").unwrap();
        let summaries = list_plans(dir.path()).unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn test_load_nonexistent_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_plan(dir.path(), "plan-ghost");
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }
}
