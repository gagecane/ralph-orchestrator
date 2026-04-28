//! Task tracking state for the TUI.
//!
//! These lightweight summaries power the task-progress widgets. Live task
//! data (from `.ralph/agent/tasks.jsonl`) is reduced to these display-only
//! shapes by `state_mutations` before being stored on `TuiState`.

/// Summary of a task for TUI display.
/// Contains only the fields needed for rendering.
#[derive(Debug, Clone, Default)]
pub struct TaskSummary {
    /// Task identifier (e.g., "task-1737372000-a1b2").
    pub id: String,
    /// Task title/description.
    pub title: String,
    /// Task status (e.g., "open", "closed", "blocked").
    pub status: String,
}

impl TaskSummary {
    /// Creates a new task summary.
    pub fn new(id: impl Into<String>, title: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: status.into(),
        }
    }
}

/// Aggregate task statistics for TUI display.
#[derive(Debug, Clone, Default)]
pub struct TaskCounts {
    /// Total number of tasks.
    pub total: usize,
    /// Number of open tasks.
    pub open: usize,
    /// Number of closed tasks.
    pub closed: usize,
    /// Number of ready (unblocked) tasks.
    pub ready: usize,
}

impl TaskCounts {
    /// Creates new task counts.
    pub fn new(total: usize, open: usize, closed: usize, ready: usize) -> Self {
        Self {
            total,
            open,
            closed,
            ready,
        }
    }
}
