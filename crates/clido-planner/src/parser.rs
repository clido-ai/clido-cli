//! Parse a plan from LLM output (JSON task graph).

use crate::graph::{Plan, PlanMeta, TaskGraph};

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

/// Parse LLM output into a full `Plan` with metadata.
///
/// Handles two JSON shapes:
/// 1. Plan format: `{ "meta": { "id": "...", "goal": "...", "created_at": "..." }, "tasks": [...] }`
/// 2. Flat LLM format: `{ "goal": "...", "tasks": [...] }` (optionally with top-level `id`/`created_at`)
///
/// Generates `id` and `created_at` if missing. Validates the task graph.
pub fn parse_plan_with_meta(llm_output: &str) -> Result<Plan, PlanParseError> {
    let json_str = extract_json(llm_output);
    let v: serde_json::Value = serde_json::from_str(json_str)?;

    // If the JSON already has a "meta" object, try to parse it directly as a Plan.
    if v.get("meta").is_some() {
        let plan: Plan = serde_json::from_value(v.clone())?;
        plan.task_graph().validate()?;
        return Ok(plan);
    }

    // Otherwise parse as a TaskGraph (flat LLM output).
    let graph: TaskGraph = serde_json::from_value(v.clone())?;
    graph.validate()?;

    let id = v["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| {
        let goal = graph.goal.as_str();
        let hash: u64 = goal
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        format!("plan-{:08x}", hash as u32)
    });

    let created_at = v["created_at"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "2026-01-01T00:00:00Z".to_string());

    Ok(Plan {
        meta: PlanMeta {
            id,
            goal: graph.goal.clone(),
            created_at,
        },
        tasks: graph.tasks,
    })
}

/// Serialize a `Plan` back to pretty-printed JSON.
pub fn plan_to_json(plan: &Plan) -> Result<String, PlanParseError> {
    Ok(serde_json::to_string_pretty(plan)?)
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

    #[test]
    fn test_parse_plan_with_meta_full_schema() {
        let json = r#"{
            "id": "plan-custom",
            "goal": "build something",
            "created_at": "2026-03-15T12:00:00Z",
            "tasks": [
                {"id": "t1", "description": "first step", "depends_on": []},
                {"id": "t2", "description": "second step", "depends_on": ["t1"]}
            ]
        }"#;
        let plan = parse_plan_with_meta(json).unwrap();
        assert_eq!(plan.meta.id, "plan-custom");
        assert_eq!(plan.meta.goal, "build something");
        assert_eq!(plan.meta.created_at, "2026-03-15T12:00:00Z");
        assert_eq!(plan.tasks.len(), 2);
    }

    #[test]
    fn test_parse_plan_with_meta_generates_id_if_missing() {
        let json = r#"{
            "goal": "do the thing",
            "tasks": [
                {"id": "t1", "description": "step one", "depends_on": []}
            ]
        }"#;
        let plan = parse_plan_with_meta(json).unwrap();
        assert!(plan.meta.id.starts_with("plan-"));
        // "plan-" (5) + 8 hex chars = 13
        assert_eq!(plan.meta.id.len(), 13);
        assert_eq!(plan.meta.created_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn test_plan_to_json_roundtrip() {
        let json = r#"{
            "id": "plan-abc",
            "goal": "roundtrip test",
            "created_at": "2026-01-01T00:00:00Z",
            "tasks": [
                {"id": "t1", "description": "task one", "depends_on": []}
            ]
        }"#;
        let plan = parse_plan_with_meta(json).unwrap();
        let serialized = plan_to_json(&plan).unwrap();
        let plan2 = parse_plan_with_meta(&serialized).unwrap();
        assert_eq!(plan2.meta.id, plan.meta.id);
        assert_eq!(plan2.meta.goal, plan.meta.goal);
        assert_eq!(plan2.tasks.len(), plan.tasks.len());
    }
}
