//! Context assembly and token budget: truncate or compact history to fit model context.

use clido_core::{ClidoError, ContentBlock, Message, Result};

/// Default max context tokens when not set in config or pricing.
pub const DEFAULT_MAX_CONTEXT_TOKENS: u32 = 200_000;

/// Default compaction threshold (compact when context > max * threshold).
pub const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.75;

/// Placeholder text for compacted (dropped) history.
const COMPACTED_PLACEHOLDER: &str =
    "[Compacted history] Earlier messages were omitted to fit context.";

/// Estimate token count for a string (chars/4 heuristic).
#[inline]
pub fn estimate_tokens_str(s: &str) -> u32 {
    (s.chars().count() as u32).div_ceil(4)
}

/// Estimate token count for a single message.
pub fn estimate_tokens_message(m: &Message) -> u32 {
    let mut n = 4; // role + structure overhead
    for block in &m.content {
        n += estimate_tokens_block(block);
    }
    n
}

fn estimate_tokens_block(b: &ContentBlock) -> u32 {
    match b {
        ContentBlock::Text { text } => estimate_tokens_str(text),
        ContentBlock::ToolUse { id, name, input } => {
            4 + estimate_tokens_str(id)
                + estimate_tokens_str(name)
                + estimate_tokens_str(&input.to_string())
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } => 4 + estimate_tokens_str(tool_use_id) + estimate_tokens_str(content),
        ContentBlock::Thinking { thinking } => estimate_tokens_str(thinking),
    }
}

/// Total token estimate for a slice of messages.
pub fn estimate_tokens_messages(messages: &[Message]) -> u32 {
    messages.iter().map(estimate_tokens_message).sum()
}

/// Assemble context: return messages that fit within the token budget.
/// If over threshold * max_context_tokens, compact by keeping the last N messages
/// and prepending a system message that earlier messages were omitted.
/// If even that would exceed max_context_tokens, return ContextLimit error.
pub fn assemble(
    messages: &[Message],
    system_prompt_tokens: u32,
    max_context_tokens: u32,
    compaction_threshold: f64,
) -> Result<Vec<Message>> {
    let threshold_limit = ((max_context_tokens as f64) * compaction_threshold) as u32;
    let placeholder_tokens = estimate_tokens_str(COMPACTED_PLACEHOLDER) + 4;
    let total = system_prompt_tokens + estimate_tokens_messages(messages);

    if total <= threshold_limit {
        return Ok(messages.to_vec());
    }

    // Compact: keep tail of messages so total + placeholder <= max_context_tokens
    let mut kept_tokens = 0u32;
    let mut start = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        let mt = estimate_tokens_message(m);
        if kept_tokens + mt + system_prompt_tokens + placeholder_tokens > max_context_tokens {
            break;
        }
        kept_tokens += mt;
        start = i;
    }

    if start == 0 && kept_tokens + system_prompt_tokens + placeholder_tokens > max_context_tokens {
        return Err(ClidoError::ContextLimit {
            tokens: (system_prompt_tokens + kept_tokens + placeholder_tokens) as u64,
        });
    }

    let tail: Vec<Message> = messages[start..].to_vec();
    let compacted_tokens =
        system_prompt_tokens + placeholder_tokens + estimate_tokens_messages(&tail);
    if compacted_tokens > max_context_tokens {
        return Err(ClidoError::ContextLimit {
            tokens: compacted_tokens as u64,
        });
    }

    tracing::warn!(
        "Context compacted: dropped {} messages, keeping {} ({} tokens)",
        start,
        tail.len(),
        compacted_tokens
    );

    let mut out = vec![Message {
        role: clido_core::Role::System,
        content: vec![ContentBlock::Text {
            text: COMPACTED_PLACEHOLDER.to_string(),
        }],
    }];
    out.extend(tail);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clido_core::Role;

    #[test]
    fn estimate_tokens_str_basic() {
        assert!(estimate_tokens_str("hello") >= 1);
        assert!(estimate_tokens_str("") == 0);
    }

    #[test]
    fn assemble_under_threshold_returns_unchanged() {
        let m = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        };
        let out = assemble(std::slice::from_ref(&m), 0, 100_000, 0.75).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.len(), 1);
    }

    #[test]
    fn assemble_over_threshold_compacts() {
        let mut messages = Vec::new();
        for i in 0..20 {
            messages.push(Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: "x".repeat(2000),
                }],
            });
        }
        let total_tokens = estimate_tokens_messages(&messages);
        let max = total_tokens / 2;
        let out = assemble(&messages, 0, max, 0.75).unwrap();
        assert!(out.len() < messages.len());
        assert!(out[0].role == Role::System);
        assert!(estimate_tokens_messages(&out) <= max + 100);
    }
}
