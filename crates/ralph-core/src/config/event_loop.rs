//! Event loop configuration.

use serde::{Deserialize, Serialize};

/// Event loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLoopConfig {
    /// Inline prompt text (mutually exclusive with prompt_file).
    pub prompt: Option<String>,

    /// Path to the prompt file.
    #[serde(default = "default_prompt_file")]
    pub prompt_file: String,

    /// Event topic that signals loop completion (must be emitted via `ralph emit`).
    #[serde(default = "default_completion_promise")]
    pub completion_promise: String,

    /// Maximum number of iterations before timeout.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    /// Maximum runtime in seconds.
    #[serde(default = "default_max_runtime")]
    pub max_runtime_seconds: u64,

    /// Maximum cost in USD before stopping.
    pub max_cost_usd: Option<f64>,

    /// Stop after this many consecutive failures.
    #[serde(default = "default_max_failures")]
    pub max_consecutive_failures: u32,

    /// Delay in seconds before starting the next iteration.
    /// Skipped when the next iteration is triggered by a human event.
    #[serde(default)]
    pub cooldown_delay_seconds: u64,

    /// Starting hat for multi-hat mode (deprecated, use starting_event instead).
    pub starting_hat: Option<String>,

    /// Event to publish after Ralph completes initial coordination.
    ///
    /// When custom hats are defined, Ralph handles `task.start` to do gap analysis
    /// and planning, then publishes this event to delegate to the first hat.
    ///
    /// Example: `starting_event: "tdd.start"` for TDD workflow.
    ///
    /// If not specified and hats are defined, Ralph will determine the appropriate
    /// event from the hat topology.
    pub starting_event: Option<String>,

    /// Warn when mutation testing score drops below this percentage (0-100).
    ///
    /// Warning-only: build.done is still accepted even if below threshold.
    #[serde(default)]
    pub mutation_score_warn_threshold: Option<f64>,

    /// When true, LOOP_COMPLETE does not terminate the loop.
    ///
    /// Instead of exiting, the loop injects a `task.resume` event and continues
    /// idling until new work arrives (human guidance, Telegram commands, etc.).
    /// The loop will only terminate on hard limits (max_iterations, max_runtime,
    /// max_cost), consecutive failures, or explicit interrupt/stop.
    #[serde(default)]
    pub persistent: bool,

    /// Event topics that must have been seen before LOOP_COMPLETE is accepted.
    /// If any required event has not been seen during the loop's lifetime,
    /// completion is rejected and a task.resume event is injected.
    #[serde(default)]
    pub required_events: Vec<String>,

    /// Event topic that triggers graceful early termination WITHOUT chain validation.
    /// Use this for human rejection, timeout escalation, or other abort paths.
    /// Defaults to "" (disabled). Set to "loop.cancel" to enable.
    #[serde(default)]
    pub cancellation_promise: String,

    /// When true, events emitted by a hat are validated against its declared
    /// `publishes` list. Out-of-scope events are dropped and replaced with
    /// `{hat_id}.scope_violation` diagnostic events. Defaults to false (permissive).
    #[serde(default)]
    pub enforce_hat_scope: bool,
}

pub(super) fn default_prompt_file() -> String {
    "PROMPT.md".to_string()
}

pub(super) fn default_completion_promise() -> String {
    "LOOP_COMPLETE".to_string()
}

pub(super) fn default_max_iterations() -> u32 {
    100
}

pub(super) fn default_max_runtime() -> u64 {
    14400 // 4 hours
}

pub(super) fn default_max_failures() -> u32 {
    5
}

impl Default for EventLoopConfig {
    fn default() -> Self {
        Self {
            prompt: None,
            prompt_file: default_prompt_file(),
            completion_promise: default_completion_promise(),
            max_iterations: default_max_iterations(),
            max_runtime_seconds: default_max_runtime(),
            max_cost_usd: None,
            max_consecutive_failures: default_max_failures(),
            cooldown_delay_seconds: 0,
            starting_hat: None,
            starting_event: None,
            mutation_score_warn_threshold: None,
            persistent: false,
            required_events: Vec::new(),
            cancellation_promise: String::new(),
            enforce_hat_scope: false,
        }
    }
}
