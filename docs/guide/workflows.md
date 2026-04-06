# Workflows

Workflows are declarative YAML files that define multi-step agent pipelines. Each step is an agent invocation with a specific prompt, model, and tool set. Steps can depend on each other, run in parallel, and pass outputs to downstream steps.

## What workflows are for

Workflows are ideal for:

- Repeatable multi-stage tasks (analyse → plan → implement → test)
- CI pipeline integration (code review, test generation, changelog writing)
- Tasks with conditional branching or retry logic
- Distributing work across multiple agent calls to stay within context limits

## A complete annotated example

::: v-pre
```yaml
# .clido/workflows/full-review.yaml
name: full-review
description: Review a PR, generate a summary, and file any issues found.

# Workflow-level inputs (passed with -i key=value)
inputs:
  branch:
    description: Branch to review
    required: true
  max_issues:
    description: Maximum number of issues to file
    default: "5"

steps:
  # Step 1: gather diff
  - id: diff
    prompt: |
      Run `git diff main..${{ inputs.branch }}` and summarise the changes.
      Return the summary as plain text.
    tools: [Bash]
    profile: fast          # use a cheaper/faster profile for this step
    on_error: fail         # fail the workflow if this step errors

  # Step 2: security review (depends on diff)
  - id: security
    prompt: |
      Review the following diff for security issues:
      ${{ steps.diff.output }}

      List each issue with: severity (HIGH/MEDIUM/LOW), file, line, and description.
    tools: [Read, Glob]
    depends_on: [diff]

  # Step 3: style review (runs in parallel with security)
  - id: style
    prompt: |
      Review the following diff for code style and best practice issues:
      ${{ steps.diff.output }}
    tools: [Read]
    depends_on: [diff]

  # Step 4: file issues (depends on both reviews)
  - id: file_issues
    prompt: |
      Based on these reviews, file the top ${{ inputs.max_issues }} issues as GitHub comments.

      Security review:
      ${{ steps.security.output }}

      Style review:
      ${{ steps.style.output }}
    tools: [Bash]
    depends_on: [security, style]
    retry:
      max_attempts: 2
      on_error: continue   # continue workflow even if filing fails
```
:::

## Running a workflow

```bash
clido workflow run full-review.yaml -i branch=feature/my-feature
```

With multiple inputs:

```bash
clido workflow run full-review.yaml \
  -i branch=feature/my-feature \
  -i max_issues=3
```

Dry run (validate and render prompts without making API calls):

```bash
clido workflow run full-review.yaml -i branch=main --dry-run
```

Skip the cost confirmation prompt:

```bash
clido workflow run full-review.yaml -i branch=main --yes
```

## Validating a workflow

Check for schema errors and missing inputs:

```bash
clido workflow validate full-review.yaml
```

```
✓ Schema valid
✓ All inputs defined
✓ No circular dependencies
✓ All profiles exist in config
```

## Inspecting a workflow

Print the step graph with dependencies:

```bash
clido workflow inspect full-review.yaml
```

```
Workflow: full-review

Steps (4):
  diff         [no deps]         profile: fast
  security     [depends: diff]
  style        [depends: diff]
  file_issues  [depends: security, style]

Execution order:
  Parallel group 1: diff
  Parallel group 2: security, style
  Parallel group 3: file_issues
```

## Pre-flight checks

Run preflight checks to verify all resources are available:

```bash
clido workflow check full-review.yaml
```

```
✓ Profile 'default' exists
✓ Profile 'fast' exists
✓ Tools: Bash, Read, Glob are available
✓ Input 'branch' is required (not provided — pass with -i branch=...)
```

## Listing workflows

List all workflows in the configured workflow directory (default: `.clido/workflows`):

```bash
clido workflow list
```

```
full-review      Review a PR, generate a summary, and file issues.
generate-tests   Generate unit tests for modified files.
changelog        Write a CHANGELOG entry from recent commits.
```

## Step definition reference

Each step supports the following fields:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique identifier for this step |
| `prompt` | string | Yes | Prompt template sent to the agent |
| `tools` | list | No | Tool names available to this step (default: all) |
| `profile` | string | No | Profile to use (default: `default_profile`) |
| `depends_on` | list | No | Step IDs this step waits for |
| `on_error` | string | No | `fail` (default) or `continue` |
| `retry` | object | No | Retry configuration (see below) |

### Retry configuration

```yaml
retry:
  max_attempts: 3     # Number of attempts (including first). Default: 1 (no retry).
  on_error: fail      # What to do when all attempts fail: fail or continue.
```

## Dynamic parameters

Prompts can reference:

::: v-pre
| Expression | Value |
|-----------|-------|
| `${{ inputs.name }}` | Workflow input value |
| `${{ steps.id.output }}` | Text output of a completed step |
:::

Expressions are rendered before the step runs. Referencing an incomplete step is a validation error.

## Parallel execution

Steps with no shared dependencies run in parallel when marked with `parallel: true` in the workflow YAML. In the example above, `security` and `style` both depend only on `diff`, so they can run concurrently.

That parallelism is implemented in the **workflow executor** (separate agent runs per step). Separately, within one agent session, a model turn that requests **only read-only** tools can execute those tools in parallel up to `[agent] max-concurrent-tools` / `--max-parallel-tools`; turns that include any write-capable tool run tools one at a time.

## Workflow storage

By default clido looks for workflows in `.clido/workflows` relative to the current directory. Change the path in config:

```toml
[workflows]
directory = "automation/clido"
```

## TUI Commands

You can create, manage, and run workflows directly from the interactive TUI:

### Create with AI guidance

```
/workflow new review PRs and run tests before merging
```

The agent walks you through the design — asking about steps, tools, inputs, error handling, and parallelism. When the design is ready, it outputs the complete YAML. You can iterate on it naturally in the conversation.

### Save the result

```
/workflow save             # uses the workflow's name field as filename
/workflow save my-review   # saves as my-review.yaml
```

Scans the last assistant messages for a YAML code block, validates it, and saves to `.clido/workflows/`.

### List saved workflows

```
/workflow list
```

Shows all workflows from `.clido/workflows/` and `~/.config/clido/workflows/`.

### View a workflow

```
/workflow show full-review
```

Displays the full YAML in the chat.

### Edit manually

```
/workflow edit full-review  # edit a saved workflow
/workflow edit              # edit the last YAML draft from chat
```

Opens a nano-style text editor overlay. **Ctrl+S** validates and saves, **Esc** discards.

### Run a workflow

```
/workflow run full-review
```

Sends the workflow steps to the agent for execution. For advanced runs with inputs and parallel control, use the CLI:

```bash
clido workflow run full-review.yaml -i branch=main
```
