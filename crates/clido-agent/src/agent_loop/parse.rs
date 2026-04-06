//! Parse assistant content blocks into tool calls; detect malformed batches.

use clido_core::ContentBlock;
use serde_json::Value;
use std::collections::HashSet;

/// Extract `(tool_use_id, tool_name, input)` from assistant message content.
pub(crate) fn tool_uses_from_assistant_content(
    content: &[ContentBlock],
) -> Result<Vec<(String, String, Value)>, String> {
    let mut out = Vec::new();
    let mut ids = HashSet::new();
    for block in content {
        if let ContentBlock::ToolUse { id, name, input } = block {
            if id.is_empty() {
                return Err("tool_use block has empty id".to_string());
            }
            if !ids.insert(id.clone()) {
                return Err(format!("duplicate tool_use id: {id}"));
            }
            if name.is_empty() {
                return Err("tool_use block has empty name".to_string());
            }
            out.push((id.clone(), name.clone(), input.clone()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_ids() {
        let content = vec![
            ContentBlock::ToolUse {
                id: "x".into(),
                name: "Read".into(),
                input: serde_json::json!({}),
            },
            ContentBlock::ToolUse {
                id: "x".into(),
                name: "Read".into(),
                input: serde_json::json!({}),
            },
        ];
        assert!(tool_uses_from_assistant_content(&content).is_err());
    }

    #[test]
    fn accepts_distinct_tools() {
        let content = vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "x"}),
            },
            ContentBlock::ToolUse {
                id: "b".into(),
                name: "Grep".into(),
                input: serde_json::json!({"pattern": "foo"}),
            },
        ];
        let u = tool_uses_from_assistant_content(&content).unwrap();
        assert_eq!(u.len(), 2);
    }
}
