//! Lifecycle hooks configuration (pre.* / post.* phase-events).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::errors::ConfigError;

/// Hooks configuration.
///
/// Controls per-project orchestrator lifecycle hooks. Hooks are disabled by
/// default and are inert until explicitly enabled.
///
/// Example configuration:
/// ```yaml
/// hooks:
///   enabled: true
///   defaults:
///     timeout_seconds: 30
///     max_output_bytes: 8192
///     suspend_mode: wait_for_resume
///   events:
///     pre.loop.start:
///       - name: env-guard
///         command: ["./scripts/hooks/env-guard.sh"]
///         on_error: block
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Whether lifecycle hooks are enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Default guardrails applied to hook specs when per-hook values are absent.
    #[serde(default)]
    pub defaults: HookDefaults,

    /// Hook lists by lifecycle phase-event key.
    #[serde(default)]
    pub events: HashMap<HookPhaseEvent, Vec<HookSpec>>,

    /// Unknown keys captured for v1 guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// Hook defaults applied when a hook spec omits optional limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefaults {
    /// Maximum execution time per hook in seconds.
    #[serde(default = "default_hook_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Maximum stdout/stderr bytes stored per stream.
    #[serde(default = "default_hook_max_output_bytes")]
    pub max_output_bytes: u64,

    /// Suspend strategy used when `on_error: suspend` and no per-hook mode is set.
    #[serde(default)]
    pub suspend_mode: HookSuspendMode,
}

fn default_hook_timeout_seconds() -> u64 {
    30
}

fn default_hook_max_output_bytes() -> u64 {
    8192
}

impl Default for HookDefaults {
    fn default() -> Self {
        Self {
            timeout_seconds: default_hook_timeout_seconds(),
            max_output_bytes: default_hook_max_output_bytes(),
            suspend_mode: HookSuspendMode::default(),
        }
    }
}

/// Supported lifecycle phase-event keys for v1 hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPhaseEvent {
    #[serde(rename = "pre.loop.start")]
    PreLoopStart,
    #[serde(rename = "post.loop.start")]
    PostLoopStart,
    #[serde(rename = "pre.iteration.start")]
    PreIterationStart,
    #[serde(rename = "post.iteration.start")]
    PostIterationStart,
    #[serde(rename = "pre.plan.created")]
    PrePlanCreated,
    #[serde(rename = "post.plan.created")]
    PostPlanCreated,
    #[serde(rename = "pre.human.interact")]
    PreHumanInteract,
    #[serde(rename = "post.human.interact")]
    PostHumanInteract,
    #[serde(rename = "pre.loop.complete")]
    PreLoopComplete,
    #[serde(rename = "post.loop.complete")]
    PostLoopComplete,
    #[serde(rename = "pre.loop.error")]
    PreLoopError,
    #[serde(rename = "post.loop.error")]
    PostLoopError,
}

impl HookPhaseEvent {
    /// Canonical string value used in YAML keys and telemetry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreLoopStart => "pre.loop.start",
            Self::PostLoopStart => "post.loop.start",
            Self::PreIterationStart => "pre.iteration.start",
            Self::PostIterationStart => "post.iteration.start",
            Self::PrePlanCreated => "pre.plan.created",
            Self::PostPlanCreated => "post.plan.created",
            Self::PreHumanInteract => "pre.human.interact",
            Self::PostHumanInteract => "post.human.interact",
            Self::PreLoopComplete => "pre.loop.complete",
            Self::PostLoopComplete => "post.loop.complete",
            Self::PreLoopError => "pre.loop.error",
            Self::PostLoopError => "post.loop.error",
        }
    }

    /// Parses a phase-event string to the canonical enum.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pre.loop.start" => Some(Self::PreLoopStart),
            "post.loop.start" => Some(Self::PostLoopStart),
            "pre.iteration.start" => Some(Self::PreIterationStart),
            "post.iteration.start" => Some(Self::PostIterationStart),
            "pre.plan.created" => Some(Self::PrePlanCreated),
            "post.plan.created" => Some(Self::PostPlanCreated),
            "pre.human.interact" => Some(Self::PreHumanInteract),
            "post.human.interact" => Some(Self::PostHumanInteract),
            "pre.loop.complete" => Some(Self::PreLoopComplete),
            "post.loop.complete" => Some(Self::PostLoopComplete),
            "pre.loop.error" => Some(Self::PreLoopError),
            "post.loop.error" => Some(Self::PostLoopError),
            _ => None,
        }
    }
}

impl std::fmt::Display for HookPhaseEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((*self).as_str())
    }
}

/// Validates phase-event keys in the raw YAML value before full deserialization.
///
/// Surfaces a precise `InvalidHookPhaseEvent` error when an unknown key is used
/// under `hooks.events`, instead of the generic serde rejection message.
pub(super) fn validate_hooks_phase_event_keys(
    value: &serde_yaml::Value,
) -> Result<(), ConfigError> {
    let Some(root) = value.as_mapping() else {
        return Ok(());
    };

    let Some(hooks) = root.get(serde_yaml::Value::String("hooks".to_string())) else {
        return Ok(());
    };

    let Some(hooks_map) = hooks.as_mapping() else {
        return Ok(());
    };

    let Some(events) = hooks_map.get(serde_yaml::Value::String("events".to_string())) else {
        return Ok(());
    };

    let Some(events_map) = events.as_mapping() else {
        return Ok(());
    };

    for key in events_map.keys() {
        if let Some(phase_event) = key.as_str()
            && HookPhaseEvent::parse(phase_event).is_none()
        {
            return Err(ConfigError::InvalidHookPhaseEvent {
                phase_event: phase_event.to_string(),
            });
        }
    }

    Ok(())
}

/// Per-hook failure disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOnError {
    /// Continue orchestration and record warning telemetry.
    Warn,
    /// Stop the current lifecycle action as a failure.
    Block,
    /// Suspend orchestration and await policy-specific recovery.
    Suspend,
}

/// Suspend mode used for `on_error: suspend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSuspendMode {
    /// Pause the loop until an explicit operator resume signal is received.
    #[default]
    WaitForResume,
    /// Retry automatically with bounded backoff.
    RetryBackoff,
    /// Wait for resume, then retry once.
    WaitThenRetry,
}

/// Mutation settings for a hook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookMutationConfig {
    /// Opt-in flag for parsing stdout as mutation JSON.
    #[serde(default)]
    pub enabled: bool,

    /// Optional explicit payload format guardrail. Only `json` is valid in v1.
    #[serde(default)]
    pub format: Option<String>,

    /// Unknown keys captured for v1 mutation guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// Hook specification for a single lifecycle event mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Stable hook identifier used in telemetry and diagnostics.
    #[serde(default)]
    pub name: String,

    /// Command argv form (`command[0]` executable + args).
    #[serde(default)]
    pub command: Vec<String>,

    /// Optional working directory override.
    #[serde(default)]
    pub cwd: Option<PathBuf>,

    /// Optional environment variable overrides.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Per-hook timeout override in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,

    /// Per-hook output cap override in bytes (applies per stream).
    #[serde(default)]
    pub max_output_bytes: Option<u64>,

    /// Failure behavior (`warn`, `block`, `suspend`). Required in v1.
    #[serde(default)]
    pub on_error: Option<HookOnError>,

    /// Optional suspend strategy override for `on_error: suspend`.
    #[serde(default)]
    pub suspend_mode: Option<HookSuspendMode>,

    /// Mutation policy (opt-in, JSON-only contract enforced by validation/runtime).
    #[serde(default)]
    pub mutate: HookMutationConfig,

    /// Unknown keys captured for v1 guardrails.
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}
