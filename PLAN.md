# Multi-Agent Exploration Implementation Plan

## Overview

Implement parallel sub-agents for file exploration and analysis tasks, significantly speeding up codebase understanding and initial task analysis.

## Goals

- **3x speedup** for exploration-heavy tasks
- **Default to profile's default model** - no special cheap model configuration
- **Smart rate limiting** - respect API limits, queue requests, exponential backoff
- **Seamless integration** - feels like single agent to user
- **Cost transparency** - track per-sub-agent costs

## Architecture

### Core Components

```
┌─────────────────────────────────────────┐
│           Parent Agent                  │
│  (Orchestrator - default model)         │
└──────────────┬──────────────────────────┘
               │ spawns
       ┌───────┴───────┐
       │               │
┌──────▼──────┐  ┌─────▼──────┐
│  Explorer   │  │  Explorer  │
│   Agent 1   │  │   Agent 2  │
│  (parallel) │  │  (parallel)│
└──────┬──────┘  └─────┬──────┘
       │               │
       └───────┬───────┘
               │
        ┌──────▼──────┐
        │ Result Merge │
        │   & Dedupe   │
        └──────────────┘
```

### Key Design Decisions

1. **Same model, same provider** - All sub-agents use the profile's default model
2. **Read-only exploration** - Sub-agents only Read/Glob/Grep/SemanticSearch, no Write/Edit/Bash
3. **Shared memory** - Common context, deduplicated file reads
4. **Rate limit aware** - Per-provider rate limit tracking, automatic throttling
5. **Timeout handling** - Sub-agents have shorter timeouts (30s default)

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1)

#### 1.1 Sub-Agent Spawn Mechanism

**File:** `crates/clido-agent/src/sub_agent.rs`

```rust
pub struct SubAgent {
    id: Uuid,
    config: AgentConfig,  // Same as parent, but read-only tools
    provider: Arc<dyn ModelProvider>,
    memory: SharedMemory, // Shared with parent
    rate_limiter: Arc<RateLimiter>,
}

impl SubAgent {
    pub async fn explore(
        &self,
        task: ExplorationTask,
    ) -> Result<ExplorationResult>;
}
```

**Key features:**
- Spawn from parent agent with shared context
- Inherit parent's model/provider configuration
- Restrict to read-only tools: `Read`, `Glob`, `Grep`, `SemanticSearch`
- Shared memory for deduplication

#### 1.2 Shared Memory & Deduplication

**File:** `crates/clido-memory/src/shared.rs`

```rust
pub struct SharedMemory {
    file_cache: Arc<RwLock<HashMap<PathBuf, FileContent>>>,
    search_cache: Arc<RwLock<HashMap<String, SearchResults>>>,
}

impl SharedMemory {
    // Before any sub-agent reads a file, check cache
    pub fn get_file(&self, path: &Path) -> Option<FileContent>;
    
    // After reading, cache for other sub-agents
    pub fn cache_file(&self, path: PathBuf, content: FileContent);
}
```

**Benefits:**
- File read once, shared across all sub-agents
- Reduces API calls and costs
- Faster subsequent reads

#### 1.3 Rate Limiting

**File:** `crates/clido-providers/src/rate_limit.rs`

```rust
pub struct RateLimiter {
    provider_id: String,
    requests_per_minute: u32,
    current_window: Arc<Mutex<SlidingWindow>>,
}

impl RateLimiter {
    pub async fn acquire(&self) -> Result<(), RateLimitError> {
        // Check if at limit
        // If yes: wait with exponential backoff
        // If no: increment counter and proceed
    }
}
```

**Provider-specific limits:**
- OpenAI: 60 RPM (RateLimiter tracks)
- Anthropic: 40 RPM
- Others: configurable

**Backoff strategy:**
- 429 response → exponential backoff (1s, 2s, 4s, 8s... max 60s)
- Queue requests if at limit
- Priority: parent agent > sub-agents

### Phase 2: Exploration Tasks (Week 2)

#### 2.1 Task Definition

**File:** `crates/clido-agent/src/exploration.rs`

```rust
pub enum ExplorationTask {
    /// Explore a specific directory
    ExploreDirectory { path: PathBuf, depth: u8 },
    
    /// Find all files matching pattern
    FindFiles { pattern: String, max_results: usize },
    
    /// Search for symbol/term across codebase
    SearchSymbol { symbol: String, file_types: Vec<String> },
    
    /// Analyze file dependencies
    AnalyzeDependencies { entry_points: Vec<PathBuf> },
    
    /// Get high-level overview
    CodebaseOverview { max_files: usize },
}

pub struct ExplorationResult {
    task_id: Uuid,
    files_read: Vec<PathBuf>,
    findings: Vec<Finding>,
    summary: String,
    cost: Usage,
    duration_ms: u64,
}
```

#### 2.2 Parallel Execution

**File:** `crates/clido-agent/src/orchestrator.rs`

```rust
pub struct ExplorationOrchestrator {
    max_concurrent: usize,  // Default: 3 sub-agents
    rate_limiter: Arc<RateLimiter>,
    shared_memory: SharedMemory,
}

impl ExplorationOrchestrator {
    pub async fn execute_parallel(
        &self,
        tasks: Vec<ExplorationTask>,
    ) -> Vec<ExplorationResult> {
        // Spawn up to max_concurrent sub-agents
        // Use tokio::spawn for each
        // Collect results with timeout
        // Merge and deduplicate
    }
}
```

**Concurrency control:**
- Default: 3 concurrent sub-agents
- Configurable via `max_exploration_agents` in config
- Respects rate limits - may queue if provider constrained

### Phase 3: Integration (Week 3)

#### 3.1 Tool Integration

**File:** `crates/clido-tools/src/explore_parallel.rs`

```rust
pub struct ExploreParallelTool;

impl Tool for ExploreParallelTool {
    fn name(&self) -> &str { "ExploreParallel" }
    
    async fn invoke(&self, input: Value) -> Result<ToolOutput> {
        // Parse exploration tasks from input
        // Spawn orchestrator
        // Execute parallel
        // Return merged results
    }
}
```

**Usage in system prompt:**
```
When you need to explore multiple files or directories, use ExploreParallel
to spawn sub-agents that work in parallel. This is faster for:
- Understanding large codebases
- Finding all occurrences of a pattern
- Analyzing multiple modules simultaneously
```

#### 3.2 TUI Integration

**File:** `crates/clido-cli/src/tui/render/status_panel.rs`

Add to status panel:
```
Exploring... (3 agents)
  ├─ Agent 1: reading src/main.rs
  ├─ Agent 2: searching for "config"
  └─ Agent 3: analyzing Cargo.toml
```

**Progress tracking:**
- Show number of active sub-agents
- Current task per agent
- Combined progress bar

#### 3.3 Cost Tracking

**File:** `crates/clido-agent/src/metrics.rs`

```rust
pub struct MultiAgentMetrics {
    parent_cost: Usage,
    sub_agent_costs: Vec<Usage>,
    total_cost: Usage,
    time_saved_ms: u64,  // vs sequential
}
```

**Display in TUI:**
```
Cost: $0.023 (parent: $0.008, sub-agents: $0.015)
Time saved: 45s (3x speedup)
```

### Phase 4: Optimization (Week 4)

#### 4.1 Smart Task Splitting

**Heuristics for automatic parallelization:**

```rust
fn should_parallelize(task: &Task) -> bool {
    match task {
        // Always parallelize
        Task::ExploreMultipleDirs(dirs) if dirs.len() > 1 => true,
        Task::SearchMultipleTerms(terms) if terms.len() > 1 => true,
        
        // Never parallelize (sequential dependency)
        Task::AnalyzeDependencyGraph => false,
        Task::StepByStepDebug => false,
        
        // Heuristic based on codebase size
        _ => estimate_exploration_time(task) > 10.seconds(),
    }
}
```

#### 4.2 Result Caching

Cache exploration results:
```rust
pub struct ExplorationCache {
    key: ExplorationTaskHash,
    result: ExplorationResult,
    ttl: Duration,  // Cache for 1 hour
}
```

**Benefits:**
- Same exploration task → instant result
- Reduced API costs
- Faster repeated queries

#### 4.3 Adaptive Concurrency

Adjust `max_concurrent` based on:
- Provider rate limits (dynamic)
- Current API latency
- Historical success rate
- User's cost preference

```rust
fn adaptive_concurrency(&self) -> usize {
    let base = self.config.max_exploration_agents;
    let rate_limit_factor = self.rate_limiter.available_capacity();
    let latency_factor = self.metrics.recent_latency();
    
    (base as f32 * rate_limit_factor * latency_factor) as usize
}
```

## Configuration

**`config.toml`:**

```toml
[exploration]
enabled = true
max_concurrent_agents = 3      # Default: 3
sub_agent_timeout_seconds = 30  # Default: 30s
cache_results = true           # Default: true
cache_ttl_minutes = 60         # Default: 60

[exploration.rate_limit]
# Per-provider rate limiting
# If not specified, uses sensible defaults
openai_requests_per_minute = 60
anthropic_requests_per_minute = 40
```

**Environment variables:**
- `CLIDO_EXPLORATION_ENABLED=1`
- `CLIDO_MAX_EXPLORATION_AGENTS=3`

## Testing Strategy

1. **Unit tests:**
   - Rate limiter behavior
   - Shared memory deduplication
   - Task splitting heuristics

2. **Integration tests:**
   - End-to-end exploration workflow
   - Rate limit handling under load
   - Cost tracking accuracy

3. **Benchmarks:**
   - Compare sequential vs parallel
   - Measure actual speedup
   - Cost vs time tradeoffs

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Rate limit exceeded | Exponential backoff, queue requests |
| Cost explosion | Default to same model, track costs |
| Merge conflicts | Read-only sub-agents, no writes |
| Timeout cascade | Shorter sub-agent timeouts |
| Cache inconsistency | TTL, invalidate on file change |

## Success Metrics

- **Speedup:** 2-3x faster for exploration tasks
- **Cost increase:** <50% vs sequential (due to caching)
- **User satisfaction:** "Feels faster" in TUI feedback
- **Reliability:** <1% rate limit errors

## Future Enhancements

1. **Speculative execution** - Predict next files, pre-fetch
2. **Hierarchical agents** - Manager agent coordinates workers
3. **Specialized agents** - One for Rust, one for JS, etc.
4. **Learning** - Remember which explorations were useful

## Timeline

- **Week 1:** Core infrastructure (sub-agent spawn, shared memory, rate limiting)
- **Week 2:** Exploration tasks, parallel execution
- **Week 3:** Tool integration, TUI, cost tracking
- **Week 4:** Optimization, caching, adaptive concurrency
- **Week 5:** Testing, documentation, release

**Total: 5 weeks**
