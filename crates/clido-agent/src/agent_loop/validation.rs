//! JSON Schema validation of tool inputs before execute.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clido_core::ToolFailureKind;
use clido_tools::ToolOutput;

use super::metrics::AgentMetrics;

const PREFIX: &str = "[validation_error] v1";

/// Compiled validators per tool name (invalidates when registry is replaced).
pub(crate) struct SchemaCache {
    map: HashMap<String, Arc<jsonschema::Validator>>,
}

impl SchemaCache {
    pub(crate) fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }

    /// Validate `input` against `schema_json` (tool `schema()` return value).
    pub(crate) fn validate(
        &mut self,
        tool_name: &str,
        schema_json: &Value,
        input: &Value,
    ) -> Result<(), String> {
        let validator = match self.map.get(tool_name) {
            Some(v) => v.clone(),
            None => {
                let v = Arc::new(jsonschema::validator_for(schema_json).map_err(|e| {
                    format!("{PREFIX} tool={tool_name} internal_schema_compile: {e}")
                })?);
                self.map.insert(tool_name.to_string(), v.clone());
                v
            }
        };
        let errors: Vec<String> = validator
            .iter_errors(input)
            .take(12)
            .map(|e| e.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{PREFIX} tool={tool_name} detail={}",
                errors.join("; ")
            ))
        }
    }
}

/// Fail closed: poisoned mutex → tool error (no execution).
pub(crate) fn validate_tool_json_or_tool_error(
    cache: &Mutex<SchemaCache>,
    metrics: &Arc<dyn AgentMetrics>,
    tool_name: &str,
    schema_json: &Value,
    input: &Value,
) -> Result<(), ToolOutput> {
    let mut guard = match cache.lock() {
        Ok(g) => g,
        Err(_) => {
            return Err(ToolOutput::err_kind(
                format!(
                    "{PREFIX} tool={tool_name} internal: schema cache lock poisoned; refusing execution"
                ),
                ToolFailureKind::ValidationInput,
            ));
        }
    };
    if let Err(msg) = guard.validate(tool_name, schema_json, input) {
        metrics.validation_rejected(tool_name);
        return Err(ToolOutput::err_kind(msg, ToolFailureKind::ValidationInput));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::agent_loop::metrics::NoopAgentMetrics;

    #[test]
    fn poisoned_mutex_returns_tool_error() {
        let cache = Arc::new(Mutex::new(SchemaCache::new()));
        let metrics: Arc<dyn AgentMetrics> = Arc::new(NoopAgentMetrics);
        let r = std::panic::catch_unwind(|| {
            let _g = cache.lock().unwrap();
            panic!("poison");
        });
        assert!(r.is_err());
        let schema = serde_json::json!({"type": "object"});
        let input = serde_json::json!({});
        let v = validate_tool_json_or_tool_error(cache.as_ref(), &metrics, "T", &schema, &input);
        assert!(v.is_err());
        let err = v.unwrap_err();
        assert!(err.is_error);
        assert!(err.content.contains("poisoned"));
    }

    #[test]
    fn rejects_wrong_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "required": ["n"]
        });
        let mut c = SchemaCache::new();
        let bad = serde_json::json!({ "n": "not int" });
        assert!(c.validate("T", &schema, &bad).is_err());
    }
}
