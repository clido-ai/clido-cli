//! Parse a plan from LLM output (JSON task graph).

use crate::graph::TaskGraph;

#[derive(Debug, thiserror::Error)]
pub enum PlanParseError {
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing field: {0}")]
    MissingField(String),
    #[error("invalid graph: {0}")]
    InvalidGraph(#[from] crate::graph::GraphError),
}

/// Parse LLM output into a TaskGraph.
/// LLM output should be JSON: { "goal": "...", "tasks": [{ "id": "...", "description": "...", "depends_on": [] }] }
/// Returns Err if the JSON is malformed or the graph is invalid (caller falls back to reactive).
pub fn parse_plan(llm_output: &str) -> Result<TaskGraph, PlanParseError> {
    // Try to extract JSON from the output (LLM may wrap in markdown code block)
    let json_str = extract_json(llm_output);
    let graph: TaskGraph = serde_json::from_str(json_str)?;
    graph.validate()?;
    Ok(graph)
}

/// Extract the JSON object from raw LLM output that may include markdown fences.
fn extract_json(s: &str) -> &str {
    // Strip ```json ... ``` if present
    if let Some(start) = s.find("```json") {
        if let Some(end) = s[start + 7..].find("```") {
            return s[start + 7..start + 7 + end].trim();
        }
    }
    // Strip plain ``` ... ``` if present
    if let Some(start) = s.find("```") {
        if let Some(end) = s[start + 3..].find("```") {
            let inner = s[start + 3..start + 3 + end].trim();
            if inner.starts_with('{') {
                return inner;
            }
        }
    }
    // Try to find the outermost { ... }
    if let Some(start) = s.find('{') {
        if let Some(end) = s.rfind('}') {
            return &s[start..=end];
        }
    }
    s.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_plan() {
        let json = r#"{"goal":"do something","tasks":[{"id":"t1","description":"first step","depends_on":[]},{"id":"t2","description":"second step","depends_on":["t1"]}]}"#;
        let graph = parse_plan(json).unwrap();
        assert_eq!(graph.goal, "do something");
        assert_eq!(graph.tasks.len(), 2);
    }

    #[test]
    fn parse_markdown_wrapped_plan() {
        let markdown = "Here is my plan:\n```json\n{\"goal\":\"refactor\",\"tasks\":[{\"id\":\"t1\",\"description\":\"analyze\",\"depends_on\":[]}]}\n```\n";
        let graph = parse_plan(markdown).unwrap();
        assert_eq!(graph.goal, "refactor");
        assert_eq!(graph.tasks.len(), 1);
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let bad = "not json at all { broken";
        assert!(parse_plan(bad).is_err());
    }

    #[test]
    fn parse_cyclic_graph_returns_error() {
        let json = r#"{"goal":"cycle","tasks":[{"id":"a","description":"a","depends_on":["b"]},{"id":"b","description":"b","depends_on":["a"]}]}"#;
        assert!(parse_plan(json).is_err());
    }

    #[test]
    fn parse_plain_backtick_fence() {
        let fenced = "```\n{\"goal\":\"g\",\"tasks\":[{\"id\":\"t1\",\"description\":\"d\",\"depends_on\":[]}]}\n```";
        let graph = parse_plan(fenced).unwrap();
        assert_eq!(graph.goal, "g");
    }
}
