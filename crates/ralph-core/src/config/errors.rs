//! Configuration error type used across all config submodules.

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error(
        "Ambiguous routing: trigger '{trigger}' is claimed by both '{hat1}' and '{hat2}'.\nFix: ensure only one hat claims this trigger or delegate with a new event.\nSee: docs/reference/troubleshooting.md#ambiguous-routing"
    )]
    AmbiguousRouting {
        trigger: String,
        hat1: String,
        hat2: String,
    },

    #[error(
        "Mutually exclusive fields: '{field1}' and '{field2}' cannot both be specified.\nFix: remove one field or split into separate configs.\nSee: docs/reference/troubleshooting.md#mutually-exclusive-fields"
    )]
    MutuallyExclusive { field1: String, field2: String },

    #[error("Invalid completion_promise: must be non-empty and non-whitespace")]
    InvalidCompletionPromise,

    #[error(
        "Custom backend requires a command.\nFix: set 'cli.command' in your config (or run `ralph init --backend custom`).\nSee: docs/reference/troubleshooting.md#custom-backend-command"
    )]
    CustomBackendRequiresCommand,

    #[error(
        "Reserved trigger '{trigger}' used by hat '{hat}' - task.start and task.resume are reserved for Ralph (the coordinator). Use a delegated event like 'work.start' instead.\nSee: docs/reference/troubleshooting.md#reserved-trigger"
    )]
    ReservedTrigger { trigger: String, hat: String },

    #[error(
        "Hat '{hat}' is missing required 'description' field - add a short description of the hat's purpose.\nSee: docs/reference/troubleshooting.md#missing-hat-description"
    )]
    MissingDescription { hat: String },

    #[error(
        "RObot config error: {field} - {hint}\nSee: docs/reference/troubleshooting.md#robot-config"
    )]
    RobotMissingField { field: String, hint: String },

    #[error(
        "Invalid hooks phase-event '{phase_event}'. Supported v1 phase-events: pre.loop.start, post.loop.start, pre.iteration.start, post.iteration.start, pre.plan.created, post.plan.created, pre.human.interact, post.human.interact, pre.loop.complete, post.loop.complete, pre.loop.error, post.loop.error.\nFix: use one of the supported keys under hooks.events."
    )]
    InvalidHookPhaseEvent { phase_event: String },

    #[error(
        "Hook config validation error at '{field}': {message}\nSee: specs/add-hooks-to-ralph-orchestrator-lifecycle/design.md#hookspec-fields-v1"
    )]
    HookValidation { field: String, message: String },

    #[error(
        "Unsupported hooks field '{field}' for v1. {reason}\nSee: specs/add-hooks-to-ralph-orchestrator-lifecycle/design.md#out-of-scope-v1-non-goals"
    )]
    UnsupportedHookField { field: String, reason: String },

    #[error(
        "Invalid config key 'project'. Use 'core' instead (e.g. 'core.specs_dir' instead of 'project.specs_dir').\nSee: docs/guide/configuration.md"
    )]
    DeprecatedProjectKey,

    #[error(
        "Hat '{hat}' has invalid concurrency: {value}. Must be >= 1.\nFix: set 'concurrency' to 1 or higher."
    )]
    InvalidConcurrency { hat: String, value: u32 },

    #[error(
        "Hat '{hat}' has both 'aggregate' and 'concurrency > 1'. An aggregator hat cannot also be a concurrent worker.\nFix: remove 'aggregate' or set 'concurrency' to 1."
    )]
    AggregateOnConcurrentHat { hat: String },
}
