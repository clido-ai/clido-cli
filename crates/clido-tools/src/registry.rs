//! Tool registry: register and lookup tools by name.

use clido_core::ToolSchema;
use std::collections::{HashMap, HashSet};

use crate::Tool;

/// Registry of named tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// Shared runtime-allowed Arc from PathGuard — lets callers grant path access in-flight.
    runtime_allowed: Option<std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            runtime_allowed: None,
        }
    }

    /// Store the shared runtime-allowed handle (obtained from `PathGuard::runtime_allowed_arc`).
    pub fn set_runtime_allowed(
        &mut self,
        arc: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
    ) {
        self.runtime_allowed = Some(arc);
    }

    /// Return a clone of the shared runtime-allowed Arc, if set.
    pub fn runtime_allowed_arc(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>> {
        self.runtime_allowed.clone()
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        let name = tool.name().to_string();
        self.tools.insert(name, Box::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| b.as_ref())
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .values()
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.schema(),
            })
            .collect()
    }

    /// Return schemas filtered for the current execution context.
    /// `in_plan_mode` controls whether ExitPlanMode is included (only useful in plan mode).
    pub fn schemas_for_context(&self, in_plan_mode: bool) -> Vec<ToolSchema> {
        self.tools
            .values()
            .filter(|t| {
                // ExitPlanMode is only relevant when actually in plan mode.
                if t.name() == "ExitPlanMode" {
                    return in_plan_mode;
                }
                true
            })
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.schema(),
            })
            .collect()
    }

    /// Apply allow/disallow lists. Disallowed takes precedence. Returns a new registry
    /// with only the allowed tools (or all if allowed is None, minus disallowed).
    pub fn with_filters(
        self,
        allowed: Option<Vec<String>>,
        disallowed: Option<Vec<String>>,
    ) -> Self {
        let disallowed_set: HashSet<String> = disallowed
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let allowed_set: Option<HashSet<String>> = allowed.map(|v| {
            v.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        let tools = self
            .tools
            .into_iter()
            .filter(|(name, _)| {
                if disallowed_set.contains(name) {
                    return false;
                }
                if let Some(ref a) = allowed_set {
                    if !a.contains(name) {
                        return false;
                    }
                }
                true
            })
            .collect();
        ToolRegistry {
            tools,
            runtime_allowed: self.runtime_allowed,
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolOutput};
    use async_trait::async_trait;

    struct FakeTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn is_read_only(&self) -> bool {
            true
        }
        async fn execute(&self, _input: serde_json::Value) -> ToolOutput {
            ToolOutput::ok("ok".to_string())
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(FakeTool { name: "Foo" });
        assert!(reg.get("Foo").is_some());
        assert!(reg.get("Bar").is_none());
    }

    #[test]
    fn schemas_returns_all() {
        let mut reg = ToolRegistry::new();
        reg.register(FakeTool { name: "A" });
        reg.register(FakeTool { name: "B" });
        let schemas = reg.schemas();
        assert_eq!(schemas.len(), 2);
    }

    #[test]
    fn with_filters_allowed() {
        let mut reg = ToolRegistry::new();
        reg.register(FakeTool { name: "Read" });
        reg.register(FakeTool { name: "Write" });
        let filtered = reg.with_filters(Some(vec!["Read".to_string()]), None);
        assert!(filtered.get("Read").is_some());
        assert!(filtered.get("Write").is_none());
    }

    #[test]
    fn with_filters_disallowed_overrides_allowed() {
        let mut reg = ToolRegistry::new();
        reg.register(FakeTool { name: "Read" });
        reg.register(FakeTool { name: "Write" });
        let filtered = reg.with_filters(
            Some(vec!["Read".to_string(), "Write".to_string()]),
            Some(vec!["Write".to_string()]),
        );
        assert!(filtered.get("Read").is_some());
        assert!(filtered.get("Write").is_none());
    }

    #[test]
    fn with_filters_no_allowed_keeps_all_minus_disallowed() {
        let mut reg = ToolRegistry::new();
        reg.register(FakeTool { name: "Read" });
        reg.register(FakeTool { name: "Bash" });
        let filtered = reg.with_filters(None, Some(vec!["Bash".to_string()]));
        assert!(filtered.get("Read").is_some());
        assert!(filtered.get("Bash").is_none());
    }

    #[test]
    fn default_is_empty() {
        let reg = ToolRegistry::default();
        assert!(reg.schemas().is_empty());
    }
}
