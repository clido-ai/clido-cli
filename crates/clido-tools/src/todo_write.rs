//! TodoWrite tool: agent-managed structured task list for session-level work tracking.
//!
//! The agent can write a complete todo list (replacing previous one) so both
//! agent and user can track progress on multi-step tasks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Tool, ToolOutput};

/// Priority of a todo item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl std::fmt::Display for TodoPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// Status of a todo item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Done => write!(f, "done"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

/// A single todo item managed by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

/// Tool that lets the agent write/update a structured todo list for the session.
pub struct TodoWriteTool {
    /// Shared todo list (Arc<Mutex> so TUI can read it too).
    store: std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>,
}

impl TodoWriteTool {
    pub fn new() -> Self {
        Self {
            store: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Create a tool instance backed by an existing shared store (for TUI sharing).
    pub fn with_store(store: std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>>) -> Self {
        Self { store }
    }

    /// Get a handle to the shared store for TUI display.
    pub fn store(&self) -> std::sync::Arc<std::sync::Mutex<Vec<TodoItem>>> {
        self.store.clone()
    }
}

impl Default for TodoWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self) -> &str {
        "Write a structured todo list for the current session. Replaces the entire list. \
         Use to track multi-step tasks so the user can see progress. \
         Items should have a unique id, content description, status (pending/in_progress/done/blocked), \
         and priority (high/medium/low)."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Complete list of todo items (replaces previous list)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id":       { "type": "string", "description": "Unique short identifier, e.g. 'setup-db'" },
                            "content":  { "type": "string", "description": "Task description" },
                            "status":   { "type": "string", "enum": ["pending","in_progress","done","blocked"] },
                            "priority": { "type": "string", "enum": ["high","medium","low"] }
                        },
                        "required": ["id","content","status","priority"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let items_raw = match input.get("todos").and_then(|v| v.as_array()) {
            Some(arr) => arr.clone(),
            None => return ToolOutput::err("Missing required field: todos"),
        };

        let mut items: Vec<TodoItem> = Vec::with_capacity(items_raw.len());
        for (i, item) in items_raw.iter().enumerate() {
            let id = match item.get("id").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => return ToolOutput::err(format!("Item {i}: missing or empty 'id'")),
            };
            let content = match item.get("content").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => return ToolOutput::err(format!("Item {i}: missing or empty 'content'")),
            };
            let status = match item.get("status").and_then(|v| v.as_str()) {
                Some("pending") => TodoStatus::Pending,
                Some("in_progress") => TodoStatus::InProgress,
                Some("done") => TodoStatus::Done,
                Some("blocked") => TodoStatus::Blocked,
                Some(other) => {
                    return ToolOutput::err(format!(
                    "Item {i}: invalid status '{other}'. Use: pending, in_progress, done, blocked"
                ))
                }
                None => return ToolOutput::err(format!("Item {i}: missing 'status'")),
            };
            let priority = match item.get("priority").and_then(|v| v.as_str()) {
                Some("high") => TodoPriority::High,
                Some("medium") => TodoPriority::Medium,
                Some("low") => TodoPriority::Low,
                Some(other) => {
                    return ToolOutput::err(format!(
                        "Item {i}: invalid priority '{other}'. Use: high, medium, low"
                    ))
                }
                None => return ToolOutput::err(format!("Item {i}: missing 'priority'")),
            };
            items.push(TodoItem {
                id,
                content,
                status,
                priority,
            });
        }

        let count = items.len();
        match self.store.lock() {
            Ok(mut guard) => {
                *guard = items;
            }
            Err(_) => return ToolOutput::err("Internal error: todo store lock poisoned"),
        }

        ToolOutput::ok(format!(
            "Todo list updated: {count} item(s). User can see the list with /todo."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool() -> TodoWriteTool {
        TodoWriteTool::new()
    }

    #[tokio::test]
    async fn write_valid_todos() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "todos": [
                    {"id": "t1", "content": "Write tests", "status": "pending", "priority": "high"},
                    {"id": "t2", "content": "Deploy", "status": "in_progress", "priority": "medium"}
                ]
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("2 item"));
        let store = tool.store.lock().unwrap();
        assert_eq!(store.len(), 2);
        assert_eq!(store[0].id, "t1");
        assert_eq!(store[1].status, TodoStatus::InProgress);
    }

    #[tokio::test]
    async fn write_replaces_previous_list() {
        let tool = make_tool();
        tool.execute(
            json!({"todos": [{"id":"a","content":"old","status":"pending","priority":"low"}]}),
        )
        .await;
        let result = tool.execute(json!({"todos": []})).await;
        assert!(!result.is_error);
        let store = tool.store.lock().unwrap();
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn invalid_status_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "todos": [{"id":"x","content":"foo","status":"unknown","priority":"high"}]
            }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("invalid status"));
    }

    #[tokio::test]
    async fn missing_id_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(json!({"todos": [{"content":"foo","status":"done","priority":"low"}]}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("'id'"));
    }

    #[tokio::test]
    async fn missing_todos_field_returns_error() {
        let tool = make_tool();
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
    }
}
