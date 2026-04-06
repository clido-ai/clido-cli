//! Session history reconstruction and repair.

use clido_core::{ClidoError, ContentBlock, Message, Role};
use clido_storage::SessionLine;

/// Serialize assistant/user content blocks for session JSONL. Fails closed (no silent drops).
pub fn content_blocks_to_json_values(
    blocks: &[ContentBlock],
) -> Result<Vec<serde_json::Value>, serde_json::Error> {
    blocks.iter().map(serde_json::to_value).collect()
}

/// Strict load: every content JSON value must decode to a [`ContentBlock`].
pub fn try_session_lines_to_messages(lines: &[SessionLine]) -> Result<Vec<Message>, ClidoError> {
    let mut messages = Vec::new();
    let mut tool_result_buf: Vec<ContentBlock> = Vec::new();

    let flush_tool_results = |msgs: &mut Vec<Message>, buf: &mut Vec<ContentBlock>| {
        if !buf.is_empty() {
            msgs.push(Message {
                role: Role::User,
                content: std::mem::take(buf),
            });
        }
    };

    for (line_idx, line) in lines.iter().enumerate() {
        match line {
            SessionLine::UserMessage { content, .. } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let mut blocks = Vec::with_capacity(content.len());
                for (i, v) in content.iter().enumerate() {
                    let b: ContentBlock = serde_json::from_value(v.clone()).map_err(|e| {
                        ClidoError::SessionLoadInvalid {
                            message: format!("line {line_idx} user content[{i}]: {e}"),
                        }
                    })?;
                    blocks.push(b);
                }
                messages.push(Message {
                    role: Role::User,
                    content: blocks,
                });
            }
            SessionLine::AssistantMessage { content } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let mut blocks = Vec::with_capacity(content.len());
                for (i, v) in content.iter().enumerate() {
                    let b: ContentBlock = serde_json::from_value(v.clone()).map_err(|e| {
                        ClidoError::SessionLoadInvalid {
                            message: format!("line {line_idx} assistant content[{i}]: {e}"),
                        }
                    })?;
                    blocks.push(b);
                }
                messages.push(Message {
                    role: Role::Assistant,
                    content: blocks,
                });
            }
            SessionLine::ToolCall { .. } => {}
            SessionLine::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                tool_result_buf.push(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                });
            }
            _ => {}
        }
    }
    flush_tool_results(&mut messages, &mut tool_result_buf);

    repair_orphaned_tool_calls(&mut messages);

    Ok(messages)
}

/// Best-effort load (drops invalid content values).
///
/// **Do not use for resume, verify, or any path where silent data loss is unacceptable.**
/// Use [`try_session_lines_to_messages`] for production resume and `clido sessions verify`.
pub fn session_lines_to_messages(lines: &[SessionLine]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut tool_result_buf: Vec<ContentBlock> = Vec::new();

    let flush_tool_results = |msgs: &mut Vec<Message>, buf: &mut Vec<ContentBlock>| {
        if !buf.is_empty() {
            msgs.push(Message {
                role: Role::User,
                content: std::mem::take(buf),
            });
        }
    };

    for line in lines {
        match line {
            SessionLine::UserMessage { content, .. } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let content: Vec<ContentBlock> = content
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                messages.push(Message {
                    role: Role::User,
                    content,
                });
            }
            SessionLine::AssistantMessage { content } => {
                flush_tool_results(&mut messages, &mut tool_result_buf);
                let content: Vec<ContentBlock> = content
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
                messages.push(Message {
                    role: Role::Assistant,
                    content,
                });
            }
            SessionLine::ToolCall { .. } => {}
            SessionLine::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                tool_result_buf.push(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                });
            }
            _ => {}
        }
    }
    flush_tool_results(&mut messages, &mut tool_result_buf);

    repair_orphaned_tool_calls(&mut messages);

    messages
}

/// Ensure every Assistant ToolUse has a matching User ToolResult.
pub(crate) fn repair_orphaned_tool_calls(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != Role::Assistant {
            i += 1;
            continue;
        }
        let tool_use_ids: Vec<String> = messages[i]
            .content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, .. } = b {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        let mut answered: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if i + 1 < messages.len() && messages[i + 1].role == Role::User {
            for b in &messages[i + 1].content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                    answered.insert(tool_use_id.as_str());
                }
            }
        }

        let missing: Vec<&String> = tool_use_ids
            .iter()
            .filter(|id| !answered.contains(id.as_str()))
            .collect();

        if !missing.is_empty() {
            let synthetic: Vec<ContentBlock> = missing
                .iter()
                .map(|id| ContentBlock::ToolResult {
                    tool_use_id: (*id).clone(),
                    content: "[interrupted — tool execution did not complete]".to_string(),
                    is_error: true,
                })
                .collect();

            if i + 1 < messages.len()
                && messages[i + 1].role == Role::User
                && messages[i + 1]
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            {
                messages[i + 1].content.extend(synthetic);
            } else {
                messages.insert(
                    i + 1,
                    Message {
                        role: Role::User,
                        content: synthetic,
                    },
                );
            }
        }
        i += 1;
    }
}
