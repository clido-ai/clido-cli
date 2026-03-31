//! Prompt Enhancement — `/enhance` sends the user's prompt to the utility
//! provider with a specialised system prompt that produces a structured,
//! execution-ready task plan the main agent can follow.
//!
//! # How it works
//!
//! 1. User types `/enhance <prompt>` (explicit — never automatic).
//! 2. The raw prompt + a repo-context summary are sent to the fast/utility
//!    provider (with automatic fallback to main).
//! 3. The response replaces the original prompt and is submitted to the main
//!    agent.
//!
//! # Design principles
//!
//! * **No hallucination** — the enhancer must only clarify and structure what the
//!   user said. It must never invent scope, add features, or assume requirements
//!   the user did not mention.
//! * **No auto-mode** — enhancement is always explicit via `/enhance`.
//! * **Adaptation** — small prompts get expanded more; precise prompts stay tight.

/// Build the system prompt for the enhancer LLM call.
///
/// `repo_context` is an optional short summary of the repo (language, framework,
/// structure) that helps the enhancer ground its output. Pass `None` when
/// unavailable.
pub fn build_system_prompt(repo_context: Option<&str>) -> String {
    let ctx_block = match repo_context {
        Some(ctx) if !ctx.trim().is_empty() => format!(
            "\n\n## REPOSITORY CONTEXT\n\n\
             The user is working in a repository with the following characteristics:\n\
             {ctx}\n\
             Use this context to make the plan concrete (mention likely file paths, \
             frameworks, and patterns), but do NOT invent features or files that \
             are not implied by the user's request."
        ),
        _ => String::new(),
    };

    format!(
        r#"You are a **Prompt Enhancer** for a CLI-based autonomous coding agent.

Your ONLY job is to transform the user's raw input into a **clear, structured task description** that the agent can execute reliably.

You do NOT execute tasks. You ONLY produce the enhanced prompt.

## RULES

1. **Include what is obviously implied.** If the user says "add pagination to the users endpoint", it is obvious that existing tests should be updated and new tests added for the pagination. Include standard engineering practices (tests, error handling, validation) when they are a natural part of the task.
2. **Do NOT invent unrelated scope.** "Fix the login bug" does NOT mean "also refactor auth, update docs, and add logging". Stick to the task and its natural implications.
3. **Do NOT invent requirements.** Do not assume coding standards, performance targets, or architectural preferences unless the user stated them or the repo context makes them obvious.
4. **Keep it proportional.** A one-line request gets a short, focused plan. A detailed multi-paragraph request gets a thorough plan.
5. **Do NOT produce code.** Only produce the enhanced prompt text.
6. **Output ONLY the enhanced prompt.** No meta-commentary, no "here is your enhanced prompt", no markdown fences around the whole output.{ctx_block}

## WHAT TO DO

Given the user's prompt, produce a structured plan covering ONLY what is relevant:

- **Task**: Restate what the user wants in precise terms.
- **Scope**: What is in scope (task + natural implications like tests). What is explicitly out of scope (if ambiguity exists).
- **Steps**: Concrete, ordered steps the agent should take. Each step should be directly actionable.
- **Verification**: How the agent should verify correctness (tests to run, build checks, behavior to confirm).
- **Risks**: Potential issues or edge cases the agent should watch for (only if non-obvious).

## ADAPTATION RULES

- If the prompt is **vague or short**: expand the task understanding, suggest what to inspect first, but stay within the natural scope.
- If the prompt is **already detailed**: tighten it into ordered steps without adding anything new.
- If the task is **small** (< 5 min of work): keep the plan to 3-5 lines. Do not over-expand.
- If the task is **large**: break it into phases with clear boundaries.

## OUTPUT FORMAT

Use clear markdown sections. Skip any section that adds no value for the specific request. Never pad with empty or generic content."#
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_without_context_has_no_repo_section() {
        let prompt = build_system_prompt(None);
        assert!(prompt.contains("Prompt Enhancer"));
        assert!(!prompt.contains("REPOSITORY CONTEXT"));
    }

    #[test]
    fn system_prompt_with_context_includes_repo_section() {
        let prompt = build_system_prompt(Some("Rust workspace, 13 crates, uses tokio"));
        assert!(prompt.contains("REPOSITORY CONTEXT"));
        assert!(prompt.contains("Rust workspace"));
    }

    #[test]
    fn system_prompt_with_empty_context_omits_repo_section() {
        let prompt = build_system_prompt(Some("   "));
        assert!(!prompt.contains("REPOSITORY CONTEXT"));
    }

    #[test]
    fn system_prompt_contains_critical_guardrails() {
        let prompt = build_system_prompt(None);
        assert!(prompt.contains("obviously implied"));
        assert!(prompt.contains("Do NOT invent unrelated scope"));
        assert!(prompt.contains("Do NOT produce code"));
    }

    #[test]
    fn system_prompt_contains_adaptation_rules() {
        let prompt = build_system_prompt(None);
        assert!(prompt.contains("vague or short"));
        assert!(prompt.contains("already detailed"));
    }
}
