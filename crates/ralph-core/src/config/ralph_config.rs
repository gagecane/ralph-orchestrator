//! Top-level `RalphConfig` struct plus YAML loading, v1→v2 normalization, and
//! full configuration validation.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::cli::{AdapterSettings, AdaptersConfig, CliConfig};
use super::core::CoreConfig;
use super::errors::ConfigError;
use super::event_loop::{EventLoopConfig, default_prompt_file};
use super::features::FeaturesConfig;
use super::hats::{EventMetadata, HatConfig};
use super::hooks::{HookMutationConfig, HookOnError, HooksConfig, validate_hooks_phase_event_keys};
use super::memories::{MemoriesConfig, TasksConfig};
use super::robot::RobotConfig;
use super::skills::SkillsConfig;
use super::tui::TuiConfig;
use super::warnings::ConfigWarning;

/// Top-level configuration for Ralph Orchestrator.
///
/// Supports both v1.x flat format and v2.0 nested format:
/// - v1: `agent: claude`, `max_iterations: 100`
/// - v2: `cli: { backend: claude }`, `event_loop: { max_iterations: 100 }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Configuration struct with multiple feature flags
pub struct RalphConfig {
    /// Event loop configuration (v2 nested style).
    #[serde(default)]
    pub event_loop: EventLoopConfig,

    /// CLI backend configuration (v2 nested style).
    #[serde(default)]
    pub cli: CliConfig,

    /// Core paths and settings shared across all hats.
    #[serde(default)]
    pub core: CoreConfig,

    /// Custom hat definitions (optional).
    /// If empty, default planner and builder hats are used.
    #[serde(default)]
    pub hats: HashMap<String, HatConfig>,

    /// Event metadata definitions (optional).
    /// Defines what each event topic means, enabling auto-derived instructions.
    /// If a hat uses custom events, define them here for proper behavior injection.
    #[serde(default)]
    pub events: HashMap<String, EventMetadata>,

    // ─────────────────────────────────────────────────────────────────────────
    // V1 COMPATIBILITY FIELDS (flat format)
    // These map to nested v2 fields for backwards compatibility.
    // ─────────────────────────────────────────────────────────────────────────
    /// V1 field: Backend CLI (maps to cli.backend).
    /// Values: "claude", "kiro", "gemini", "codex", "amp", "pi", "auto", or "custom".
    #[serde(default)]
    pub agent: Option<String>,

    /// V1 field: Fallback order for auto-detection.
    #[serde(default)]
    pub agent_priority: Vec<String>,

    /// V1 field: Path to prompt file (maps to `event_loop.prompt_file`).
    #[serde(default)]
    pub prompt_file: Option<String>,

    /// V1 field: Completion detection string (maps to event_loop.completion_promise).
    #[serde(default)]
    pub completion_promise: Option<String>,

    /// V1 field: Maximum loop iterations (maps to event_loop.max_iterations).
    #[serde(default)]
    pub max_iterations: Option<u32>,

    /// V1 field: Maximum runtime in seconds (maps to event_loop.max_runtime_seconds).
    #[serde(default)]
    pub max_runtime: Option<u64>,

    /// V1 field: Maximum cost in USD (maps to event_loop.max_cost_usd).
    #[serde(default)]
    pub max_cost: Option<f64>,

    // ─────────────────────────────────────────────────────────────────────────
    // FEATURE FLAGS
    // ─────────────────────────────────────────────────────────────────────────
    /// Enable verbose output.
    #[serde(default)]
    pub verbose: bool,

    /// Archive prompts after completion (DEFERRED: warn if enabled).
    #[serde(default)]
    pub archive_prompts: bool,

    /// Enable metrics collection (DEFERRED: warn if enabled).
    #[serde(default)]
    pub enable_metrics: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // DROPPED FIELDS (accepted but ignored with warning)
    // ─────────────────────────────────────────────────────────────────────────
    /// V1 field: Token limits (DROPPED: controlled by CLI tool).
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// V1 field: Retry delay (DROPPED: handled differently in v2).
    #[serde(default)]
    pub retry_delay: Option<u32>,

    /// V1 adapter settings (partially supported).
    #[serde(default)]
    pub adapters: AdaptersConfig,

    // ─────────────────────────────────────────────────────────────────────────
    // WARNING CONTROL
    // ─────────────────────────────────────────────────────────────────────────
    /// Suppress all warnings (for CI environments).
    #[serde(default, rename = "_suppress_warnings")]
    pub suppress_warnings: bool,

    /// TUI configuration.
    #[serde(default)]
    pub tui: TuiConfig,

    /// Memories configuration for persistent learning across sessions.
    #[serde(default)]
    pub memories: MemoriesConfig,

    /// Tasks configuration for runtime work tracking.
    #[serde(default)]
    pub tasks: TasksConfig,

    /// Lifecycle hooks configuration.
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Skills configuration for the skill discovery and injection system.
    #[serde(default)]
    pub skills: SkillsConfig,

    /// Feature flags for optional capabilities.
    #[serde(default)]
    pub features: FeaturesConfig,

    /// RObot (Ralph-Orchestrator bot) configuration for Telegram-based interaction.
    #[serde(default, rename = "RObot")]
    pub robot: RobotConfig,
}

#[allow(clippy::derivable_impls)] // Cannot derive due to serde default functions
impl Default for RalphConfig {
    fn default() -> Self {
        Self {
            event_loop: EventLoopConfig::default(),
            cli: CliConfig::default(),
            core: CoreConfig::default(),
            hats: HashMap::new(),
            events: HashMap::new(),
            // V1 compatibility fields
            agent: None,
            agent_priority: vec![],
            prompt_file: None,
            completion_promise: None,
            max_iterations: None,
            max_runtime: None,
            max_cost: None,
            // Feature flags
            verbose: false,
            archive_prompts: false,
            enable_metrics: false,
            // Dropped fields
            max_tokens: None,
            retry_delay: None,
            adapters: AdaptersConfig::default(),
            // Warning control
            suppress_warnings: false,
            // TUI
            tui: TuiConfig::default(),
            // Memories
            memories: MemoriesConfig::default(),
            // Tasks
            tasks: TasksConfig::default(),
            // Hooks
            hooks: HooksConfig::default(),
            // Skills
            skills: SkillsConfig::default(),
            // Features
            features: FeaturesConfig::default(),
            // RObot (Ralph-Orchestrator bot)
            robot: RobotConfig::default(),
        }
    }
}

impl RalphConfig {
    /// Loads configuration from a YAML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path_ref = path.as_ref();
        debug!(path = %path_ref.display(), "Loading configuration from file");
        let content = std::fs::read_to_string(path_ref)?;
        Self::parse_yaml(&content)
    }

    /// Parses configuration from a YAML string.
    pub fn parse_yaml(content: &str) -> Result<Self, ConfigError> {
        // Pre-flight check for deprecated/invalid keys to improve UX.
        let value: serde_yaml::Value = serde_yaml::from_str(content)?;
        if let Some(map) = value.as_mapping()
            && map.contains_key(serde_yaml::Value::String("project".to_string()))
        {
            return Err(ConfigError::DeprecatedProjectKey);
        }

        validate_hooks_phase_event_keys(&value)?;

        let config: Self = serde_yaml::from_value(value)?;
        debug!(
            backend = %config.cli.backend,
            has_v1_fields = config.agent.is_some(),
            custom_hats = config.hats.len(),
            "Configuration loaded"
        );
        Ok(config)
    }

    /// Normalizes v1 flat fields into v2 nested structure.
    ///
    /// V1 flat fields take precedence over v2 nested fields when both are present.
    /// This allows users to use either format or mix them.
    pub fn normalize(&mut self) {
        let mut normalized_count = 0;

        // Map v1 `agent` to v2 `cli.backend`
        if let Some(ref agent) = self.agent {
            debug!(from = "agent", to = "cli.backend", value = %agent, "Normalizing v1 field");
            self.cli.backend = agent.clone();
            normalized_count += 1;
        }

        // Map v1 `prompt_file` to v2 `event_loop.prompt_file`
        if let Some(ref pf) = self.prompt_file {
            debug!(from = "prompt_file", to = "event_loop.prompt_file", value = %pf, "Normalizing v1 field");
            self.event_loop.prompt_file = pf.clone();
            normalized_count += 1;
        }

        // Map v1 `completion_promise` to v2 `event_loop.completion_promise`
        if let Some(ref cp) = self.completion_promise {
            debug!(
                from = "completion_promise",
                to = "event_loop.completion_promise",
                "Normalizing v1 field"
            );
            self.event_loop.completion_promise = cp.clone();
            normalized_count += 1;
        }

        // Map v1 `max_iterations` to v2 `event_loop.max_iterations`
        if let Some(mi) = self.max_iterations {
            debug!(
                from = "max_iterations",
                to = "event_loop.max_iterations",
                value = mi,
                "Normalizing v1 field"
            );
            self.event_loop.max_iterations = mi;
            normalized_count += 1;
        }

        // Map v1 `max_runtime` to v2 `event_loop.max_runtime_seconds`
        if let Some(mr) = self.max_runtime {
            debug!(
                from = "max_runtime",
                to = "event_loop.max_runtime_seconds",
                value = mr,
                "Normalizing v1 field"
            );
            self.event_loop.max_runtime_seconds = mr;
            normalized_count += 1;
        }

        // Map v1 `max_cost` to v2 `event_loop.max_cost_usd`
        if self.max_cost.is_some() {
            debug!(
                from = "max_cost",
                to = "event_loop.max_cost_usd",
                "Normalizing v1 field"
            );
            self.event_loop.max_cost_usd = self.max_cost;
            normalized_count += 1;
        }

        // Merge extra_instructions into instructions for each hat
        for (hat_id, hat) in &mut self.hats {
            if !hat.extra_instructions.is_empty() {
                for fragment in hat.extra_instructions.drain(..) {
                    if !hat.instructions.ends_with('\n') {
                        hat.instructions.push('\n');
                    }
                    hat.instructions.push_str(&fragment);
                }
                debug!(hat = %hat_id, "Merged extra_instructions into hat instructions");
                normalized_count += 1;
            }
        }

        if normalized_count > 0 {
            debug!(
                fields_normalized = normalized_count,
                "V1 to V2 config normalization complete"
            );
        }
    }

    /// Validates the configuration and returns warnings.
    ///
    /// This method checks for:
    /// - Deferred features that are enabled (archive_prompts, enable_metrics)
    /// - Dropped fields that are present (max_tokens, retry_delay, tool_permissions)
    /// - Ambiguous trigger routing across custom hats
    /// - Mutual exclusivity of prompt and prompt_file
    ///
    /// Returns a list of warnings that should be displayed to the user.
    pub fn validate(&self) -> Result<Vec<ConfigWarning>, ConfigError> {
        let mut warnings = Vec::new();

        // Skip all warnings if suppressed
        if self.suppress_warnings {
            return Ok(warnings);
        }

        // Check for mutual exclusivity of prompt and prompt_file in config
        // Only error if both are explicitly set (not defaults)
        if self.event_loop.prompt.is_some()
            && !self.event_loop.prompt_file.is_empty()
            && self.event_loop.prompt_file != default_prompt_file()
        {
            return Err(ConfigError::MutuallyExclusive {
                field1: "event_loop.prompt".to_string(),
                field2: "event_loop.prompt_file".to_string(),
            });
        }
        if self.event_loop.completion_promise.trim().is_empty() {
            return Err(ConfigError::InvalidCompletionPromise);
        }

        // Check custom backend has a command
        if self.cli.backend == "custom" && self.cli.command.as_ref().is_none_or(String::is_empty) {
            return Err(ConfigError::CustomBackendRequiresCommand);
        }

        // Check for deferred features
        if self.archive_prompts {
            warnings.push(ConfigWarning::DeferredFeature {
                field: "archive_prompts".to_string(),
                message: "Feature not yet available in v2".to_string(),
            });
        }

        if self.enable_metrics {
            warnings.push(ConfigWarning::DeferredFeature {
                field: "enable_metrics".to_string(),
                message: "Feature not yet available in v2".to_string(),
            });
        }

        // Check for dropped fields
        if self.max_tokens.is_some() {
            warnings.push(ConfigWarning::DroppedField {
                field: "max_tokens".to_string(),
                reason: "Token limits are controlled by the CLI tool".to_string(),
            });
        }

        if self.retry_delay.is_some() {
            warnings.push(ConfigWarning::DroppedField {
                field: "retry_delay".to_string(),
                reason: "Retry logic handled differently in v2".to_string(),
            });
        }

        if let Some(threshold) = self.event_loop.mutation_score_warn_threshold
            && !(0.0..=100.0).contains(&threshold)
        {
            warnings.push(ConfigWarning::InvalidValue {
                field: "event_loop.mutation_score_warn_threshold".to_string(),
                message: "Value must be between 0 and 100".to_string(),
            });
        }

        // Check adapter tool_permissions (dropped field)
        if self.adapters.claude.tool_permissions.is_some()
            || self.adapters.gemini.tool_permissions.is_some()
            || self.adapters.codex.tool_permissions.is_some()
            || self.adapters.amp.tool_permissions.is_some()
        {
            warnings.push(ConfigWarning::DroppedField {
                field: "adapters.*.tool_permissions".to_string(),
                reason: "CLI tool manages its own permissions".to_string(),
            });
        }

        // Validate RObot config
        self.robot.validate()?;

        // Validate hooks config semantics (v1 guardrails)
        self.validate_hooks()?;

        // Check for required description field on all hats
        for (hat_id, hat_config) in &self.hats {
            if hat_config
                .description
                .as_ref()
                .is_none_or(|d| d.trim().is_empty())
            {
                return Err(ConfigError::MissingDescription {
                    hat: hat_id.clone(),
                });
            }
        }

        // Check wave config validity
        for (hat_id, hat_config) in &self.hats {
            if hat_config.concurrency == 0 {
                return Err(ConfigError::InvalidConcurrency {
                    hat: hat_id.clone(),
                    value: 0,
                });
            }
            if hat_config.aggregate.is_some() && hat_config.concurrency > 1 {
                return Err(ConfigError::AggregateOnConcurrentHat {
                    hat: hat_id.clone(),
                });
            }
        }

        // Check for reserved triggers: task.start and task.resume are reserved for Ralph
        // Per design: Ralph coordinates first, then delegates to custom hats via events
        const RESERVED_TRIGGERS: &[&str] = &["task.start", "task.resume"];
        for (hat_id, hat_config) in &self.hats {
            for trigger in &hat_config.triggers {
                if RESERVED_TRIGGERS.contains(&trigger.as_str()) {
                    return Err(ConfigError::ReservedTrigger {
                        trigger: trigger.clone(),
                        hat: hat_id.clone(),
                    });
                }
            }
        }

        // Check for ambiguous routing: each trigger topic must map to exactly one hat
        // Per spec: "Every trigger maps to exactly one hat | No ambiguous routing"
        if !self.hats.is_empty() {
            let mut trigger_to_hat: HashMap<&str, &str> = HashMap::new();
            for (hat_id, hat_config) in &self.hats {
                for trigger in &hat_config.triggers {
                    if let Some(existing_hat) = trigger_to_hat.get(trigger.as_str()) {
                        return Err(ConfigError::AmbiguousRouting {
                            trigger: trigger.clone(),
                            hat1: (*existing_hat).to_string(),
                            hat2: hat_id.clone(),
                        });
                    }
                    trigger_to_hat.insert(trigger.as_str(), hat_id.as_str());
                }
            }
        }

        Ok(warnings)
    }

    fn validate_hooks(&self) -> Result<(), ConfigError> {
        Self::validate_non_v1_hook_fields("hooks", &self.hooks.extra)?;

        if self.hooks.defaults.timeout_seconds == 0 {
            return Err(ConfigError::HookValidation {
                field: "hooks.defaults.timeout_seconds".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }

        if self.hooks.defaults.max_output_bytes == 0 {
            return Err(ConfigError::HookValidation {
                field: "hooks.defaults.max_output_bytes".to_string(),
                message: "must be greater than 0".to_string(),
            });
        }

        for (phase_event, hook_specs) in &self.hooks.events {
            for (index, hook) in hook_specs.iter().enumerate() {
                let hook_field_base = format!("hooks.events.{phase_event}[{index}]");

                if hook.name.trim().is_empty() {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.name"),
                        message: "is required and must be non-empty".to_string(),
                    });
                }

                if hook
                    .command
                    .first()
                    .is_none_or(|command| command.trim().is_empty())
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.command"),
                        message: "is required and must include an executable at command[0]"
                            .to_string(),
                    });
                }

                if hook.on_error.is_none() {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.on_error"),
                        message: "is required in v1 (warn | block | suspend)".to_string(),
                    });
                }

                if let Some(timeout_seconds) = hook.timeout_seconds
                    && timeout_seconds == 0
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.timeout_seconds"),
                        message: "must be greater than 0 when specified".to_string(),
                    });
                }

                if let Some(max_output_bytes) = hook.max_output_bytes
                    && max_output_bytes == 0
                {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.max_output_bytes"),
                        message: "must be greater than 0 when specified".to_string(),
                    });
                }

                if hook.suspend_mode.is_some() && hook.on_error != Some(HookOnError::Suspend) {
                    return Err(ConfigError::HookValidation {
                        field: format!("{hook_field_base}.suspend_mode"),
                        message: "requires on_error: suspend".to_string(),
                    });
                }

                Self::validate_non_v1_hook_fields(&hook_field_base, &hook.extra)?;
                Self::validate_mutation_contract(&hook_field_base, &hook.mutate)?;
            }
        }

        Ok(())
    }

    fn validate_non_v1_hook_fields(
        path_prefix: &str,
        fields: &HashMap<String, serde_yaml::Value>,
    ) -> Result<(), ConfigError> {
        for key in fields.keys() {
            let field = format!("{path_prefix}.{key}");
            match key.as_str() {
                "global" | "globals" | "global_defaults" | "global_hooks" | "scope" => {
                    return Err(ConfigError::UnsupportedHookField {
                        field,
                        reason: "Use ~/.ralph/config.yml for user-level defaults; per-hook `global`/`scope` fields are not supported in v1"
                            .to_string(),
                    });
                }
                "parallel" | "parallelism" | "max_parallel" | "concurrency" | "run_in_parallel" => {
                    return Err(ConfigError::UnsupportedHookField {
                        field,
                        reason:
                            "Parallel hook execution is out of scope for v1; hooks must run sequentially"
                                .to_string(),
                    });
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_mutation_contract(
        hook_field_base: &str,
        mutate: &HookMutationConfig,
    ) -> Result<(), ConfigError> {
        let mutate_field_base = format!("{hook_field_base}.mutate");

        if !mutate.enabled {
            if mutate.format.is_some() || !mutate.extra.is_empty() {
                return Err(ConfigError::HookValidation {
                    field: mutate_field_base,
                    message: "mutation settings require mutate.enabled: true".to_string(),
                });
            }
            return Ok(());
        }

        if let Some(format) = mutate.format.as_deref()
            && !format.eq_ignore_ascii_case("json")
        {
            return Err(ConfigError::HookValidation {
                field: format!("{mutate_field_base}.format"),
                message: "only 'json' is supported for v1 mutation payloads".to_string(),
            });
        }

        if let Some(key) = mutate.extra.keys().next() {
            let field = format!("{mutate_field_base}.{key}");
            let reason = match key.as_str() {
                "prompt" | "prompt_mutation" | "events" | "event" | "config" | "full_context" => {
                    "v1 allows metadata-only mutation; prompt/event/config mutation is unsupported"
                        .to_string()
                }
                "xml" => "v1 mutation payloads are JSON-only".to_string(),
                _ => "unsupported mutate field in v1 (supported keys: enabled, format)".to_string(),
            };

            return Err(ConfigError::UnsupportedHookField { field, reason });
        }

        Ok(())
    }

    /// Gets the effective backend name, resolving "auto" using the priority list.
    pub fn effective_backend(&self) -> &str {
        &self.cli.backend
    }

    /// Returns the agent priority list for auto-detection.
    /// If empty, returns the default priority order.
    pub fn get_agent_priority(&self) -> Vec<&str> {
        if self.agent_priority.is_empty() {
            vec!["claude", "kiro", "gemini", "codex", "amp"]
        } else {
            self.agent_priority.iter().map(String::as_str).collect()
        }
    }

    /// Gets the adapter settings for a specific backend.
    #[allow(clippy::match_same_arms)] // Explicit match arms for each backend improves readability
    pub fn adapter_settings(&self, backend: &str) -> &AdapterSettings {
        match backend {
            "claude" => &self.adapters.claude,
            "gemini" => &self.adapters.gemini,
            "kiro" => &self.adapters.kiro,
            "codex" => &self.adapters.codex,
            "amp" => &self.adapters.amp,
            _ => &self.adapters.claude, // Default fallback
        }
    }
}
