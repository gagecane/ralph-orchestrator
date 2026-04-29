//! Memories configuration for persistent learning across sessions.

use serde::{Deserialize, Serialize};

/// Memory injection mode.
///
/// Controls how memories are injected into agent context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InjectMode {
    /// Ralph automatically injects memories at the start of each iteration.
    #[default]
    Auto,
    /// Agent must explicitly run `ralph memory search` to access memories.
    Manual,
    /// Memories feature is disabled.
    None,
}

impl std::fmt::Display for InjectMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Manual => write!(f, "manual"),
            Self::None => write!(f, "none"),
        }
    }
}

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
