//! Content storage for a single Ralph iteration.
//!
//! Each iteration in the TUI has its own `IterationBuffer` with
//! independent scroll state and metadata. The iteration-management
//! methods on `TuiState` (start, finish, navigate between iterations)
//! live in `tui_state.rs`.

use super::wave::WaveInfo;
use ratatui::text::Line;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Stores formatted output content for a single Ralph iteration.
/// Each iteration has its own buffer with independent scroll state.
///
/// The `lines` field is wrapped in `Arc<Mutex<>>` to allow sharing
/// with stream handlers during execution, enabling real-time streaming
/// to the TUI instead of batch transfer after execution completes.
pub struct IterationBuffer {
    /// Iteration number (1-indexed for display)
    pub number: u32,
    /// Formatted lines of output (shared for streaming)
    pub lines: Arc<Mutex<Vec<Line<'static>>>>,
    /// Scroll position within this buffer
    pub scroll_offset: usize,
    /// Whether to auto-scroll to bottom as new content arrives.
    /// Starts true, becomes false when user scrolls up, restored when user
    /// scrolls to bottom (G key) or manually scrolls down to reach bottom.
    pub following_bottom: bool,
    /// Hat display name (emoji + name) for this iteration.
    pub hat_display: Option<String>,
    /// Backend used for this iteration (e.g., "claude", "kiro").
    pub backend: Option<String>,
    /// When this iteration started (for elapsed time calculation).
    pub started_at: Option<Instant>,
    /// Frozen elapsed duration for this iteration (set when completed).
    pub elapsed: Option<Duration>,
    /// Wave data associated with this iteration (stored on wave completion).
    pub wave_info: Option<WaveInfo>,
}

impl IterationBuffer {
    /// Creates a new buffer for the given iteration number.
    pub fn new(number: u32) -> Self {
        Self {
            number,
            lines: Arc::new(Mutex::new(Vec::new())),
            scroll_offset: 0,
            following_bottom: true, // Start following bottom for auto-scroll
            hat_display: None,
            backend: None,
            started_at: None,
            elapsed: None,
            wave_info: None,
        }
    }

    /// Returns a shared handle to the lines buffer for streaming.
    ///
    /// This allows stream handlers to write directly to the buffer,
    /// enabling real-time streaming to the TUI.
    pub fn lines_handle(&self) -> Arc<Mutex<Vec<Line<'static>>>> {
        Arc::clone(&self.lines)
    }

    /// Appends a line to the buffer.
    pub fn append_line(&mut self, line: Line<'static>) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(line);
        }
    }

    /// Returns the total number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.lines.lock().map(|l| l.len()).unwrap_or(0)
    }

    /// Returns a clone of the visible lines based on scroll offset and viewport height.
    ///
    /// Note: Returns owned Vec instead of slice due to interior mutability.
    pub fn visible_lines(&self, viewport_height: usize) -> Vec<Line<'static>> {
        let Ok(lines) = self.lines.lock() else {
            return Vec::new();
        };
        if lines.is_empty() {
            return Vec::new();
        }
        let start = self.scroll_offset.min(lines.len());
        let end = (start + viewport_height).min(lines.len());
        lines[start..end].to_vec()
    }

    /// Scrolls up by one line.
    /// Disables auto-scroll since user is moving away from bottom.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.following_bottom = false;
    }

    /// Scrolls down by one line, respecting the viewport bounds.
    /// Re-enables auto-scroll if user reaches the bottom.
    pub fn scroll_down(&mut self, viewport_height: usize) {
        let max_scroll = self.max_scroll_offset(viewport_height);
        if self.scroll_offset < max_scroll {
            self.scroll_offset += 1;
        }
        // Re-enable following if user scrolled to or past the bottom
        if self.scroll_offset >= max_scroll {
            self.following_bottom = true;
        }
    }

    /// Scrolls to the top of the buffer.
    /// Disables auto-scroll since user is moving away from bottom.
    pub fn scroll_top(&mut self) {
        self.scroll_offset = 0;
        self.following_bottom = false;
    }

    /// Scrolls to the bottom of the buffer.
    /// Re-enables auto-scroll since user explicitly went to bottom.
    pub fn scroll_bottom(&mut self, viewport_height: usize) {
        self.scroll_offset = self.max_scroll_offset(viewport_height);
        self.following_bottom = true;
    }

    /// Calculates the maximum scroll offset for the given viewport height.
    fn max_scroll_offset(&self, viewport_height: usize) -> usize {
        self.lines
            .lock()
            .map(|l| l.len().saturating_sub(viewport_height))
            .unwrap_or(0)
    }
}
