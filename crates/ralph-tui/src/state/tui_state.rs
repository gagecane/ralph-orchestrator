//! The central `TuiState` struct and all of its methods.
//!
//! This is the big one. It holds every piece of observable state the
//! TUI renders, applies `Event` updates from the orchestration loop,
//! and exposes iteration/search/guidance/wave-view navigation helpers.
//!
//! The value types it composes (tasks, search, guidance, waves, iteration
//! buffers, update status) live in sibling modules under `state::`.

use super::guidance::{GuidanceMode, GuidanceResult};
use super::iteration::IterationBuffer;
use super::search::SearchState;
use super::task::{TaskCounts, TaskSummary};
use super::update::UpdateStatus;
use super::wave::WaveInfo;

use ralph_proto::{Event, HatId};
use ratatui::text::Line;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Observable state derived from loop events.
pub struct TuiState {
    /// Which hat will process next event (ID + display name).
    pub pending_hat: Option<(HatId, String)>,
    /// Backend expected for the next iteration (used when metadata is missing).
    pub pending_backend: Option<String>,
    /// Current iteration number (0-indexed, display as +1).
    pub iteration: u32,
    /// Previous iteration number (for detecting changes).
    pub prev_iteration: u32,
    /// When loop began.
    pub loop_started: Option<Instant>,
    /// When current iteration began.
    pub iteration_started: Option<Instant>,
    /// Most recent event topic.
    pub last_event: Option<String>,
    /// Timestamp of last event.
    pub last_event_at: Option<Instant>,
    /// Whether to show help overlay.
    pub show_help: bool,
    /// Whether mouse capture is enabled for wheel scrolling.
    /// When false, the terminal keeps native drag-to-select behavior.
    pub mouse_capture_enabled: bool,
    /// Whether in scroll mode.
    pub in_scroll_mode: bool,
    /// Current search query (if in search input mode).
    pub search_query: String,
    /// Search direction (true = forward, false = backward).
    pub search_forward: bool,
    /// Maximum iterations from config.
    pub max_iterations: Option<u32>,
    /// Idle timeout countdown.
    pub idle_timeout_remaining: Option<Duration>,
    /// Status of the asynchronous update check.
    pub update_status: UpdateStatus,
    /// Git branch for the workspace the TUI was launched from.
    current_branch: Option<String>,
    /// Map of event topics to hat display information (for custom hats).
    /// Key: event topic (e.g., "review.security")
    /// Value: (HatId, display name including emoji)
    hat_map: HashMap<String, (HatId, String)>,

    // ========================================================================
    // Iteration Management (new fields for TUI refactor)
    // ========================================================================
    /// Content buffers for each iteration.
    pub iterations: Vec<IterationBuffer>,
    /// Index of the iteration currently being viewed (0-indexed).
    pub current_view: usize,
    /// Whether to automatically follow the latest iteration.
    pub following_latest: bool,
    /// Alert about a new iteration (shown when viewing history and new iteration arrives).
    /// Contains the iteration number to alert about. Cleared when navigating to latest.
    pub new_iteration_alert: Option<usize>,

    // ========================================================================
    // Search State
    // ========================================================================
    /// Search state for finding and navigating matches in iteration content.
    pub search_state: SearchState,

    // ========================================================================
    // Completion State
    // ========================================================================
    /// Whether the loop has completed (received loop.terminate event).
    pub loop_completed: bool,
    /// Frozen elapsed time when loop completed (timer stops at this value).
    pub final_iteration_elapsed: Option<Duration>,
    /// Frozen total elapsed time when loop completed (footer timer stops).
    pub final_loop_elapsed: Option<Duration>,

    // ========================================================================
    // Task Tracking State
    // ========================================================================
    /// Aggregate task counts for display in TUI widgets.
    pub task_counts: TaskCounts,
    /// Currently active task (if any) for display in TUI widgets.
    pub active_task: Option<TaskSummary>,

    // ========================================================================
    // Wave State
    // ========================================================================
    /// Active wave info for header display (only set while a wave is running).
    pub wave_active: Option<WaveInfo>,
    /// Index into `iterations` that the active wave belongs to.
    /// Used by `wave_info_for_view()` to only return `wave_active` when viewing
    /// the specific iteration that owns the running wave.
    pub wave_active_iteration_idx: Option<usize>,
    /// Whether the wave worker drill-down view is active.
    pub wave_view_active: bool,
    /// Index of the worker currently being viewed in wave view (0-indexed).
    pub wave_view_index: usize,

    // ========================================================================
    // Guidance State
    // ========================================================================
    /// Active guidance input mode (None when not entering guidance).
    pub guidance_mode: Option<GuidanceMode>,
    /// Text being typed in guidance input.
    pub guidance_input: String,
    /// Queue of guidance messages for the next iteration (drained by loop_runner).
    pub guidance_next_queue: Arc<Mutex<Vec<String>>>,
    /// Path to events.jsonl for writing urgent guidance for the next prompt.
    pub events_path: Option<std::path::PathBuf>,
    /// Path to the urgent-steer marker file used to gate `ralph emit`.
    pub urgent_steer_path: Option<std::path::PathBuf>,
    /// Brief flash message after attempting to send guidance.
    /// (mode, result, when)
    pub guidance_flash: Option<(GuidanceMode, GuidanceResult, Instant)>,

    // ========================================================================
    // Subprocess Error State
    // ========================================================================
    /// Error message set when subprocess exits before sending any RPC events.
    /// When set, the TUI displays an error state instead of empty content.
    pub subprocess_error: Option<String>,

    // ========================================================================
    // RPC Text Accumulation State
    // ========================================================================
    /// Buffer for accumulating streaming text deltas received via RPC.
    /// Text is rendered as a group when frozen (on tool call, error, or iteration end)
    /// rather than rendering each small delta independently.
    pub rpc_text_buffer: String,
    /// Number of lines in the current iteration buffer that belong to the
    /// current (unfrozen) text. When new text arrives, these lines are
    /// replaced with a fresh render of the full accumulated text.
    pub rpc_text_line_count: usize,
}

impl TuiState {
    /// Creates empty state. Timer starts immediately at creation.
    pub fn new() -> Self {
        Self {
            pending_hat: None,
            pending_backend: None,
            iteration: 0,
            prev_iteration: 0,
            loop_started: Some(Instant::now()),
            iteration_started: None,
            last_event: None,
            last_event_at: None,
            show_help: false,
            mouse_capture_enabled: false,
            in_scroll_mode: false,
            search_query: String::new(),
            search_forward: true,
            max_iterations: None,
            idle_timeout_remaining: None,
            update_status: UpdateStatus::Unknown,
            current_branch: None,
            hat_map: HashMap::new(),
            // Iteration management
            iterations: Vec::new(),
            current_view: 0,
            following_latest: true,
            new_iteration_alert: None,
            // Search state
            search_state: SearchState::new(),
            // Completion state
            loop_completed: false,
            final_iteration_elapsed: None,
            final_loop_elapsed: None,
            // Task tracking state
            task_counts: TaskCounts::default(),
            active_task: None,
            // Wave state
            wave_active: None,
            wave_active_iteration_idx: None,
            wave_view_active: false,
            wave_view_index: 0,
            // Guidance state
            guidance_mode: None,
            guidance_input: String::new(),
            guidance_next_queue: Arc::new(Mutex::new(Vec::new())),
            events_path: None,
            urgent_steer_path: None,
            guidance_flash: None,
            // Subprocess error state
            subprocess_error: None,
            // RPC text accumulation state
            rpc_text_buffer: String::new(),
            rpc_text_line_count: 0,
        }
    }

    /// Creates state with a custom hat map for dynamic topic-to-hat resolution.
    /// Timer starts immediately at creation.
    pub fn with_hat_map(hat_map: HashMap<String, (HatId, String)>) -> Self {
        let mut state = Self::new();
        state.hat_map = hat_map;
        state
    }

    /// Sets the git branch displayed by the TUI.
    pub fn set_current_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    /// Returns the git branch displayed by the TUI, if known.
    pub fn current_branch(&self) -> Option<&str> {
        self.current_branch.as_deref()
    }

    /// Replaces the dynamic topic-to-hat mapping without resetting the rest of the state.
    pub fn set_hat_map(&mut self, hat_map: HashMap<String, (HatId, String)>) {
        self.hat_map = hat_map;
    }

    /// Updates state based on event topic.
    pub fn update(&mut self, event: &Event) {
        let now = Instant::now();
        let topic = event.topic.as_str();

        self.last_event = Some(topic.to_string());
        self.last_event_at = Some(now);

        let custom_hat = self.hat_map.get(topic).cloned();
        if let Some((hat_id, hat_display)) = custom_hat.clone() {
            self.pending_hat = Some((hat_id, hat_display));
            // Handle iteration timing for custom hats
            if topic.starts_with("build.") {
                self.iteration_started = Some(now);
            }
        }

        // Fall back to hardcoded mappings for backward compatibility
        match topic {
            "task.start" => {
                // Save state we want to preserve across reset
                let saved_hat_map = std::mem::take(&mut self.hat_map);
                let saved_loop_started = self.loop_started; // Preserve timer from TUI init
                let saved_max_iterations = self.max_iterations;
                // Preserve iteration buffers so TUI history survives across task restarts
                let saved_iterations = std::mem::take(&mut self.iterations);
                let saved_current_view = self.current_view;
                let saved_following_latest = self.following_latest;
                let saved_new_iteration_alert = self.new_iteration_alert.take();
                let saved_pending_backend = self.pending_backend.clone();
                let saved_current_branch = self.current_branch.clone();
                let saved_guidance_next_queue = Arc::clone(&self.guidance_next_queue);
                let saved_events_path = self.events_path.clone();
                let saved_urgent_steer_path = self.urgent_steer_path.clone();
                *self = Self::new();
                self.hat_map = saved_hat_map;
                self.loop_started = saved_loop_started; // Keep original timer
                self.max_iterations = saved_max_iterations;
                self.iterations = saved_iterations;
                self.current_view = saved_current_view;
                self.following_latest = saved_following_latest;
                self.new_iteration_alert = saved_new_iteration_alert;
                self.pending_backend = saved_pending_backend;
                self.current_branch = saved_current_branch;
                self.guidance_next_queue = saved_guidance_next_queue;
                self.events_path = saved_events_path;
                self.urgent_steer_path = saved_urgent_steer_path;
                if let Some((hat_id, hat_display)) = custom_hat {
                    self.pending_hat = Some((hat_id, hat_display));
                } else {
                    self.pending_hat = Some((HatId::new("planner"), "📋Planner".to_string()));
                }
                self.last_event = Some(topic.to_string());
                self.last_event_at = Some(now);
            }
            "task.resume" => {
                // Don't reset timer on resume - keep counting from TUI init
                if custom_hat.is_none() {
                    self.pending_hat = Some((HatId::new("planner"), "📋Planner".to_string()));
                }
            }
            "build.task" => {
                if custom_hat.is_none() {
                    self.pending_hat = Some((HatId::new("builder"), "🔨Builder".to_string()));
                }
                // Resume the loop timer if a new iteration starts after a freeze.
                self.final_loop_elapsed = None;
                self.iteration_started = Some(now);
            }
            "build.done" => {
                if custom_hat.is_none() {
                    self.pending_hat = Some((HatId::new("planner"), "📋Planner".to_string()));
                }
                self.prev_iteration = self.iteration;
                self.iteration += 1;
                self.finish_latest_iteration();
                self.freeze_loop_elapsed();
            }
            "build.blocked" => {
                if custom_hat.is_none() {
                    self.pending_hat = Some((HatId::new("planner"), "📋Planner".to_string()));
                }
                self.finish_latest_iteration();
                self.freeze_loop_elapsed();
            }
            "loop.terminate" => {
                self.pending_hat = None;
                self.loop_completed = true;
                // Freeze the iteration timer at its current value
                self.final_iteration_elapsed = self.iteration_started.map(|start| start.elapsed());
                // Freeze the total loop timer for the footer display
                self.freeze_loop_elapsed();
                self.finish_latest_iteration();
            }
            _ => {
                // Unknown topic - don't change pending_hat
            }
        }
    }

    /// Returns formatted hat display (emoji + name).
    pub fn get_pending_hat_display(&self) -> String {
        self.pending_hat
            .as_ref()
            .map_or_else(|| "—".to_string(), |(_, display)| display.clone())
    }

    /// Time since loop started.
    pub fn get_loop_elapsed(&self) -> Option<Duration> {
        if let Some(final_elapsed) = self.final_loop_elapsed {
            return Some(final_elapsed);
        }
        self.loop_started.map(|start| start.elapsed())
    }

    /// Time since iteration started, or frozen value if loop completed.
    pub fn get_iteration_elapsed(&self) -> Option<Duration> {
        if let Some(buffer) = self.current_iteration() {
            if let Some(elapsed) = buffer.elapsed {
                return Some(elapsed);
            }
            if let Some(started_at) = buffer.started_at {
                return Some(started_at.elapsed());
            }
        }
        if let Some(final_elapsed) = self.final_iteration_elapsed {
            return Some(final_elapsed);
        }
        self.iteration_started.map(|start| start.elapsed())
    }

    /// True if event received in last 2 seconds.
    pub fn is_active(&self) -> bool {
        self.last_event_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
    }

    /// True if iteration changed since last check.
    pub fn iteration_changed(&self) -> bool {
        self.iteration != self.prev_iteration
    }

    // ========================================================================
    // Task Tracking Methods
    // ========================================================================

    /// Returns a reference to the current task counts.
    pub fn get_task_counts(&self) -> &TaskCounts {
        &self.task_counts
    }

    /// Returns a reference to the active task, if any.
    pub fn get_active_task(&self) -> Option<&TaskSummary> {
        self.active_task.as_ref()
    }

    /// Updates the task counts.
    pub fn set_task_counts(&mut self, counts: TaskCounts) {
        self.task_counts = counts;
    }

    /// Sets the active task.
    pub fn set_active_task(&mut self, task: Option<TaskSummary>) {
        self.active_task = task;
    }

    /// Returns true if there are any open tasks.
    pub fn has_open_tasks(&self) -> bool {
        self.task_counts.open > 0
    }

    /// Returns a formatted string for task progress display (e.g., "3/5 tasks").
    pub fn get_task_progress_display(&self) -> String {
        if self.task_counts.total == 0 {
            "No tasks".to_string()
        } else {
            format!(
                "{}/{} tasks",
                self.task_counts.closed, self.task_counts.total
            )
        }
    }

    // ========================================================================
    // Iteration Management Methods
    // ========================================================================

    /// Starts a new iteration, creating a new IterationBuffer.
    /// If following_latest is true, current_view is updated to the new iteration.
    /// If not following, sets the new_iteration_alert to notify the user.
    pub fn start_new_iteration(&mut self) {
        self.start_new_iteration_with_metadata(None, None);
    }

    /// Starts a new iteration with optional metadata for hat and backend display.
    pub fn start_new_iteration_with_metadata(
        &mut self,
        hat_display: Option<String>,
        backend: Option<String>,
    ) {
        // Reset text accumulation buffer for the new iteration
        self.rpc_text_buffer.clear();
        self.rpc_text_line_count = 0;

        // Exit wave view on new iteration — the user can re-enter with 'w'.
        // Wave data is preserved per-iteration on the IterationBuffer, so users
        // can navigate to any iteration and press 'w' to review its wave workers.
        self.wave_view_active = false;

        // Resume the total loop timer — it gets frozen on build.done/build.blocked
        // but should keep ticking when the next iteration starts.
        self.final_loop_elapsed = None;

        let hat_display = hat_display.or_else(|| {
            self.pending_hat
                .as_ref()
                .map(|(_, display)| display.clone())
        });
        let backend = backend.or_else(|| self.pending_backend.clone());
        let number = (self.iterations.len() + 1) as u32;
        let mut buffer = IterationBuffer::new(number);
        buffer.hat_display = hat_display;
        buffer.backend = backend;
        buffer.started_at = Some(Instant::now());
        if buffer.backend.is_some() {
            self.pending_backend = buffer.backend.clone();
        }
        self.iterations.push(buffer);

        // Auto-follow if enabled
        if self.following_latest {
            self.current_view = self.iterations.len().saturating_sub(1);
        } else {
            // Alert user about new iteration when reviewing history
            self.new_iteration_alert = Some(number as usize);
        }
    }

    /// Finalizes the latest iteration's elapsed time if it isn't already set.
    pub fn finish_latest_iteration(&mut self) {
        let Some(buffer) = self.iterations.last_mut() else {
            return;
        };
        if buffer.elapsed.is_some() {
            return;
        }
        if let Some(started_at) = buffer.started_at {
            buffer.elapsed = Some(started_at.elapsed());
        }
    }

    /// Freeze total loop elapsed time for the footer if it is still ticking.
    fn freeze_loop_elapsed(&mut self) {
        if self.final_loop_elapsed.is_some() {
            return;
        }
        self.final_loop_elapsed = self.loop_started.map(|start| start.elapsed());
    }

    /// Returns the hat display for the currently viewed iteration, if available.
    pub fn current_iteration_hat_display(&self) -> Option<&str> {
        self.current_iteration()
            .and_then(|buffer| buffer.hat_display.as_deref())
    }

    /// Returns the backend display for the currently viewed iteration, if available.
    pub fn current_iteration_backend(&self) -> Option<&str> {
        self.current_iteration()
            .and_then(|buffer| buffer.backend.as_deref())
    }

    /// Returns a reference to the currently viewed iteration buffer.
    pub fn current_iteration(&self) -> Option<&IterationBuffer> {
        self.iterations.get(self.current_view)
    }

    /// Returns a mutable reference to the currently viewed iteration buffer.
    pub fn current_iteration_mut(&mut self) -> Option<&mut IterationBuffer> {
        self.iterations.get_mut(self.current_view)
    }

    /// Returns a shared handle to the current iteration's lines buffer.
    ///
    /// This allows stream handlers to write directly to the buffer,
    /// enabling real-time streaming to the TUI during execution.
    pub fn current_iteration_lines_handle(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<Vec<Line<'static>>>>> {
        self.iterations
            .get(self.current_view)
            .map(|buffer| buffer.lines_handle())
    }

    /// Returns a shared handle to the latest iteration's lines buffer.
    ///
    /// This should be used when writing output from the currently executing
    /// iteration, regardless of which iteration the user is viewing.
    /// This prevents output from being written to the wrong iteration when
    /// the user is reviewing an older iteration.
    pub fn latest_iteration_lines_handle(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<Vec<Line<'static>>>>> {
        self.iterations.last().map(|buffer| buffer.lines_handle())
    }

    /// Navigates to the next iteration (if not at the last one).
    /// If reaching the last iteration, re-enables following_latest and clears alerts.
    pub fn navigate_next(&mut self) {
        if self.iterations.is_empty() {
            return;
        }
        let max_index = self.iterations.len().saturating_sub(1);
        if self.current_view < max_index {
            self.current_view += 1;
            // Re-enable following when reaching the latest
            if self.current_view == max_index {
                self.following_latest = true;
                self.new_iteration_alert = None;
            }
        }
    }

    /// Navigates to the previous iteration (if not at the first one).
    /// Disables following_latest when navigating backwards.
    pub fn navigate_prev(&mut self) {
        if self.current_view > 0 {
            self.current_view -= 1;
            self.following_latest = false;
        }
    }

    /// Returns the total number of iterations.
    pub fn total_iterations(&self) -> usize {
        self.iterations.len()
    }

    // ========================================================================
    // Search Methods
    // ========================================================================

    /// Searches for the given query in the current iteration's content.
    /// Populates matches with (line_index, char_offset) pairs.
    /// Search is case-insensitive.
    pub fn search(&mut self, query: &str) {
        self.search_state.query = Some(query.to_string());
        self.search_state.matches.clear();
        self.search_state.current_match = 0;

        // Check if we have an iteration to search
        if self.iterations.get(self.current_view).is_none() {
            return;
        }

        let query_lower = query.to_lowercase();

        // Collect matches first (avoid borrow conflicts)
        let matches: Vec<(usize, usize)> = self
            .iterations
            .get(self.current_view)
            .and_then(|buffer| {
                let lines = buffer.lines.lock().ok()?;
                let mut found = Vec::new();
                for (line_idx, line) in lines.iter().enumerate() {
                    // Get the text content of the line
                    let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                    let line_lower = line_text.to_lowercase();

                    // Find all occurrences in this line
                    let mut search_start = 0;
                    while let Some(pos) = line_lower[search_start..].find(&query_lower) {
                        let char_offset = search_start + pos;
                        found.push((line_idx, char_offset));
                        search_start = char_offset + query_lower.len();
                    }
                }
                Some(found)
            })
            .unwrap_or_default();

        self.search_state.matches = matches;

        // Jump to first match if any exist
        if !self.search_state.matches.is_empty() {
            self.jump_to_current_match();
        }
    }

    /// Navigates to the next match, cycling back to the first if at the end.
    pub fn next_match(&mut self) {
        if self.search_state.matches.is_empty() {
            return;
        }

        self.search_state.current_match =
            (self.search_state.current_match + 1) % self.search_state.matches.len();
        self.jump_to_current_match();
    }

    /// Navigates to the previous match, cycling to the last if at the beginning.
    pub fn prev_match(&mut self) {
        if self.search_state.matches.is_empty() {
            return;
        }

        if self.search_state.current_match == 0 {
            self.search_state.current_match = self.search_state.matches.len() - 1;
        } else {
            self.search_state.current_match -= 1;
        }
        self.jump_to_current_match();
    }

    /// Clears the search state.
    pub fn clear_search(&mut self) {
        self.search_state.clear();
    }

    /// Jumps to the current match by adjusting scroll_offset to show the match line.
    fn jump_to_current_match(&mut self) {
        if self.search_state.matches.is_empty() {
            return;
        }

        let (line_idx, _) = self.search_state.matches[self.search_state.current_match];

        // Adjust scroll to show the match line
        // Use a default viewport height for calculation (will be overridden by actual render)
        let viewport_height = 20;
        if let Some(buffer) = self.current_iteration_mut() {
            // If the match line is above the current view, scroll up to it
            if line_idx < buffer.scroll_offset {
                buffer.scroll_offset = line_idx;
            }
            // If the match line is below the current view, scroll down to show it
            else if line_idx >= buffer.scroll_offset + viewport_height {
                buffer.scroll_offset = line_idx.saturating_sub(viewport_height / 2);
            }
        }
    }

    // ========================================================================
    // Guidance Methods
    // ========================================================================

    /// Enters guidance input mode.
    pub fn start_guidance(&mut self, mode: GuidanceMode) {
        self.guidance_mode = Some(mode);
        self.guidance_input.clear();
        self.guidance_flash = None;
    }

    /// Cancels guidance input without sending.
    pub fn cancel_guidance(&mut self) {
        self.guidance_mode = None;
        self.guidance_input.clear();
    }

    /// Sends the current guidance input.
    ///
    /// For `GuidanceMode::Next`, pushes to the shared queue (drained by loop_runner).
    /// For `GuidanceMode::Now`, writes an urgent-steer marker immediately and
    /// records `human.guidance` for the next prompt boundary.
    ///
    /// Returns true if guidance was sent successfully.
    pub fn send_guidance(&mut self) -> bool {
        let input = self.guidance_input.trim().to_string();
        if input.is_empty() {
            self.cancel_guidance();
            return false;
        }

        let mode = match self.guidance_mode {
            Some(m) => m,
            None => return false,
        };

        let (ok, result) = match mode {
            GuidanceMode::Next => {
                if let Ok(mut queue) = self.guidance_next_queue.lock() {
                    queue.push(input);
                    (true, GuidanceResult::Queued)
                } else {
                    (false, GuidanceResult::Failed)
                }
            }
            GuidanceMode::Now => {
                let ok =
                    self.write_urgent_steer_marker(&input) && self.write_guidance_event(&input);
                if ok {
                    (true, GuidanceResult::Sent)
                } else {
                    (false, GuidanceResult::Failed)
                }
            }
        };

        self.guidance_flash = Some((mode, result, Instant::now()));
        self.guidance_mode = None;
        self.guidance_input.clear();
        ok
    }

    /// Writes a human.guidance event directly to events.jsonl.
    fn write_guidance_event(&self, message: &str) -> bool {
        let Some(ref path) = self.events_path else {
            return false;
        };

        let timestamp = chrono::Utc::now().to_rfc3339();
        let event = serde_json::json!({
            "topic": "human.guidance",
            "payload": message,
            "ts": timestamp,
        });

        let line = match serde_json::to_string(&event) {
            Ok(l) => l,
            Err(_) => return false,
        };

        use std::io::Write;
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => f,
            Err(_) => return false,
        };

        file.write_all(line.as_bytes()).is_ok() && file.write_all(b"\n").is_ok()
    }

    fn write_urgent_steer_marker(&self, message: &str) -> bool {
        let Some(ref path) = self.urgent_steer_path else {
            return false;
        };

        ralph_core::UrgentSteerStore::new(path.clone())
            .append_message(message.to_string())
            .is_ok()
    }

    /// Returns true if guidance input is currently active.
    pub fn is_guidance_active(&self) -> bool {
        self.guidance_mode.is_some()
    }

    /// Clears flash message if it has expired.
    pub fn clear_expired_guidance_flash(&mut self) {
        if let Some((_, _, when)) = self.guidance_flash
            && when.elapsed() >= Duration::from_secs(2)
        {
            self.guidance_flash = None;
        }
    }

    /// Returns active guidance flash (mode + result) if still within display window (2 seconds).
    pub fn active_guidance_flash(&self) -> Option<(GuidanceMode, GuidanceResult)> {
        self.guidance_flash.and_then(|(mode, result, when)| {
            if when.elapsed() < Duration::from_secs(2) {
                Some((mode, result))
            } else {
                None
            }
        })
    }

    /// Updates the cached result of the asynchronous version check.
    pub fn set_update_status(&mut self, status: UpdateStatus) {
        self.update_status = status;
    }

    // ========================================================================
    // Wave View Methods
    // ========================================================================

    /// Returns the WaveInfo to use for wave view.
    ///
    /// If viewing the iteration that owns the active wave, returns the live
    /// `wave_active` (for real-time streaming). Otherwise returns the stored
    /// wave data from the currently viewed iteration buffer.
    fn wave_info_for_view(&self) -> Option<&WaveInfo> {
        // Active wave takes priority when viewing its owning iteration
        if let Some(wave_iter) = self.wave_active_iteration_idx
            && self.current_view == wave_iter
            && self.wave_active.is_some()
        {
            return self.wave_active.as_ref();
        }
        // Fall back to the per-iteration stored wave data
        self.iterations
            .get(self.current_view)
            .and_then(|buf| buf.wave_info.as_ref())
    }

    /// Returns the WaveInfo to use for wave view (mutable).
    fn wave_info_for_view_mut(&mut self) -> Option<&mut WaveInfo> {
        if let Some(wave_iter) = self.wave_active_iteration_idx
            && self.current_view == wave_iter
            && self.wave_active.is_some()
        {
            return self.wave_active.as_mut();
        }
        let idx = self.current_view;
        self.iterations
            .get_mut(idx)
            .and_then(|buf| buf.wave_info.as_mut())
    }

    /// Returns the WaveInfo for the current wave view (public, for header rendering).
    pub fn wave_info_for_wave_view(&self) -> Option<&WaveInfo> {
        self.wave_info_for_view()
    }

    /// Enters wave worker drill-down view. No-op if no wave data exists.
    pub fn enter_wave_view(&mut self) {
        if self.wave_info_for_view().is_some() {
            self.wave_view_active = true;
            self.wave_view_index = 0;
        }
    }

    /// Exits wave worker drill-down view.
    pub fn exit_wave_view(&mut self) {
        self.wave_view_active = false;
    }

    /// Cycles to the next worker in wave view.
    pub fn wave_view_next(&mut self) {
        if let Some(wave) = self.wave_info_for_view() {
            let total = wave.worker_buffers.len();
            if total > 0 {
                self.wave_view_index = (self.wave_view_index + 1) % total;
            }
        }
    }

    /// Cycles to the previous worker in wave view.
    pub fn wave_view_prev(&mut self) {
        if let Some(wave) = self.wave_info_for_view() {
            let total = wave.worker_buffers.len();
            if total > 0 {
                if self.wave_view_index == 0 {
                    self.wave_view_index = total - 1;
                } else {
                    self.wave_view_index -= 1;
                }
            }
        }
    }

    /// Returns the current wave worker buffer (immutable) for rendering.
    pub fn current_wave_worker_buffer(&self) -> Option<&IterationBuffer> {
        self.wave_info_for_view()
            .and_then(|w| w.worker_buffers.get(self.wave_view_index))
    }

    /// Returns the current wave worker buffer (mutable) for scrolling.
    pub fn current_wave_worker_buffer_mut(&mut self) -> Option<&mut IterationBuffer> {
        let idx = self.wave_view_index;
        self.wave_info_for_view_mut()
            .and_then(|w| w.worker_buffers.get_mut(idx))
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}
