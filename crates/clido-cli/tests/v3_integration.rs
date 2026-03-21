//! V3 integration test suite.
//!
//! Validates each V3 feature at the integration level:
//! 1. Workflow load + preflight + run (fixture YAML)
//! 2. Memory insert / search / prune / reset
//! 3. Sub-agent isolation
//! 4. Repo index build + search
//! 5. MCP client initialization (config deserialization)
//! 6. Semantic search tool schema

use std::io::Write;

// ---------------------------------------------------------------------------
// 1. Workflow load + preflight + dry-run
// ---------------------------------------------------------------------------

#[test]
fn workflow_load_and_preflight() {
    use clido_workflows::{load, preflight, validate};

    let yaml = r#"
name: code-review
version: "1"
inputs:
  - name: repo
    required: true
steps:
  - id: analyze
    prompt: "Analyze the repository at ${{ inputs.repo }}"
  - id: report
    prompt: "Write a report based on {{ steps.analyze.output }}"
"#;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(yaml.as_bytes()).unwrap();
    f.flush().unwrap();

    let def = load(f.path()).unwrap();
    assert_eq!(def.name, "code-review");
    assert_eq!(def.steps.len(), 2);

    // Validate
    validate(&def).unwrap();

    // Preflight
    let result = preflight(&def, &["default"], &["Read", "Write", "Glob"]);
    assert!(result.is_ok());
}

#[test]
fn workflow_template_github_actions_style() {
    use clido_workflows::{render, WorkflowContext};
    use std::collections::HashMap;

    let mut inputs = HashMap::new();
    inputs.insert(
        "branch".to_string(),
        serde_json::Value::String("main".to_string()),
    );
    let ctx = WorkflowContext::new(inputs);

    // ${{ inputs.branch }} should render correctly
    let out = render("Deploy branch: ${{ inputs.branch }}", &ctx).unwrap();
    assert_eq!(out, "Deploy branch: main");

    // Standard {{ inputs.branch }} should also work
    let out2 = render("Deploy branch: {{ inputs.branch }}", &ctx).unwrap();
    assert_eq!(out2, "Deploy branch: main");
}

#[tokio::test]
async fn workflow_dry_run_renders_prompts() {
    use clido_workflows::{load, render, validate, WorkflowContext};

    let yaml = r#"
name: dry-run-test
version: "1"
inputs:
  - name: target
    required: false
    default: "world"
steps:
  - id: greet
    prompt: "Hello {{ inputs.target }}"
"#;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(yaml.as_bytes()).unwrap();
    f.flush().unwrap();

    let def = load(f.path()).unwrap();
    validate(&def).unwrap();

    let inputs =
        WorkflowContext::resolve_inputs(&def, &[]).unwrap();
    let ctx = WorkflowContext::new(inputs);

    let prompt = render(&def.steps[0].prompt, &ctx).unwrap();
    assert_eq!(prompt, "Hello world");
}

// ---------------------------------------------------------------------------
// 2. Memory insert / search / prune / reset
// ---------------------------------------------------------------------------

#[test]
fn memory_insert_search_prune_reset() {
    use clido_memory::MemoryStore;
    use tempfile::NamedTempFile;

    let f = NamedTempFile::new().unwrap();
    let mut store = MemoryStore::open(f.path()).unwrap();

    // Insert
    let id1 = store.insert("rust ownership system", &["rust", "memory"]).unwrap();
    let id2 = store.insert("python garbage collection", &["python"]).unwrap();
    let _id3 = store.insert("async programming patterns", &["async"]).unwrap();

    assert_eq!(store.count().unwrap(), 3);

    // Keyword search — should find rust entry
    let results = store.search_keyword("rust", 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|e| e.id == id1));
    // Python and async entries should not appear for "rust"
    assert!(!results.iter().any(|e| e.id == id2));

    // List
    let list = store.list(10).unwrap();
    assert_eq!(list.len(), 3);

    // Prune: keep 2
    let deleted = store.prune_old(2).unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(store.count().unwrap(), 2);

    // Reset
    store.reset().unwrap();
    assert_eq!(store.count().unwrap(), 0);
}

#[test]
fn memory_hybrid_search_delegates_to_keyword() {
    use clido_memory::MemoryStore;
    use tempfile::NamedTempFile;

    let f = NamedTempFile::new().unwrap();
    let mut store = MemoryStore::open(f.path()).unwrap();
    store.insert("machine learning transformers", &[]).unwrap();

    let results = store.search_hybrid("transformers", 5).unwrap();
    assert!(!results.is_empty());
}

// ---------------------------------------------------------------------------
// 3. Sub-agent isolation (also tested in clido-agent/tests/)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sub_agent_isolation_from_cli_crate() {
    use async_trait::async_trait;
    use clido_agent::SubAgent;
    use clido_core::{
        AgentConfig, ContentBlock, Message, ModelResponse, PermissionMode, StopReason, ToolSchema,
        Usage,
    };
    use clido_providers::ModelProvider;
    use clido_tools::default_registry_with_blocked;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    struct EchoProvider(String);

    #[async_trait]
    impl ModelProvider for EchoProvider {
        async fn complete(
            &self,
            _m: &[Message],
            _t: &[ToolSchema],
            _c: &AgentConfig,
        ) -> clido_core::Result<ModelResponse> {
            tokio::task::yield_now().await;
            Ok(ModelResponse {
                id: "mock".to_string(),
                model: "mock".to_string(),
                content: vec![ContentBlock::Text { text: self.0.clone() }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 3,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            })
        }
        async fn complete_stream(
            &self,
            _m: &[Message],
            _t: &[ToolSchema],
            _c: &AgentConfig,
        ) -> clido_core::Result<
            Pin<Box<dyn Stream<Item = clido_core::Result<clido_providers::StreamEvent>> + Send>>,
        > {
            unimplemented!()
        }
    }

    let config = AgentConfig {
        model: "mock".to_string(),
        system_prompt: None,
        max_turns: 2,
        max_budget_usd: None,
        permission_mode: PermissionMode::AcceptAll,
        max_context_tokens: None,
        compaction_threshold: None,
        quiet: false,
        max_parallel_tools: 1,
        use_planner: false,
        use_index: false,
    };

    let tmp = std::env::temp_dir();
    let mut a1 = SubAgent::new(
        Arc::new(EchoProvider("hello".to_string())),
        default_registry_with_blocked(tmp.clone(), vec![]),
        config.clone(),
    );
    let mut a2 = SubAgent::new(
        Arc::new(EchoProvider("world".to_string())),
        default_registry_with_blocked(tmp, vec![]),
        config,
    );

    let r1 = a1.run("prompt").await.unwrap();
    let r2 = a2.run("prompt").await.unwrap();

    assert_eq!(r1, "hello");
    assert_eq!(r2, "world");
    assert_ne!(r1, r2, "Sub-agents must have isolated responses");
}

// ---------------------------------------------------------------------------
// 4. Repo index build + search
// ---------------------------------------------------------------------------

#[test]
fn repo_index_build_and_search() {
    use clido_index::RepoIndex;
    use std::fs;
    use tempfile::{tempdir, NamedTempFile};

    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("agent.rs"),
        "pub struct AgentLoop {}\npub fn run_agent() {}\npub trait ModelProvider {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("tools.rs"),
        "pub fn default_registry() {}\npub struct ReadTool {}\n",
    )
    .unwrap();

    let db = NamedTempFile::new().unwrap();
    let mut idx = RepoIndex::open(db.path()).unwrap();
    let count = idx.build(dir.path(), &["rs"]).unwrap();
    assert_eq!(count, 2);

    let (fc, sc) = idx.stats().unwrap();
    assert_eq!(fc, 2);
    assert!(sc >= 5, "Expected at least 5 symbols");

    // Symbol search
    let syms = idx.search_symbols("AgentLoop").unwrap();
    assert!(!syms.is_empty());
    assert_eq!(syms[0].name, "AgentLoop");

    // File search
    let files = idx.search_files("agent").unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].path.contains("agent"));
}

// ---------------------------------------------------------------------------
// 5. MCP client config deserialization
// ---------------------------------------------------------------------------

#[test]
fn mcp_config_roundtrip() {
    use clido_tools::McpConfig;

    let json = r#"{
        "servers": [
            {
                "name": "filesystem",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
            },
            {
                "name": "github",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_test" }
            }
        ]
    }"#;
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.servers.len(), 2);
    assert_eq!(cfg.servers[0].name, "filesystem");
    assert_eq!(cfg.servers[0].command, "npx");
    assert_eq!(cfg.servers[1].env.get("GITHUB_PERSONAL_ACCESS_TOKEN").map(String::as_str), Some("ghp_test"));

    // Serialize back to JSON
    let back = serde_json::to_string(&cfg).unwrap();
    let cfg2: McpConfig = serde_json::from_str(&back).unwrap();
    assert_eq!(cfg2.servers.len(), 2);
}

#[test]
fn mcp_load_config_from_file() {
    use clido_tools::load_mcp_config;

    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(
        br#"{"servers":[{"name":"test-srv","command":"python","args":["srv.py"]}]}"#,
    )
    .unwrap();
    f.flush().unwrap();

    let cfg = load_mcp_config(f.path()).unwrap();
    assert_eq!(cfg.servers.len(), 1);
    assert_eq!(cfg.servers[0].name, "test-srv");
    assert_eq!(cfg.servers[0].command, "python");
}

// ---------------------------------------------------------------------------
// 6. SemanticSearch tool schema
// ---------------------------------------------------------------------------

#[test]
fn semantic_search_tool_schema_and_name() {
    use clido_tools::{SemanticSearchTool, Tool};

    let tool = SemanticSearchTool::new(std::env::temp_dir());
    assert_eq!(tool.name(), "SemanticSearch");
    assert!(tool.is_read_only());

    let schema = tool.schema();
    assert_eq!(schema["type"], "object");
    let props = &schema["properties"];
    assert!(props.get("query").is_some());
    assert!(props.get("target_directory").is_some());
    assert!(props.get("num_results").is_some());
    let required = schema["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v.as_str() == Some("query")));
}
