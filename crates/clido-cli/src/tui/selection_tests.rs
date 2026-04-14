#[cfg(test)]
mod selection_tests {
    use crate::tui::state::{Selection, ContentLine, LineSource, WrappedLine};
    use ratatui::text::Span;

    #[test]
    fn test_selection_bounds() {
        let mut sel = Selection::default();
        sel.start(0, 0);
        sel.update(1, 5);
        
        let (sr, sc, er, ec) = sel.bounds();
        assert_eq!(sr, 0);
        assert_eq!(sc, 0);
        assert_eq!(er, 1);
        assert_eq!(ec, 5);
    }

    #[test]
    fn test_selection_reverse() {
        let mut sel = Selection::default();
        sel.start(1, 5);
        sel.update(0, 0);
        
        let (sr, sc, er, ec) = sel.bounds();
        // Should normalize so start <= end
        assert_eq!(sr, 0);
        assert_eq!(sc, 0);
        assert_eq!(er, 1);
        assert_eq!(ec, 5);
    }

    #[test]
    fn test_selection_clear() {
        let mut sel = Selection::default();
        sel.start(0, 0);
        sel.update(1, 5);
        sel.clear();
        
        assert!(!sel.active);
    }

    #[test]
    fn test_selection_single_line() {
        let mut sel = Selection::default();
        sel.start(0, 2);
        sel.update(0, 8);
        
        let (sr, sc, er, ec) = sel.bounds();
        assert_eq!(sr, 0);
        assert_eq!(er, 0);
        assert_eq!(sc, 2);
        assert_eq!(ec, 8);
    }
}

#[cfg(test)]
mod wrapped_selection_tests {
    use crate::tui::app_state::AppState;
    use crate::tui::state::{WrappedLine, LineSource};
    use ratatui::text::Span;

    fn create_test_app_with_wrapped_lines() -> AppState {
        let mut app = AppState::default();
        app.wrapped_lines = vec![
            WrappedLine::new(
                vec![Span::raw("Hello world")],
                LineSource::User,
                true,
                0,
                0,
                0,
            ),
            WrappedLine::new(
                vec![Span::raw("Second line")],
                LineSource::User,
                true,
                0,
                0,
                11,
            ),
        ];
        app
    }

    #[test]
    fn test_get_selected_text_single_line() {
        let mut app = create_test_app_with_wrapped_lines();
        app.selection.start(0, 0);
        app.selection.update(0, 5);
        app.selection.active = true;
        
        let text = app.get_selected_text();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_get_selected_text_multi_line() {
        let mut app = create_test_app_with_wrapped_lines();
        app.selection.start(0, 6);
        app.selection.update(1, 4);
        app.selection.active = true;
        
        let text = app.get_selected_text();
        assert_eq!(text, "world\nSecond");
    }

    #[test]
    fn test_get_selected_text_inactive() {
        let app = create_test_app_with_wrapped_lines();
        let text = app.get_selected_text();
        assert!(text.is_empty());
    }
}
