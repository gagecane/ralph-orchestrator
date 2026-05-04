//! Memories and tasks configuration.

use serde::{Deserialize, Serialize};

use super::defaults::default_true;
use super::tui::InjectMode;

/// Memories configuration.
///
/// Controls the persistent learning system that allows Ralph to accumulate
/// wisdom across sessions. Memories are stored in `.ralph/agent/memories.md`.
///
/// When enabled, the memories skill is automatically injected to teach
/// agents how to create and search memories (skill injection is implicit).
///
/// Example configuration:
/// ```yaml
/// memories:
///   enabled: true
///   inject: auto
///   budget: 2000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoriesConfig {
    /// Whether the memories feature is enabled.
    ///
    /// When true, memories are injected and the skill is taught to the agent.
    #[serde(default)]
    pub enabled: bool,

    /// How memories are injected into agent context.
    #[serde(default)]
    pub inject: InjectMode,

    /// Maximum tokens to inject (0 = unlimited).
    ///
    /// When set, memories are truncated to fit within this budget.
    #[serde(default)]
    pub budget: usize,

    /// Filter configuration for memory injection.
    #[serde(default)]
    pub filter: MemoriesFilter,
}

impl Default for MemoriesConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Memories enabled by default
            inject: InjectMode::Auto,
            budget: 0,
            filter: MemoriesFilter::default(),
        }
    }
}

/// Filter configuration for memory injection.
///
/// Controls which memories are included when priming context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoriesFilter {
    /// Filter by memory types (empty = all types).
    #[serde(default)]
    pub types: Vec<String>,

    /// Filter by tags (empty = all tags).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Only include memories from the last N days (0 = no time limit).
    #[serde(default)]
    pub recent: u32,
}

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
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Tasks enabled by default
        }
    }
}
