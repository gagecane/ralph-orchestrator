//! Hat definitions, backends, event metadata, and wave aggregation.

use ralph_proto::Topic;
use serde::{Deserialize, Serialize};

use super::scratchpad::{ScratchpadConfig, deserialize_optional_scratchpad_config};

/// Metadata for an event topic.
///
/// Defines what an event means, enabling auto-derived instructions for hats.
/// When a hat triggers on or publishes an event, this metadata is used to
/// generate appropriate behavior instructions.
///
/// Example:
/// ```yaml
/// events:
///   deploy.start:
///     description: "Deployment has been requested"
///     on_trigger: "Prepare artifacts, validate config, check dependencies"
///     on_publish: "Signal that deployment should begin"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Brief description of what this event represents.
    #[serde(default)]
    pub description: String,

    /// Instructions for a hat that triggers on (receives) this event.
    /// Describes what the hat should do when it receives this event.
    #[serde(default)]
    pub on_trigger: String,

    /// Instructions for a hat that publishes (emits) this event.
    /// Describes when/how the hat should emit this event.
    #[serde(default)]
    pub on_publish: String,
}

/// Backend configuration for a hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HatBackend {
    // Order matters for serde untagged - most specific first
    /// Kiro agent with custom agent name and optional args.
    KiroAgent {
        #[serde(rename = "type")]
        backend_type: String,
        agent: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Named backend with args (has `type` but no `agent`).
    NamedWithArgs {
        #[serde(rename = "type")]
        backend_type: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Simple named backend (string form).
    Named(String),
    /// Custom backend with command and args.
    Custom {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl HatBackend {
    /// Converts to CLI backend string for execution.
    pub fn to_cli_backend(&self) -> String {
        match self {
            HatBackend::Named(name) => name.clone(),
            HatBackend::NamedWithArgs { backend_type, .. } => backend_type.clone(),
            HatBackend::KiroAgent { backend_type, .. } => backend_type.clone(),
            HatBackend::Custom { .. } => "custom".to_string(),
        }
    }
}

/// Configuration for a single hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatConfig {
    /// Human-readable name for the hat.
    pub name: String,

    /// Short description of the hat's purpose (required).
    /// Used in the HATS table to help Ralph understand when to delegate to this hat.
    pub description: Option<String>,

    /// Events that trigger this hat to be worn.
    /// Per spec: "Hats define triggers — which events cause Ralph to wear this hat."
    #[serde(default)]
    pub triggers: Vec<String>,

    /// Topics this hat publishes.
    #[serde(default)]
    pub publishes: Vec<String>,

    /// Instructions prepended to prompts.
    #[serde(default)]
    pub instructions: String,

    /// Additional instruction fragments appended to `instructions`.
    ///
    /// Use with YAML anchors to share common instruction blocks across hats:
    /// ```yaml
    /// _confidence_protocol: &confidence_protocol |
    ///   ### Confidence-Based Decision Protocol
    ///   ...
    ///
    /// hats:
    ///   architect:
    ///     instructions: |
    ///       ## ARCHITECT MODE
    ///       ...
    ///     extra_instructions:
    ///       - *confidence_protocol
    /// ```
    #[serde(default)]
    pub extra_instructions: Vec<String>,

    /// Backend to use for this hat (inherits from cli.backend if not specified).
    #[serde(default)]
    pub backend: Option<HatBackend>,

    /// Custom args to append to the backend CLI when this hat is active.
    ///
    /// Accepts both `backend_args:` and shorthand `args:`.
    #[serde(default, alias = "args")]
    pub backend_args: Option<Vec<String>>,

    /// Default event to publish if hat forgets to write an event.
    #[serde(default)]
    pub default_publishes: Option<String>,

    /// Maximum number of times this hat may be activated in a single loop run.
    ///
    /// When the limit is exceeded, the orchestrator publishes `<hat_id>.exhausted`
    /// instead of activating the hat again.
    pub max_activations: Option<u32>,

    /// Per-hat scratchpad override. If None, inherits from core.scratchpad.
    /// Accepts both a plain string shorthand and a structured object.
    #[serde(default, deserialize_with = "deserialize_optional_scratchpad_config")]
    pub scratchpad: Option<ScratchpadConfig>,

    /// Tools the hat is not allowed to use.
    ///
    /// Injected as a TOOL RESTRICTIONS section in the prompt (soft enforcement).
    /// After each iteration, a file-modification audit checks compliance when
    /// `Edit` or `Write` are disallowed (hard enforcement via scope_violation event).
    #[serde(default)]
    pub disallowed_tools: Vec<String>,

    /// Execution timeout in seconds for this hat.
    ///
    /// For wave workers, this controls how long each parallel worker can run.
    /// Defaults to the adapter-level timeout (typically 300s) if not set.
    #[serde(default)]
    pub timeout: Option<u32>,

    /// Maximum concurrent wave instances for this hat.
    ///
    /// When > 1, the loop runner spawns multiple backend instances in parallel
    /// for wave events targeting this hat. Default is 1 (sequential execution).
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,

    /// Aggregation configuration for this hat.
    ///
    /// When set, this hat acts as an aggregator — it buffers wave results and
    /// activates only when all correlated results have arrived (or timeout).
    /// Cannot be set on a hat with `concurrency > 1`.
    #[serde(default)]
    pub aggregate: Option<AggregateConfig>,
}

pub(super) fn default_concurrency() -> u32 {
    1
}

/// Configuration for wave result aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateConfig {
    /// Aggregation mode.
    pub mode: AggregateMode,

    /// Timeout in seconds for waiting on all wave results.
    /// After this timeout, the aggregator activates with whatever results are available.
    pub timeout: u32,
}

/// Aggregation mode for wave results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    /// Wait for all wave instances to complete before activating the aggregator.
    WaitForAll,
}

impl HatConfig {
    /// Converts trigger strings to Topic objects.
    pub fn trigger_topics(&self) -> Vec<Topic> {
        self.triggers.iter().map(|s| Topic::new(s)).collect()
    }

    /// Converts publish strings to Topic objects.
    pub fn publish_topics(&self) -> Vec<Topic> {
        self.publishes.iter().map(|s| Topic::new(s)).collect()
    }
}
