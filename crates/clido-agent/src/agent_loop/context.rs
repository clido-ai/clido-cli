//! Context compaction and proactive summarization.

use clido_context::{
    assemble, dedup_file_reads, estimate_tokens_message, estimate_tokens_messages,
    estimate_tokens_str,
};
use clido_core::{AgentConfig, ClidoError, ContentBlock, Message, Result, Role};
use clido_providers::ModelProvider;
use tracing::warn;

use super::history::repair_orphaned_tool_calls;

/// Proactive summarization triggers at this fraction of effective max context (below full compaction).
pub(crate) const PROACTIVE_SUMMARIZE_THRESHOLD: f64 = 0.45;

/// Reserve tokens for model output + provider overhead so request assembly stays under the real limit.
pub(crate) const CONTEXT_OUTPUT_RESERVE: u32 = 12_288;
/// Maximum number of tool pairs to summarize per turn (to limit latency).
const MAX_PROACTIVE_SUMMARIES_PER_TURN: usize = 5;

/// Find indices of the oldest tool_call + tool_result message pairs in history
/// that haven't already been summarized. Returns pairs of (assistant_idx, user_idx).
pub(crate) fn find_unsummarized_tool_pairs(
    messages: &[Message],
    max: usize,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < messages.len().saturating_sub(1) && pairs.len() < max {
        let msg = &messages[i];
        let next = &messages[i + 1];
        // Look for Assistant message with ToolUse followed by User message with ToolResult
        if msg.role == Role::Assistant
            && next.role == Role::User
            && msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            && next
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        {
            // Skip if already summarized (contains our marker)
            let already_summarized = next.content.iter().any(|b| {
                if let ContentBlock::ToolResult { content, .. } = b {
                    content.starts_with("[Summary]")
                } else {
                    false
                }
            });
            if !already_summarized {
                pairs.push((i, i + 1));
            }
        }
        i += 1;
    }
    pairs
}

/// Proactively summarize oldest tool pairs to reduce context before hitting compaction.
/// Replaces the tool result content with a 1-sentence LLM summary.
pub(crate) async fn proactive_summarize_pairs(
    history: &mut [Message],
    provider: &dyn ModelProvider,
    config: &AgentConfig,
    preserve_recent: usize,
) -> usize {
    // Only consider messages outside the "recent" window
    let safe_len = history.len().saturating_sub(preserve_recent);
    if safe_len < 2 {
        return 0;
    }

    let pairs =
        find_unsummarized_tool_pairs(&history[..safe_len], MAX_PROACTIVE_SUMMARIES_PER_TURN);
    if pairs.is_empty() {
        return 0;
    }

    let mut summarized = 0;
    for (asst_idx, user_idx) in &pairs {
        // Build a concise representation of the tool call + result
        let tool_call_text: String = history[*asst_idx]
            .content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { name, input, .. } = b {
                    let input_preview: String = serde_json::to_string(input)
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect();
                    Some(format!("{}({})", name, input_preview))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let result_text: String = history[*user_idx]
            .content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolResult {
                    content, is_error, ..
                } = b
                {
                    let prefix = if *is_error { "ERROR: " } else { "" };
                    let preview: String = content.chars().take(500).collect();
                    Some(format!("{}{}", prefix, preview))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Summarize this tool interaction in ONE sentence (max 100 words). \
             Focus on the outcome/result, not the process.\n\n\
             Tool call: {}\nResult: {}",
            tool_call_text, result_text
        );

        // Use fast model for summarization
        let messages_for_llm = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.clone(),
            }],
        }];

        match provider.complete(&messages_for_llm, &[], config).await {
            Ok(response) => {
                let summary = response
                    .content
                    .iter()
                    .find_map(|b| {
                        if let ContentBlock::Text { text } = b {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "Tool interaction completed.".to_string());

                // Replace tool result content with summary
                for block in &mut history[*user_idx].content {
                    if let ContentBlock::ToolResult { content, .. } = block {
                        *content = format!("[Summary] {}", summary);
                    }
                }
                summarized += 1;
            }
            Err(e) => {
                warn!("Proactive summarization failed: {}", e);
                break; // Stop if LLM call fails
            }
        }
    }
    summarized
}

/// Compress tool result content in older messages to reduce context before summarization.
/// Keeps the last `preserve_recent` messages intact, only compresses older ones.
pub(crate) fn compress_tool_results(messages: &mut [Message], preserve_recent: usize) {
    let compress_end = messages.len().saturating_sub(preserve_recent);
    for msg in messages[..compress_end].iter_mut() {
        for block in msg.content.iter_mut() {
            if let ContentBlock::ToolResult { content, .. } = block {
                if content.len() > 500 {
                    // Use char_indices for safe multi-byte truncation.
                    let boundary = content
                        .char_indices()
                        .nth(300)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());
                    let truncated = format!(
                        "{}... [truncated {} chars]",
                        &content[..boundary],
                        content.len() - boundary
                    );
                    *content = truncated;
                }
            }
        }
    }
}

/// Drop-in async replacement for `assemble()` that uses the provider to produce
/// a meaningful summary of the dropped history instead of a static placeholder.
///
/// Falls back to the static-placeholder path (identical to `assemble()`) if the
/// summarization call fails for any reason, so the agent loop is never blocked.
pub(crate) async fn compact_with_summary(
    messages: &[Message],
    system_prompt_tokens: u32,
    max_context_tokens: u32,
    compaction_threshold: f64,
    provider: &dyn ModelProvider,
    config: &AgentConfig,
) -> Result<Vec<Message>> {
    // Deduplicate repeated file reads before counting tokens.
    // Then repair any orphaned tool_use blocks that dedup may have created by
    // dropping a user message whose ToolResult content was a duplicate.
    let mut deduped = dedup_file_reads(messages);
    repair_orphaned_tool_calls(&mut deduped);
    let msgs = deduped.as_slice();

    let threshold_limit = ((max_context_tokens as f64) * compaction_threshold) as u32;
    let total = system_prompt_tokens + estimate_tokens_messages(msgs);

    // Under threshold — nothing to do.
    if total <= threshold_limit {
        return Ok(msgs.to_vec());
    }

    // Find the split point: keep the tail that fits within max_context_tokens.
    // Reserve 512 tokens for the summary message.
    const SUMMARY_RESERVE: u32 = 2048;
    let mut kept_tokens = 0u32;
    let mut start = msgs.len();
    for (i, m) in msgs.iter().enumerate().rev() {
        let mt = estimate_tokens_message(m);
        if kept_tokens + mt + system_prompt_tokens + SUMMARY_RESERVE > max_context_tokens {
            break;
        }
        kept_tokens += mt;
        start = i;
    }

    // Never split between an Assistant(ToolUse) and its User(ToolResult).
    while start > 0 && start < msgs.len() {
        let msg = &msgs[start];
        if msg.role == Role::User
            && msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        {
            start -= 1;
        } else {
            break;
        }
    }

    // Nothing to compact (entire history fits in tail) — let assemble() handle it.
    if start == 0 {
        return assemble(
            msgs,
            system_prompt_tokens,
            max_context_tokens,
            compaction_threshold,
        );
    }

    let to_compact = &msgs[..start];
    let tail = &msgs[start..];

    // Compress old tool results to reduce summarization input.
    let mut compressed = to_compact.to_vec();
    compress_tool_results(&mut compressed, 4);

    // Try LLM summarization; log and fall back to static text on failure.
    let summary_text = match summarize_messages(&compressed, provider, config).await {
        Ok(s) => {
            tracing::info!(
                dropped = to_compact.len(),
                kept = tail.len(),
                summary_chars = s.len(),
                "context compacted with LLM summary"
            );
            format!("[Compacted history] {s}")
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "context compaction: summarization failed, using static placeholder"
            );
            "[Compacted history] Earlier messages were omitted to fit context.".to_string()
        }
    };

    // Verify the compacted result still fits.
    let summary_tokens = estimate_tokens_str(&summary_text) + 4;
    let total_after = system_prompt_tokens + summary_tokens + kept_tokens;
    if total_after > max_context_tokens {
        return Err(ClidoError::ContextLimit {
            tokens: total_after as u64,
        });
    }

    let mut out = vec![Message {
        role: Role::System,
        content: vec![ContentBlock::Text { text: summary_text }],
    }];
    out.extend_from_slice(tail);
    Ok(out)
}

/// Last-resort truncation: drops the oldest messages one-by-one until history fits the budget.
/// Keeps at minimum the most recent user message and the last assistant/tool exchange so the
/// agent can still respond meaningfully. Returns a system message explaining the truncation.
fn aggressive_tail_truncation(
    messages: &[Message],
    system_prompt_tokens: u32,
    budget: u32,
) -> Result<Vec<Message>> {
    const MIN_KEEP: usize = 4; // system + last 3 messages minimum

    let usable = budget.saturating_sub(system_prompt_tokens);
    if usable == 0 {
        return Err(ClidoError::ContextLimit { tokens: budget as u64 });
    }

    // Count tokens per message and work backwards from the end.
    let msg_tokens: Vec<usize> = messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .map(|c| match c {
                    ContentBlock::Text { text } => text.chars().count() / 3,
                    ContentBlock::ToolUse { name, input, .. } => {
                        name.chars().count() + input.to_string().len() / 3
                    }
                    ContentBlock::ToolResult { content, .. } => content.chars().count() / 3,
                    ContentBlock::Thinking { thinking, .. } => thinking.chars().count() / 3,
                    ContentBlock::Image { .. } => 0, // images don't add text tokens
                })
                .sum::<usize>()
        })
        .collect();

    let mut tail_start = 0;
    let mut tail_total: usize = msg_tokens.iter().sum();

    while tail_total > usable as usize && messages.len() - tail_start > MIN_KEEP {
        tail_total -= msg_tokens[tail_start];
        tail_start += 1;
    }

    // Build output: system notice + surviving tail messages.
    let mut out = Vec::with_capacity(1 + messages.len() - tail_start);
    let dropped = tail_start;
    out.push(Message {
        role: Role::System,
        content: vec![ContentBlock::Text {
            text: format!(
                "[Context truncated: dropped {dropped} older message(s) to stay within limits.]"
            ),
        }],
    });
    out.extend_from_slice(&messages[tail_start..]);
    warn!(
        "aggressive truncation: dropped {dropped} messages, kept {}",
        out.len() - 1
    );
    Ok(out)
}

/// Like [`compact_with_summary`], but subtracts an output reserve from the context budget and
/// retries once with a tighter budget + forced compaction if the first pass hits `ContextLimit`.
/// If even the tightest compaction fails, falls back to aggressive tail-only truncation —
/// keeping only the most recent messages that fit within the budget, ensuring the agent never
/// hard-fails on context limits.
pub(crate) async fn compact_for_model_request(
    messages: &[Message],
    system_prompt_tokens: u32,
    max_context_tokens: u32,
    compaction_threshold: f64,
    provider: &dyn ModelProvider,
    config: &AgentConfig,
) -> Result<Vec<Message>> {
    let budget = max_context_tokens
        .saturating_sub(CONTEXT_OUTPUT_RESERVE)
        .max(24_000);
    match compact_with_summary(
        messages,
        system_prompt_tokens,
        budget,
        compaction_threshold,
        provider,
        config,
    )
    .await
    {
        Ok(m) => Ok(m),
        Err(ClidoError::ContextLimit { .. }) => {
            warn!(
                "context assembly exceeded budget={budget}; retrying with tighter budget and full compaction"
            );
            let tight = budget.saturating_sub(8192).max(16_000);
            match compact_with_summary(
                messages,
                system_prompt_tokens,
                tight,
                0.0,
                provider,
                config,
            )
            .await
            {
                Ok(m) => Ok(m),
                Err(ClidoError::ContextLimit { .. }) => {
                    warn!(
                        "context assembly still exceeded budget={tight}; falling back to tail-only truncation"
                    );
                    aggressive_tail_truncation(messages, system_prompt_tokens, budget)
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// Format `messages` as a flat transcript and ask the provider to summarize them.
pub(crate) async fn summarize_messages(
    messages: &[Message],
    provider: &dyn ModelProvider,
    config: &AgentConfig,
) -> Result<String> {
    const MAX_TOOL_RESULT_CHARS: usize = 1_500;

    let mut transcript = String::new();
    for msg in messages {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    transcript.push_str(&format!("[{role_label}]: {text}\n\n"));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    transcript.push_str(&format!("[{role_label}] Tool call: {name}({input})\n\n"));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    let body = if content.chars().count() > MAX_TOOL_RESULT_CHARS {
                        format!(
                            "{}… (truncated)",
                            content
                                .chars()
                                .take(MAX_TOOL_RESULT_CHARS)
                                .collect::<String>()
                        )
                    } else {
                        content.clone()
                    };
                    transcript.push_str(&format!("[{role_label}] {label}: {body}\n\n"));
                }
                _ => {} // skip Image / Thinking blocks
            }
        }
    }

    let prompt = format!(
        "You are a summarizer for a coding agent session.\n\
        Summarize the following conversation history in 2–4 concise paragraphs.\n\
        Preserve:\n\
        - Every file path that was read or edited (list them).\n\
        - Every tool name that was called (list them).\n\
        - The user's high-level goal and any constraints they stated.\n\
        - The current state of the task (what was done, what might be left).\n\
        \n\
        Output only the summary, no preamble.\n\
        \n\
        ---\n\n\
        {transcript}"
    );

    let request = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: prompt }],
    }];

    let response = provider.complete(&request, &[], config).await?;

    let text: String = response
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        return Err(ClidoError::Other(anyhow::anyhow!(
            "summarization returned empty response"
        )));
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MIN_KEEP: usize = 4;

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text { text: text.to_string() }],
        }
    }

    #[test]
    fn aggressive_tail_truncation_drops_oldest_messages() {
        // Create 20 messages of 1000 chars each (≈333 tokens per message).
        // Total: ~6660 tokens.
        let messages: Vec<Message> = (0..20)
            .map(|_i| text_msg(Role::Assistant, &"x".repeat(1000)))
            .collect();
        let system_tokens = 0;
        // Budget: 3000 chars → fits ~9 messages.
        let budget = 3_000;

        let result = aggressive_tail_truncation(&messages, system_tokens, budget).unwrap();
        // First message is system truncation notice.
        match &result[0].content[0] {
            ContentBlock::Text { text } => assert!(text.starts_with("[Context truncated")),
            _ => panic!("expected text block"),
        }
        // Should keep ~9 messages + 1 system notice = 10.
        assert!(result.len() < 21);
        assert!(result.len() > TEST_MIN_KEEP);
    }

    #[test]
    fn aggressive_tail_truncation_keeps_minimum() {
        // Very tight budget — only MIN_KEEP should remain.
        let messages: Vec<Message> = (0..10)
            .map(|_i| text_msg(Role::Assistant, &"x".repeat(2000)))
            .collect();
        let result = aggressive_tail_truncation(&messages, 0, 3000).unwrap();
        assert_eq!(result.len(), TEST_MIN_KEEP + 1); // system notice + TEST_MIN_KEEP messages
    }
}
