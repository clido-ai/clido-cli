#[cfg(test)]
mod unified_renderer_tests {
    use crate::tui::render::{render_chat_to_content_lines, wrap_content_lines};
    use crate::tui::state::{ChatLine, ContentLine, LineSource};

    #[test]
    fn test_render_user_message() {
        let messages = vec![ChatLine::User("Hello world".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "test-model");
        
        // Should have: header + content + blank + extra blank
        assert!(lines.len() >= 3);
        assert_eq!(lines[0].source, LineSource::User);
        assert!(!lines[0].selectable); // Header not selectable
        assert!(lines[1].selectable);  // Content selectable
    }

    #[test]
    fn test_render_assistant_message() {
        let messages = vec![ChatLine::Assistant("Response text".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "claude");
        
        assert!(lines.len() >= 3);
        assert_eq!(lines[0].source, LineSource::Assistant);
    }

    #[test]
    fn test_render_multiple_messages() {
        let messages = vec![
            ChatLine::User("Question?".to_string()),
            ChatLine::Assistant("Answer!".to_string()),
        ];
        let lines = render_chat_to_content_lines(&messages, 80, "model");
        
        // Should have more lines due to multiple messages
        assert!(lines.len() > 4);
    }

    #[test]
    fn test_render_info_message() {
        let messages = vec![ChatLine::Info("Info text".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::Info);
    }

    #[test]
    fn test_render_tool_call() {
        let messages = vec![ChatLine::ToolCall {
            tool_use_id: "1".to_string(),
            name: "Bash".to_string(),
            detail: "ls -la".to_string(),
            done: true,
            is_error: false,
        }];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::ToolCall);
    }

    #[test]
    fn test_render_diff() {
        let diff_text = "+ added line\n- removed line".to_string();
        let messages = vec![ChatLine::Diff(diff_text)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.len() >= 3); // Header + 2 diff lines
    }

    #[test]
    fn test_render_slash_command() {
        let messages = vec![ChatLine::SlashCommand {
            cmd: "/plan".to_string(),
            text: Some("Do something".to_string()),
        }];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::User);
    }
}

#[cfg(test)]
mod wrapped_line_tests {
    use crate::tui::render::wrap_content_lines;
    use crate::tui::state::{ContentLine, LineSource};
    use ratatui::text::Span;

    #[test]
    fn test_wrap_short_line() {
        let content_lines = vec![ContentLine::new(
            vec![Span::raw("Short text")],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 80);
        assert_eq!(wrapped.len(), 1);
    }

    #[test]
    fn test_wrap_long_line() {
        let long_text = "a".repeat(100);
        let content_lines = vec![ContentLine::new(
            vec![Span::raw(long_text)],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 40);
        assert!(wrapped.len() > 1); // Should wrap to multiple lines
    }

    #[test]
    fn test_wrap_preserves_attributes() {
        let content_lines = vec![ContentLine::new(
            vec![Span::raw("Test")],
            LineSource::Assistant,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 80);
        assert_eq!(wrapped[0].source, LineSource::Assistant);
        assert!(wrapped[0].selectable);
        assert_eq!(wrapped[0].msg_idx, 0);
    }

    #[test]
    fn test_wrap_multiple_lines() {
        let content_lines = vec![
            ContentLine::new(vec![Span::raw("Line 1")], LineSource::User, true, 0),
            ContentLine::new(vec![Span::raw("Line 2")], LineSource::Assistant, true, 1),
        ];
        
        let wrapped = wrap_content_lines(&content_lines, 80);
        assert!(wrapped.len() >= 2);
    }
}
