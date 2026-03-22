//! Pure in-memory plan editing operations.

use crate::graph::{Complexity, GraphError, Plan, TaskNode, TaskStatus};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlanEditError {
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("cannot delete task with dependents: {dep} needs {task}")]
    HasDependents { task: String, dep: String },
    #[error("graph error after edit: {0}")]
    InvalidGraph(#[from] GraphError),
    #[error("index out of range")]
    IndexOutOfRange,
}

pub struct PlanEditor {
    pub plan: Plan,
}

impl PlanEditor {
    pub fn new(plan: Plan) -> Self {
        Self { plan }
    }

    /// Rename a task's description.
    pub fn rename_task(
        &mut self,
        task_id: &str,
        new_description: &str,
    ) -> Result<(), PlanEditError> {
        let task = self
            .plan
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| PlanEditError::TaskNotFound(task_id.to_string()))?;
        task.description = new_description.to_string();
        self.validate()?;
        Ok(())
    }

    /// Update a task's notes.
    pub fn set_notes(&mut self, task_id: &str, notes: &str) -> Result<(), PlanEditError> {
        let task = self
            .plan
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| PlanEditError::TaskNotFound(task_id.to_string()))?;
        task.notes = notes.to_string();
        self.validate()?;
        Ok(())
    }

    /// Update a task's complexity.
    pub fn set_complexity(
        &mut self,
        task_id: &str,
        complexity: Complexity,
    ) -> Result<(), PlanEditError> {
        let task = self
            .plan
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| PlanEditError::TaskNotFound(task_id.to_string()))?;
        task.complexity = complexity;
        self.validate()?;
        Ok(())
    }

    /// Toggle skip on a task. Skipped tasks get status = Skipped.
    pub fn toggle_skip(&mut self, task_id: &str) -> Result<(), PlanEditError> {
        let task = self
            .plan
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| PlanEditError::TaskNotFound(task_id.to_string()))?;
        task.skip = !task.skip;
        if task.skip {
            task.status = TaskStatus::Skipped;
        } else {
            task.status = TaskStatus::Pending;
        }
        self.validate()?;
        Ok(())
    }

    /// Delete a task. Fails if any other task depends on it.
    pub fn delete_task(&mut self, task_id: &str) -> Result<(), PlanEditError> {
        // Check that the task exists.
        if !self.plan.tasks.iter().any(|t| t.id == task_id) {
            return Err(PlanEditError::TaskNotFound(task_id.to_string()));
        }
        // Check no other task depends on it.
        for task in &self.plan.tasks {
            if task.id != task_id {
                for dep in &task.depends_on {
                    if dep == task_id {
                        return Err(PlanEditError::HasDependents {
                            task: task_id.to_string(),
                            dep: task.id.clone(),
                        });
                    }
                }
            }
        }
        self.plan.tasks.retain(|t| t.id != task_id);
        self.validate()?;
        Ok(())
    }

    /// Add a new task at the end. Validates after.
    pub fn add_task(
        &mut self,
        id: String,
        description: String,
        depends_on: Vec<String>,
    ) -> Result<(), PlanEditError> {
        let task_id = if id.is_empty() {
            format!("t{}", self.plan.tasks.len() + 1)
        } else {
            id
        };
        let task = TaskNode {
            id: task_id,
            description,
            depends_on,
            tools: None,
            complexity: Complexity::default(),
            skip: false,
            notes: String::new(),
            status: TaskStatus::default(),
        };
        self.plan.tasks.push(task);
        self.validate()?;
        Ok(())
    }

    /// Move a task up by one position in the tasks vec (not dependency order).
    pub fn move_up(&mut self, index: usize) -> Result<(), PlanEditError> {
        if index == 0 || index >= self.plan.tasks.len() {
            return Err(PlanEditError::IndexOutOfRange);
        }
        self.plan.tasks.swap(index, index - 1);
        self.validate()?;
        Ok(())
    }

    /// Move a task down by one position in the tasks vec.
    pub fn move_down(&mut self, index: usize) -> Result<(), PlanEditError> {
        if index + 1 >= self.plan.tasks.len() {
            return Err(PlanEditError::IndexOutOfRange);
        }
        self.plan.tasks.swap(index, index + 1);
        self.validate()?;
        Ok(())
    }

    /// Set task status.
    pub fn set_status(&mut self, task_id: &str, status: TaskStatus) -> Result<(), PlanEditError> {
        let task = self
            .plan
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| PlanEditError::TaskNotFound(task_id.to_string()))?;
        task.status = status;
        self.validate()?;
        Ok(())
    }

    /// Validate the current plan's task graph.
    fn validate(&self) -> Result<(), GraphError> {
        self.plan.task_graph().validate()
    }
}

#[cfg(test)]
fn make_test_plan() -> Plan {
    use crate::graph::{Complexity, PlanMeta, TaskStatus};
    Plan {
        meta: PlanMeta {
            id: "plan-test".to_string(),
            goal: "test goal".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        },
        tasks: vec![
            TaskNode {
                id: "t1".to_string(),
                description: "first task".to_string(),
                depends_on: vec![],
                tools: None,
                complexity: Complexity::default(),
                skip: false,
                notes: String::new(),
                status: TaskStatus::default(),
            },
            TaskNode {
                id: "t2".to_string(),
                description: "second task".to_string(),
                depends_on: vec!["t1".to_string()],
                tools: None,
                complexity: Complexity::default(),
                skip: false,
                notes: String::new(),
                status: TaskStatus::default(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rename_task() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor.rename_task("t1", "renamed task").unwrap();
        assert_eq!(editor.plan.tasks[0].description, "renamed task");
    }

    #[test]
    fn test_rename_task_not_found() {
        let mut editor = PlanEditor::new(make_test_plan());
        let result = editor.rename_task("nonexistent", "new name");
        assert!(matches!(result, Err(PlanEditError::TaskNotFound(_))));
    }

    #[test]
    fn test_set_notes() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor.set_notes("t1", "some notes here").unwrap();
        assert_eq!(editor.plan.tasks[0].notes, "some notes here");
    }

    #[test]
    fn test_set_complexity() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor.set_complexity("t1", Complexity::High).unwrap();
        assert_eq!(editor.plan.tasks[0].complexity, Complexity::High);
    }

    #[test]
    fn test_toggle_skip() {
        let mut editor = PlanEditor::new(make_test_plan());
        // Toggle on
        editor.toggle_skip("t1").unwrap();
        assert!(editor.plan.tasks[0].skip);
        assert_eq!(editor.plan.tasks[0].status, TaskStatus::Skipped);
        // Toggle off
        editor.toggle_skip("t1").unwrap();
        assert!(!editor.plan.tasks[0].skip);
        assert_eq!(editor.plan.tasks[0].status, TaskStatus::Pending);
    }

    #[test]
    fn test_delete_task() {
        let mut editor = PlanEditor::new(make_test_plan());
        // t2 depends on t1, so delete t2 first
        editor.delete_task("t2").unwrap();
        assert_eq!(editor.plan.tasks.len(), 1);
        assert_eq!(editor.plan.tasks[0].id, "t1");
    }

    #[test]
    fn test_delete_task_with_dependents_fails() {
        let mut editor = PlanEditor::new(make_test_plan());
        // t1 has t2 depending on it
        let result = editor.delete_task("t1");
        assert!(matches!(result, Err(PlanEditError::HasDependents { .. })));
    }

    #[test]
    fn test_add_task() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor
            .add_task(
                "t3".to_string(),
                "third task".to_string(),
                vec!["t2".to_string()],
            )
            .unwrap();
        assert_eq!(editor.plan.tasks.len(), 3);
        assert_eq!(editor.plan.tasks[2].id, "t3");
    }

    #[test]
    fn test_add_task_auto_id() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor
            .add_task(String::new(), "auto id task".to_string(), vec![])
            .unwrap();
        assert_eq!(editor.plan.tasks[2].id, "t3");
    }

    #[test]
    fn test_move_up() {
        let mut editor = PlanEditor::new(make_test_plan());
        // Add a third independent task to move around freely
        editor
            .add_task("t3".to_string(), "third task".to_string(), vec![])
            .unwrap();
        // Move t3 (index 2) up to index 1
        editor.move_up(2).unwrap();
        assert_eq!(editor.plan.tasks[1].id, "t3");
        assert_eq!(editor.plan.tasks[2].id, "t2");
    }

    #[test]
    fn test_move_up_index_zero_fails() {
        let mut editor = PlanEditor::new(make_test_plan());
        let result = editor.move_up(0);
        assert!(matches!(result, Err(PlanEditError::IndexOutOfRange)));
    }

    #[test]
    fn test_move_down() {
        let mut editor = PlanEditor::new(make_test_plan());
        // Add a third task at end, independent
        editor
            .add_task("t3".to_string(), "third task".to_string(), vec![])
            .unwrap();
        // Move t1 (index 0) down — but t2 depends on t1, graph should still be valid
        editor.move_down(0).unwrap();
        assert_eq!(editor.plan.tasks[0].id, "t2");
        assert_eq!(editor.plan.tasks[1].id, "t1");
    }

    #[test]
    fn test_move_down_last_index_fails() {
        let mut editor = PlanEditor::new(make_test_plan());
        let last = editor.plan.tasks.len() - 1;
        let result = editor.move_down(last);
        assert!(matches!(result, Err(PlanEditError::IndexOutOfRange)));
    }

    #[test]
    fn test_set_status() {
        let mut editor = PlanEditor::new(make_test_plan());
        editor.set_status("t1", TaskStatus::Done).unwrap();
        assert_eq!(editor.plan.tasks[0].status, TaskStatus::Done);
    }
}
