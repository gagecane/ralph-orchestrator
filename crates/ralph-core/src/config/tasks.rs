//! Tasks configuration for runtime work tracking.

use serde::{Deserialize, Serialize};

/// Tasks configuration.
///
/// Controls the runtime task tracking system that allows Ralph to manage
/// work items across iterations. Tasks are stored in `.ralph/agent/tasks.jsonl`.
///
/// When enabled, tasks replace scratchpad for loop completion verification.
///
/// Example configuration:
/// ```yaml
/// tasks:
///   enabled: true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksConfig {
    /// Whether the tasks feature is enabled.
    ///
    /// When true, tasks are used for loop completion verification.
    #[serde(default = "super::default_true")]
    pub enabled: bool,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Tasks enabled by default
        }
    }
}
