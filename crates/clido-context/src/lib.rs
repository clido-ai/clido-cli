//! Context assembly and token budget: truncate or compact history to fit model context.

pub mod read_cache;
pub mod rules;

pub use rules::{assemble_rules_prompt, discover as discover_rules, RulesFile};

use clido_core::{ClidoError, ContentBlock, Message, Result};

/// Discover rules files, assemble them into a prompt string, and return it.
/// This is the main entry point for the rules feature.
///
/// Returns an empty string if no rules files are found or `no_rules` is true.
pub fn load_and_assemble_rules(
    cwd: &std::path::Path,
    no_rules: bool,
    rules_file: Option<&std::path::Path>,
) -> String {
    let files = rules::discover(cwd, no_rules, rules_file);
    rules::assemble_rules_prompt(&files)
}

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
        ContentBlock::Image { .. } => 85, // rough token estimate for an image block
    }
}

/// Total token estimate for a slice of messages.
pub fn estimate_tokens_messages(messages: &[Message]) -> u32 {
    messages.iter().map(estimate_tokens_message).sum()
}

/// Deduplicate repeated file reads in context.
///
/// Scans ToolResult blocks for file-path reads (content that looks like a Read tool result
/// with the same path and content). If the same path appears multiple times with identical
/// content, only the most recent occurrence is kept.
pub fn dedup_file_reads(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashMap;

    // First pass: for each tool_use_id in ToolResult blocks, check if it's a read result
    // We track (tool_use_id → content) for ToolResult blocks, then find duplicates.
    // Strategy: collect all (index, tool_use_id, content) for ToolResult blocks in User messages.
    // If content is identical to a later occurrence, remove the earlier one.

    // Collect ToolResult positions: (msg_index, block_index, tool_use_id, content)
    let mut result_positions: Vec<(usize, usize, String, String)> = Vec::new();
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } = block
            {
                result_positions.push((mi, bi, tool_use_id.clone(), content.clone()));
            }
        }
    }

    // Find duplicate content: same content appearing in multiple ToolResult blocks.
    // Keep only the last occurrence of each content value.
    // Use a map from content → last position index.
    let mut content_last_seen: HashMap<String, usize> = HashMap::new();
    for (pos_idx, (_, _, _, content)) in result_positions.iter().enumerate() {
        content_last_seen.insert(content.clone(), pos_idx);
    }

    // Build set of (msg_index, block_index) to remove (earlier duplicates)
    let mut to_remove: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut content_seen_at: HashMap<String, usize> = HashMap::new();
    for (pos_idx, (mi, bi, _, content)) in result_positions.iter().enumerate() {
        let last = content_last_seen[content];
        if last != pos_idx {
            // This is not the last occurrence — remove it
            to_remove.insert((*mi, *bi));
        } else if content_seen_at.contains_key(content) {
            // This is the last occurrence but we've seen it before — remove earlier ones
            // (already handled above)
            let _ = content_seen_at.insert(content.clone(), pos_idx);
        } else {
            content_seen_at.insert(content.clone(), pos_idx);
        }
    }

    if to_remove.is_empty() {
        return messages.to_vec();
    }

    // Rebuild messages without removed blocks; drop empty user messages
    let mut out = Vec::new();
    for (mi, msg) in messages.iter().enumerate() {
        let new_content: Vec<ContentBlock> = msg
            .content
            .iter()
            .enumerate()
            .filter(|(bi, _)| !to_remove.contains(&(mi, *bi)))
            .map(|(_, b)| b.clone())
            .collect();
        if new_content.is_empty() && msg.role == clido_core::Role::User {
            // Drop empty user messages (they only contained deduplicated ToolResult blocks)
            continue;
        }
        out.push(Message {
            role: msg.role,
            content: new_content,
        });
    }
    out
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
    // Deduplicate repeated file reads before computing token budget
    let messages_cow: std::borrow::Cow<[Message]> = {
        let deduped = dedup_file_reads(messages);
        if deduped.len() != messages.len() {
            std::borrow::Cow::Owned(deduped)
        } else {
            std::borrow::Cow::Borrowed(messages)
        }
    };
    let messages = messages_cow.as_ref();

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
    fn dedup_file_reads_removes_older_duplicates() {
        // Two user messages with ToolResult blocks having identical content
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "id1".to_string(),
                    content: "file content here".to_string(),
                    is_error: false,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "id2".to_string(),
                    content: "file content here".to_string(),
                    is_error: false,
                }],
            },
        ];
        let deduped = dedup_file_reads(&messages);
        // The first ToolResult (id1) should be removed; the last (id2) kept
        let tool_result_count: usize = deduped
            .iter()
            .map(|m| {
                m.content
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
                    .count()
            })
            .sum();
        assert_eq!(tool_result_count, 1, "Should only keep the last duplicate");
        // The assistant message should still be present
        assert!(deduped.iter().any(|m| m.role == Role::Assistant));
    }

    #[test]
    fn dedup_file_reads_keeps_unique_content() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "id1".to_string(),
                    content: "content A".to_string(),
                    is_error: false,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "id2".to_string(),
                    content: "content B".to_string(),
                    is_error: false,
                }],
            },
        ];
        let deduped = dedup_file_reads(&messages);
        assert_eq!(deduped.len(), 2, "Unique content should not be removed");
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
