//! Search state for finding and navigating matches in TUI content.
//!
//! The actual search-over-iteration logic lives on `TuiState` (see
//! `tui_state.rs`); this module holds only the value type that stores
//! the query, the match list, and the cursor across those matches.

/// Search state for finding and navigating matches in TUI content.
/// Tracks the current query, match positions, and navigation index.
#[derive(Debug, Default)]
pub struct SearchState {
    /// Current search query (None when no active search).
    pub query: Option<String>,
    /// Match positions as (line_index, char_offset) pairs.
    pub matches: Vec<(usize, usize)>,
    /// Index into matches vector for current match.
    pub current_match: usize,
    /// Whether search input mode is active (user is typing query).
    pub search_mode: bool,
}

impl SearchState {
    /// Creates a new empty search state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all search state.
    pub fn clear(&mut self) {
        self.query = None;
        self.matches.clear();
        self.current_match = 0;
        self.search_mode = false;
    }
}
