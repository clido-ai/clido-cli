//! TaskGraph: DAG of tasks with dependency validation.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub type TaskId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Complexity {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: TaskId,
    pub description: String,
    /// IDs of tasks that must complete before this task.
    pub depends_on: Vec<TaskId>,
    /// Optional tool allowlist for this task's sub-agent.
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub complexity: Complexity,
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanMeta {
    pub id: String,
    pub goal: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub meta: PlanMeta,
    pub tasks: Vec<TaskNode>,
}

impl Plan {
    pub fn task_graph(&self) -> TaskGraph {
        TaskGraph {
            goal: self.meta.goal.clone(),
            tasks: self.tasks.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraph {
    pub tasks: Vec<TaskNode>,
    pub goal: String,
}

impl TaskGraph {
    /// Validate the graph: no duplicate IDs, no missing dependencies, no cycles.
    pub fn validate(&self) -> Result<(), GraphError> {
        let ids: HashSet<&str> = self.tasks.iter().map(|t| t.id.as_str()).collect();
        // Check uniqueness
        if ids.len() != self.tasks.len() {
            return Err(GraphError::DuplicateTaskId);
        }
        // Check all deps exist
        for task in &self.tasks {
            for dep in &task.depends_on {
                if !ids.contains(dep.as_str()) {
                    return Err(GraphError::UnknownDependency {
                        task: task.id.clone(),
                        dep: dep.clone(),
                    });
                }
            }
        }
        // Check for cycles (topological sort)
        let order = self.topological_order()?;
        if order.len() != self.tasks.len() {
            return Err(GraphError::Cycle);
        }
        Ok(())
    }

    /// Return tasks in topological order (dependencies first).
    pub fn topological_order(&self) -> Result<Vec<&TaskNode>, GraphError> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();

        // Initialize in-degree for every task to 0.
        for task in &self.tasks {
            in_degree.entry(task.id.as_str()).or_insert(0);
            graph.entry(task.id.as_str()).or_default();
        }

        for task in &self.tasks {
            for dep in &task.depends_on {
                *in_degree.entry(task.id.as_str()).or_insert(0) += 1;
                graph
                    .entry(dep.as_str())
                    .or_default()
                    .push(task.id.as_str());
            }
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&id, _)| id)
            .collect();
        // Sort for deterministic ordering in tests.
        queue.sort_unstable();

        let mut result = Vec::new();
        while let Some(id) = queue.pop() {
            let node = self.tasks.iter().find(|t| t.id == id).unwrap();
            result.push(node);
            if let Some(dependents) = graph.get(id) {
                let mut next: Vec<&str> = Vec::new();
                for &dep in dependents {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next.push(dep);
                    }
                }
                next.sort_unstable();
                queue.extend(next);
            }
        }

        if result.len() != self.tasks.len() {
            return Err(GraphError::Cycle);
        }
        Ok(result)
    }

    /// Return batches of tasks that can run in parallel (each batch's tasks have all deps satisfied).
    pub fn parallel_batches(&self) -> Result<Vec<Vec<&TaskNode>>, GraphError> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();

        for task in &self.tasks {
            in_degree.entry(task.id.as_str()).or_insert(0);
            graph.entry(task.id.as_str()).or_default();
        }

        for task in &self.tasks {
            for dep in &task.depends_on {
                *in_degree.entry(task.id.as_str()).or_insert(0) += 1;
                graph
                    .entry(dep.as_str())
                    .or_default()
                    .push(task.id.as_str());
            }
        }

        // Validate first (catches cycles)
        self.validate()?;

        let mut remaining_degree = in_degree.clone();
        let mut batches: Vec<Vec<&TaskNode>> = Vec::new();
        let mut completed: HashSet<&str> = HashSet::new();

        loop {
            let mut batch_ids: Vec<&str> = remaining_degree
                .iter()
                .filter(|(id, &d)| d == 0 && !completed.contains(*id))
                .map(|(&id, _)| id)
                .collect();
            if batch_ids.is_empty() {
                break;
            }
            batch_ids.sort_unstable();
            let batch: Vec<&TaskNode> = batch_ids
                .iter()
                .map(|&id| self.tasks.iter().find(|t| t.id == id).unwrap())
                .collect();

            for &id in &batch_ids {
                completed.insert(id);
                if let Some(dependents) = graph.get(id) {
                    for &dep in dependents {
                        if let Some(d) = remaining_degree.get_mut(dep) {
                            *d = d.saturating_sub(1);
                        }
                    }
                }
            }

            batches.push(batch);
        }

        Ok(batches)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("duplicate task ID in graph")]
    DuplicateTaskId,
    #[error("task {task} depends on unknown task {dep}")]
    UnknownDependency { task: String, dep: String },
    #[error("task graph contains a cycle")]
    Cycle,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, depends_on: Vec<&str>) -> TaskNode {
        TaskNode {
            id: id.to_string(),
            description: format!("Task {}", id),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            tools: None,
            complexity: Complexity::default(),
            skip: false,
            notes: String::new(),
            status: TaskStatus::default(),
        }
    }

    fn linear_graph() -> TaskGraph {
        TaskGraph {
            goal: "test".to_string(),
            tasks: vec![
                make_node("t1", vec![]),
                make_node("t2", vec!["t1"]),
                make_node("t3", vec!["t2"]),
            ],
        }
    }

    #[test]
    fn validate_valid_dag() {
        let g = linear_graph();
        assert!(g.validate().is_ok());
    }

    #[test]
    fn validate_cycle_detected() {
        let g = TaskGraph {
            goal: "test".to_string(),
            tasks: vec![make_node("a", vec!["b"]), make_node("b", vec!["a"])],
        };
        assert!(matches!(g.validate(), Err(GraphError::Cycle)));
    }

    #[test]
    fn validate_unknown_dep() {
        let g = TaskGraph {
            goal: "test".to_string(),
            tasks: vec![make_node("a", vec!["nonexistent"])],
        };
        assert!(matches!(
            g.validate(),
            Err(GraphError::UnknownDependency { .. })
        ));
    }

    #[test]
    fn validate_duplicate_id() {
        let g = TaskGraph {
            goal: "test".to_string(),
            tasks: vec![make_node("a", vec![]), make_node("a", vec![])],
        };
        assert!(matches!(g.validate(), Err(GraphError::DuplicateTaskId)));
    }

    #[test]
    fn topological_order_respects_dependencies() {
        let g = linear_graph();
        let order = g.topological_order().unwrap();
        let ids: Vec<&str> = order.iter().map(|n| n.id.as_str()).collect();
        let t1_pos = ids.iter().position(|&x| x == "t1").unwrap();
        let t2_pos = ids.iter().position(|&x| x == "t2").unwrap();
        let t3_pos = ids.iter().position(|&x| x == "t3").unwrap();
        assert!(t1_pos < t2_pos);
        assert!(t2_pos < t3_pos);
    }

    #[test]
    fn parallel_batches_independent_tasks_same_batch() {
        let g = TaskGraph {
            goal: "test".to_string(),
            tasks: vec![
                make_node("t1", vec![]),
                make_node("t2", vec![]),
                make_node("t3", vec!["t1", "t2"]),
            ],
        };
        let batches = g.parallel_batches().unwrap();
        assert_eq!(batches.len(), 2);
        // First batch should contain t1 and t2 (order may vary)
        let first_ids: Vec<&str> = batches[0].iter().map(|n| n.id.as_str()).collect();
        assert!(first_ids.contains(&"t1"));
        assert!(first_ids.contains(&"t2"));
        // Second batch should contain t3
        let second_ids: Vec<&str> = batches[1].iter().map(|n| n.id.as_str()).collect();
        assert_eq!(second_ids, vec!["t3"]);
    }

    #[test]
    fn parallel_batches_linear_chain() {
        let g = linear_graph();
        let batches = g.parallel_batches().unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0][0].id, "t1");
        assert_eq!(batches[1][0].id, "t2");
        assert_eq!(batches[2][0].id, "t3");
    }

    #[test]
    fn empty_graph_is_valid() {
        let g = TaskGraph {
            goal: "empty".to_string(),
            tasks: vec![],
        };
        assert!(g.validate().is_ok());
        assert!(g.parallel_batches().unwrap().is_empty());
    }

    #[test]
    fn task_node_new_fields_have_defaults() {
        let json = r#"{"id":"t1","description":"test","depends_on":[]}"#;
        let node: TaskNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.complexity, Complexity::Low);
        assert!(!node.skip);
        assert_eq!(node.notes, "");
        assert_eq!(node.status, TaskStatus::Pending);
    }

    #[test]
    fn plan_task_graph_returns_correct_graph() {
        let plan = Plan {
            meta: PlanMeta {
                id: "plan-abc".to_string(),
                goal: "build something".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            },
            tasks: vec![make_node("t1", vec![])],
        };
        let graph = plan.task_graph();
        assert_eq!(graph.goal, "build something");
        assert_eq!(graph.tasks.len(), 1);
    }
}
