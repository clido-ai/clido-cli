//! Generic filterable list picker with scrolling and selection.
//!
//! Used by: model picker, session picker, profile picker, role picker,
//! provider picker, and any future list-selection overlay.

use crate::text_input::{handle_text_key, TextInput};

/// Trait that items must implement to be displayed in a `ListPicker`.
pub trait PickerItem {
    /// The primary text used for filtering. Case-insensitive substring match.
    fn filter_text(&self) -> String;

    /// Optional secondary filter text (e.g. provider name, description).
    fn filter_text_secondary(&self) -> Option<String> {
        None
    }
}

/// A generic filterable list picker.
///
/// Manages selection, scrolling, and filtering for any `Vec<T: PickerItem>`.
/// Does NOT handle rendering — that's the overlay's job. The picker only
/// tracks state; overlays read `filtered_indices()`, `selected()`, etc.
#[derive(Debug, Clone)]
pub struct ListPicker<T> {
    items: Vec<T>,
    /// Indices into `items` that match the current filter.
    filtered: Vec<usize>,
    /// Selected index within the *filtered* list (not the source list).
    pub selected: usize,
    /// Scroll offset within the filtered list.
    pub scroll_offset: usize,
    /// Maximum visible rows (set by the overlay based on available height).
    pub visible_rows: usize,
    /// The text filter input.
    pub filter: TextInput,
    /// Whether filtering is enabled for this picker.
    pub filterable: bool,
}

#[allow(dead_code)]
impl<T: PickerItem> ListPicker<T> {
    /// Create a new picker with the given items and visible row count.
    pub fn new(items: Vec<T>, visible_rows: usize) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered,
            selected: 0,
            scroll_offset: 0,
            visible_rows,
            filter: TextInput::new(),
            filterable: true,
        }
    }

    /// Create a picker without filtering (for simple lists).
    pub fn without_filter(items: Vec<T>, visible_rows: usize) -> Self {
        let mut picker = Self::new(items, visible_rows);
        picker.filterable = false;
        picker
    }

    /// Number of items after filtering.
    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    /// Whether the filtered list is empty.
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// Get the currently selected item (if any).
    pub fn selected_item(&self) -> Option<&T> {
        self.filtered
            .get(self.selected)
            .map(|&idx| &self.items[idx])
    }

    /// Get the index into the original `items` vec for the current selection.
    pub fn selected_original_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// Get all items (unfiltered).
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Get a mutable reference to all items.
    pub fn items_mut(&mut self) -> &mut Vec<T> {
        &mut self.items
    }

    /// Get the indices into `items` that pass the current filter.
    pub fn filtered_indices(&self) -> &[usize] {
        &self.filtered
    }

    /// Iterate over filtered items as (original_index, &item).
    pub fn filtered_items(&self) -> impl Iterator<Item = (usize, &T)> {
        self.filtered.iter().map(|&i| (i, &self.items[i]))
    }

    /// Replace all items and reapply filter.
    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.apply_filter();
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Move selection up (wraps to bottom).
    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.filtered.len() - 1;
        } else {
            self.selected -= 1;
        }
        self.ensure_visible();
    }

    /// Move selection down (wraps to top).
    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected + 1 >= self.filtered.len() {
            self.selected = 0;
        } else {
            self.selected += 1;
        }
        self.ensure_visible();
    }

    /// Jump to the first item.
    pub fn home(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Jump to the last item.
    pub fn end(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self.filtered.len() - 1;
            self.ensure_visible();
        }
    }

    /// Move selection up by a page.
    pub fn page_up(&mut self) {
        if self.visible_rows == 0 || self.filtered.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(self.visible_rows);
        self.ensure_visible();
    }

    /// Move selection down by a page.
    pub fn page_down(&mut self) {
        if self.visible_rows == 0 || self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected + self.visible_rows).min(self.filtered.len() - 1);
        self.ensure_visible();
    }

    // ── Filtering ─────────────────────────────────────────────────────

    /// Recompute the filtered list from the current filter text.
    pub fn apply_filter(&mut self) {
        let f = self.filter.text.trim().to_lowercase();
        if f.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.filter_text().to_lowercase().contains(&f)
                        || item
                            .filter_text_secondary()
                            .map(|s| s.to_lowercase().contains(&f))
                            .unwrap_or(false)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.clamp();
    }

    /// Handle a key event. Delegates to filter TextInput for character keys,
    /// handles navigation for arrows. Returns true if the key was consumed.
    ///
    /// Does NOT handle Enter or Esc — the overlay handles those.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Up => {
                self.move_up();
                true
            }
            KeyCode::Down => {
                self.move_down();
                true
            }
            KeyCode::Home => {
                self.home();
                true
            }
            KeyCode::End => {
                self.end();
                true
            }
            KeyCode::PageUp => {
                self.page_up();
                true
            }
            KeyCode::PageDown => {
                self.page_down();
                true
            }
            _ if self.filterable => {
                if handle_text_key(&mut self.filter, key) {
                    self.apply_filter();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    // ── Internal ──────────────────────────────────────────────────────

    /// Adjust scroll_offset so that `selected` is within the visible window.
    fn ensure_visible(&mut self) {
        if self.visible_rows == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.visible_rows {
            self.scroll_offset = self.selected - self.visible_rows + 1;
        }
    }

    /// Clamp selected and scroll after filter changes.
    fn clamp(&mut self) {
        let n = self.filtered.len();
        if n == 0 {
            self.selected = 0;
            self.scroll_offset = 0;
        } else {
            self.selected = self.selected.min(n - 1);
            self.scroll_offset = self.scroll_offset.min(self.selected);
        }
    }

    // ── Scroll indicators ─────────────────────────────────────────────

    /// Number of items above the visible window.
    pub fn items_above(&self) -> usize {
        self.scroll_offset
    }

    /// Number of items below the visible window.
    pub fn items_below(&self) -> usize {
        let total = self.filtered.len();
        let visible_end = self.scroll_offset + self.visible_rows;
        total.saturating_sub(visible_end)
    }

    /// The range of filtered indices currently visible.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let start = self.scroll_offset;
        let end = (self.scroll_offset + self.visible_rows).min(self.filtered.len());
        start..end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct Item {
        name: String,
        category: String,
    }

    impl PickerItem for Item {
        fn filter_text(&self) -> String {
            self.name.clone()
        }
        fn filter_text_secondary(&self) -> Option<String> {
            Some(self.category.clone())
        }
    }

    fn make_items(names: &[&str]) -> Vec<Item> {
        names
            .iter()
            .map(|n| Item {
                name: n.to_string(),
                category: "test".to_string(),
            })
            .collect()
    }

    #[test]
    fn basic_navigation() {
        let mut p = ListPicker::new(make_items(&["a", "b", "c", "d"]), 10);
        assert_eq!(p.selected, 0);
        assert_eq!(p.selected_item().unwrap().name, "a");

        p.move_down();
        assert_eq!(p.selected_item().unwrap().name, "b");

        p.move_down();
        p.move_down();
        assert_eq!(p.selected_item().unwrap().name, "d");

        // Wraps
        p.move_down();
        assert_eq!(p.selected_item().unwrap().name, "a");

        // Wraps up
        p.move_up();
        assert_eq!(p.selected_item().unwrap().name, "d");
    }

    #[test]
    fn filtering() {
        let items = vec![
            Item {
                name: "alpha".into(),
                category: "greek".into(),
            },
            Item {
                name: "beta".into(),
                category: "greek".into(),
            },
            Item {
                name: "gamma".into(),
                category: "greek".into(),
            },
            Item {
                name: "apple".into(),
                category: "fruit".into(),
            },
        ];
        let mut p = ListPicker::new(items, 10);

        // Filter by primary text
        p.filter.set_text("al");
        p.apply_filter();
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.selected_item().unwrap().name, "alpha");

        // Filter by secondary text
        p.filter.set_text("fruit");
        p.apply_filter();
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.selected_item().unwrap().name, "apple");

        // Clear filter shows all
        p.filter.clear();
        p.apply_filter();
        assert_eq!(p.filtered_count(), 4);
    }

    #[test]
    fn filter_clamps_selection() {
        let mut p = ListPicker::new(make_items(&["aa", "ab", "ba", "bb"]), 10);
        p.selected = 3; // on "bb"

        p.filter.set_text("a");
        p.apply_filter();
        // Only "aa", "ab", "ba" pass → selected clamped to 2
        assert_eq!(p.filtered_count(), 3);
        assert!(p.selected <= 2);
    }

    #[test]
    fn scroll_management() {
        let names: Vec<&str> = (0..20)
            .map(|i| match i {
                0 => "item00",
                1 => "item01",
                2 => "item02",
                3 => "item03",
                4 => "item04",
                5 => "item05",
                6 => "item06",
                7 => "item07",
                8 => "item08",
                9 => "item09",
                10 => "item10",
                11 => "item11",
                12 => "item12",
                13 => "item13",
                14 => "item14",
                15 => "item15",
                16 => "item16",
                17 => "item17",
                18 => "item18",
                _ => "item19",
            })
            .collect();
        let mut p = ListPicker::new(make_items(&names), 5);

        // Move down past visible window
        for _ in 0..7 {
            p.move_down();
        }
        assert_eq!(p.selected, 7);
        assert!(p.scroll_offset > 0);
        assert!(p.selected < p.scroll_offset + p.visible_rows);

        // Items above/below
        assert!(p.items_above() > 0);
        assert!(p.items_below() > 0);
    }

    #[test]
    fn home_end() {
        let mut p = ListPicker::new(make_items(&["a", "b", "c"]), 10);
        p.end();
        assert_eq!(p.selected, 2);
        p.home();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn empty_picker() {
        let mut p: ListPicker<Item> = ListPicker::new(vec![], 10);
        assert!(p.selected_item().is_none());
        p.move_up(); // no panic
        p.move_down(); // no panic
        assert_eq!(p.filtered_count(), 0);
    }

    #[test]
    fn without_filter() {
        let mut p = ListPicker::without_filter(make_items(&["a", "b"]), 10);
        assert!(!p.filterable);
        // Navigation still works
        p.move_down();
        assert_eq!(p.selected_item().unwrap().name, "b");
    }

    #[test]
    fn set_items_refilters() {
        let mut p = ListPicker::new(make_items(&["old1", "old2"]), 10);
        p.filter.set_text("new");
        p.apply_filter();
        assert_eq!(p.filtered_count(), 0);

        p.set_items(make_items(&["new1", "new2", "other"]));
        assert_eq!(p.filtered_count(), 2);
    }

    #[test]
    fn page_navigation() {
        let names: Vec<&str> = (0..20).map(|_| "item").collect();
        let mut p = ListPicker::new(make_items(&names), 5);

        p.page_down();
        assert_eq!(p.selected, 5);

        p.page_down();
        assert_eq!(p.selected, 10);

        p.page_up();
        assert_eq!(p.selected, 5);

        p.page_up();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn visible_range() {
        let names: Vec<&str> = (0..10).map(|_| "item").collect();
        let mut p = ListPicker::new(make_items(&names), 3);

        let r = p.visible_range();
        assert_eq!(r, 0..3);

        p.selected = 5;
        p.ensure_visible();
        let r = p.visible_range();
        assert!(r.contains(&5));
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn handle_key_navigation() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut p = ListPicker::new(make_items(&["a", "b", "c"]), 10);

        // Down arrow
        assert!(p.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        assert_eq!(p.selected, 1);

        // Up arrow
        assert!(p.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(p.selected, 0);

        // Character triggers filter
        assert!(p.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)));
        assert_eq!(p.filter.text, "b");
        assert_eq!(p.filtered_count(), 1);

        // Enter not consumed (overlay handles it)
        assert!(!p.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }
}
