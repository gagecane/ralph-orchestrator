//! Guidance input mode types for human-in-the-loop steering.
//!
//! Guidance writes are performed by methods on `TuiState` (see
//! `tui_state.rs`). This module defines only the small enums that
//! describe which kind of guidance is being entered and how the last
//! send attempt resolved.

/// Whether guidance is being entered for the next or current iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceMode {
    /// Guidance for the next prompt boundary.
    Next,
    /// Urgent steer for the active iteration.
    Now,
}

/// Result of attempting to send guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceResult {
    /// Next-iteration guidance was queued successfully.
    Queued,
    /// Urgent steer was persisted successfully.
    Sent,
    /// Guidance could not be queued/written.
    Failed,
}
