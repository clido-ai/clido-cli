//! JSON Schema validation of tool inputs before execute.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

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

#[cfg(test)]
mod tests {
    use super::*;

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
