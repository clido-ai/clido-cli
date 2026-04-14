#[cfg(test)]
mod unified_renderer_tests {
    use crate::tui::render::{render_chat_to_content_lines, wrap_content_lines};
    use crate::tui::state::{ChatLine, ContentLine, LineSource};

    #[test]
    fn test_render_user_message() {
        let messages = vec![ChatLine::User("Hello world".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "test-model");
        
        assert!(lines.len() >= 3);
        assert_eq!(lines[0].source, LineSource::User);
        assert!(!lines[0].selectable);
        assert!(lines[1].selectable);
    }

    #[test]
    fn test_render_assistant_message() {
        let messages = vec![ChatLine::Assistant("Response text".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "claude");
        
        assert!(lines.len() >= 3);
        assert_eq!(lines[0].source, LineSource::Assistant);
    }

    #[test]
    fn test_render_thinking_message() {
        let messages = vec![ChatLine::Thinking("Thinking...".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::Thinking);
    }

    #[test]
    fn test_render_info_message() {
        let messages = vec![ChatLine::Info("Info text".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::Info);
    }

    #[test]
    fn test_render_section_message() {
        let messages = vec![ChatLine::Section("Section Header".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
        assert_eq!(lines[0].source, LineSource::Section);
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
        
        assert!(lines.len() >= 3);
        assert_eq!(lines[0].source, LineSource::Diff);
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

    #[test]
    fn test_render_empty_messages() {
        let messages: Vec<ChatLine> = vec![];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.is_empty());
    }

    #[test]
    fn test_render_multiple_messages() {
        let messages = vec![
            ChatLine::User("Question?".to_string()),
            ChatLine::Assistant("Answer!".to_string()),
            ChatLine::Info("Note".to_string()),
        ];
        let lines = render_chat_to_content_lines(&messages, 80, "model");
        
        assert!(lines.len() > 6);
    }

    #[test]
    fn test_render_long_message_wrapping() {
        let long_text = "a".repeat(200);
        let messages = vec![ChatLine::User(long_text)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        // Should create multiple content lines due to wrapping
        assert!(lines.len() > 3);
    }

    #[test]
    fn test_render_markdown_formatting() {
        let md_text = "# Heading\n\n**bold** and *italic*".to_string();
        let messages = vec![ChatLine::Assistant(md_text)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.len() > 3);
    }

    #[test]
    fn test_render_code_block() {
        let code = "```rust\nfn main() {}\n```".to_string();
        let messages = vec![ChatLine::Assistant(code)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_render_list_items() {
        let list = "- Item 1\n- Item 2\n- Item 3".to_string();
        let messages = vec![ChatLine::Assistant(list)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.len() >= 4);
    }

    #[test]
    fn test_render_table() {
        let table = "| Col1 | Col2 |\n|------|------|\n| A    | B    |".to_string();
        let messages = vec![ChatLine::Assistant(table)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
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
        assert!(wrapped.len() > 1);
    }

    #[test]
    fn test_wrap_exact_width() {
        let text = "a".repeat(80);
        let content_lines = vec![ContentLine::new(
            vec![Span::raw(text)],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 80);
        assert_eq!(wrapped.len(), 1);
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

    #[test]
    fn test_wrap_unicode() {
        let unicode_text = "日本語のテキスト".to_string();
        let content_lines = vec![ContentLine::new(
            vec![Span::raw(unicode_text)],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 10);
        assert!(!wrapped.is_empty());
    }

    #[test]
    fn test_wrap_empty_line() {
        let content_lines = vec![ContentLine::new(
            vec![],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 80);
        // Empty lines might be skipped or produce empty wrapped line
        assert!(wrapped.is_empty() || wrapped[0].plain_text().is_empty());
    }

    #[test]
    fn test_wrap_tracks_char_offset() {
        let text = "Hello world this is a test";
        let content_lines = vec![ContentLine::new(
            vec![Span::raw(text)],
            LineSource::User,
            true,
            0,
        )];
        
        let wrapped = wrap_content_lines(&content_lines, 10);
        assert!(wrapped.len() > 1);
        // Each wrapped line should have increasing char_offset
        if wrapped.len() >= 2 {
            assert!(wrapped[1].char_offset > wrapped[0].char_offset);
        }
    }
}

#[cfg(test)]
mod edge_case_tests {
    use crate::tui::render::render_chat_to_content_lines;
    use crate::tui::state::ChatLine;

    #[test]
    fn test_render_empty_string() {
        let messages = vec![ChatLine::User("".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        // Should still have header and blank line
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_whitespace_only() {
        let messages = vec![ChatLine::User("   ".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_newlines_only() {
        let messages = vec![ChatLine::User("\n\n\n".to_string())];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_very_long_single_word() {
        let word = "a".repeat(1000);
        let messages = vec![ChatLine::User(word)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(lines.len() > 10);
    }

    #[test]
    fn test_render_special_chars() {
        let special = "<>&\"'\n\t\r".to_string();
        let messages = vec![ChatLine::User(special)];
        let lines = render_chat_to_content_lines(&messages, 80, "");
        
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_zero_width() {
        let messages = vec![ChatLine::User("test".to_string())];
        let lines = render_chat_to_content_lines(&messages, 0, "");
        
        // Should handle gracefully
        assert!(!lines.is_empty());
    }
}
