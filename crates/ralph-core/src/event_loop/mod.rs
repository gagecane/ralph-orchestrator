//! Event loop orchestration.
//!
//! The event loop coordinates the execution of hats via pub/sub messaging.

mod event_processing;
mod guidance;
mod hat_routing;
mod loop_state;
mod mutation;
mod prompt;
mod tasks_scratchpad;
mod termination;
#[cfg(test)]
mod tests;

pub use event_processing::{ProcessedEvents, ProcessedEventsWithWaves};
pub use loop_state::LoopState;
pub use prompt::UserPrompt;
pub use termination::TerminationReason;

#[cfg(test)]
use crate::config::HatBackend;
#[cfg(test)]
use crate::event_parser::{MutationEvidence, MutationStatus};
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use termination::{format_duration, termination_status_text};

use crate::config::RalphConfig;
use crate::event_reader::EventReader;
use crate::hat_registry::HatRegistry;
use crate::hatless_ralph::HatlessRalph;
use crate::instructions::InstructionBuilder;
use crate::loop_context::LoopContext;
use crate::skill_registry::SkillRegistry;
use ralph_proto::{Event, EventBus, RobotService};

// Re-exports used by the submodule tests in `tests.rs`. These are imported at the
// parent-module scope so `use super::*;` inside `tests.rs` keeps working after the
// submodule split.
#[cfg(test)]
use ralph_proto::HatId;
use tracing::{debug, warn};

/// The main event loop orchestrator.
pub struct EventLoop {
    config: RalphConfig,
    registry: HatRegistry,
    bus: EventBus,
    state: LoopState,
    instruction_builder: InstructionBuilder,
    ralph: HatlessRalph,
    /// Cached human guidance messages that should persist across iterations.
    robot_guidance: Vec<String>,
    /// Event reader for consuming events from JSONL file.
    /// Made pub(crate) to allow tests to override the path.
    pub(crate) event_reader: EventReader,
    diagnostics: crate::diagnostics::DiagnosticsCollector,
    /// Loop context for path resolution (None for legacy single-loop mode).
    loop_context: Option<LoopContext>,
    /// Skill registry for the current loop.
    skill_registry: SkillRegistry,
    /// Robot service for human-in-the-loop communication.
    /// Injected externally when `human.enabled` is true and this is the primary loop.
    robot_service: Option<Box<dyn RobotService>>,
}

impl EventLoop {
    /// Creates a new event loop from configuration.
    pub fn new(config: RalphConfig) -> Self {
        // Try to create diagnostics collector, but fall back to disabled if it fails
        // (e.g., in tests without proper directory setup)
        let diagnostics = crate::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
            .unwrap_or_else(|e| {
                debug!(
                    "Failed to initialize diagnostics: {}, using disabled collector",
                    e
                );
                crate::diagnostics::DiagnosticsCollector::disabled()
            });

        Self::with_diagnostics(config, diagnostics)
    }

    /// Creates a new event loop with a loop context for path resolution.
    ///
    /// The loop context determines where events, tasks, and other state files
    /// are located. Use this for multi-loop scenarios where each loop runs
    /// in an isolated workspace (git worktree).
    pub fn with_context(config: RalphConfig, context: LoopContext) -> Self {
        let diagnostics = crate::diagnostics::DiagnosticsCollector::new(context.workspace())
            .unwrap_or_else(|e| {
                debug!(
                    "Failed to initialize diagnostics: {}, using disabled collector",
                    e
                );
                crate::diagnostics::DiagnosticsCollector::disabled()
            });

        Self::with_context_and_diagnostics(config, context, diagnostics)
    }

    /// Creates a new event loop with explicit loop context and diagnostics.
    pub fn with_context_and_diagnostics(
        mut config: RalphConfig,
        context: LoopContext,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        let registry = HatRegistry::from_config(&config);
        let instruction_builder =
            InstructionBuilder::with_events(config.core.clone(), config.events.clone());

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        // Always register Ralph as catch-all coordinator
        // Per spec: "Ralph runs when no hat triggered — Universal fallback for orphaned events"
        let ralph_hat = ralph_proto::Hat::new("ralph", "Ralph").subscribe("*"); // Subscribe to all events
        bus.register(ralph_hat);

        if registry.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                registry.len()
            );
        }

        // Build skill registry from config
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                context.workspace(),
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read timestamped events path from marker file, fall back to default
        // The marker file contains a relative path like ".ralph/events-20260127-123456.jsonl"
        // which we resolve relative to the workspace root
        let events_path = std::fs::read_to_string(context.current_events_marker())
            .map(|s| {
                let relative = s.trim();
                context.workspace().join(relative)
            })
            .unwrap_or_else(|_| context.events_path());
        let event_reader = EventReader::new(&events_path);

        Self {
            config,
            registry,
            bus,
            state: LoopState::new(),
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: Some(context),
            skill_registry,
            robot_service: None,
        }
    }

    /// Creates a new event loop with explicit diagnostics collector (for testing).
    pub fn with_diagnostics(
        mut config: RalphConfig,
        diagnostics: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        // Solo mode safety guard: force scratchpad enabled when no hats defined
        if config.hats.is_empty() && !config.core.scratchpad.enabled {
            warn!(
                "core.scratchpad.enabled is false but no hats are defined. \
                 Scratchpad is the only continuity mechanism in solo mode — forcing enabled."
            );
            config.core.scratchpad.enabled = true;
        }

        let registry = HatRegistry::from_config(&config);
        let instruction_builder =
            InstructionBuilder::with_events(config.core.clone(), config.events.clone());

        let mut bus = EventBus::new();

        // Per spec: "Hatless Ralph is constant — Cannot be replaced, overwritten, or configured away"
        // Ralph is ALWAYS registered as the universal fallback for orphaned events.
        // Custom hats are registered first (higher priority), Ralph catches everything else.
        for hat in registry.all() {
            bus.register(hat.clone());
        }

        // Always register Ralph as catch-all coordinator
        // Per spec: "Ralph runs when no hat triggered — Universal fallback for orphaned events"
        let ralph_hat = ralph_proto::Hat::new("ralph", "Ralph").subscribe("*"); // Subscribe to all events
        bus.register(ralph_hat);

        if registry.is_empty() {
            debug!("Solo mode: Ralph is the only coordinator");
        } else {
            debug!(
                "Multi-hat mode: {} custom hats + Ralph as fallback",
                registry.len()
            );
        }

        // Build skill registry from config
        let workspace_root = std::path::Path::new(".");
        let mut skill_registry = if config.skills.enabled {
            SkillRegistry::from_config(
                &config.skills,
                workspace_root,
                Some(config.cli.backend.as_str()),
            )
            .unwrap_or_else(|e| {
                warn!(
                    "Failed to build skill registry: {}, using empty registry",
                    e
                );
                SkillRegistry::new(Some(config.cli.backend.as_str()))
            })
        } else {
            SkillRegistry::new(Some(config.cli.backend.as_str()))
        };

        // Remove task/memory skills from the index when their config is disabled
        if !config.tasks.enabled {
            skill_registry.remove("ralph-tools-tasks");
        }
        if !config.memories.enabled {
            skill_registry.remove("ralph-tools-memories");
        }

        let skill_index = if config.skills.enabled {
            skill_registry.build_index(None)
        } else {
            String::new()
        };

        // When memories are enabled, add tasks CLI instructions alongside scratchpad
        let ralph = HatlessRalph::new(
            config.event_loop.completion_promise.clone(),
            config.core.clone(),
            &registry,
            config.event_loop.starting_event.clone(),
        )
        .with_memories_enabled(config.memories.enabled)
        .with_skill_index(skill_index);

        // Read events path from marker file, fall back to default if not present
        // The marker file is written by run_loop_impl() at run startup
        let events_path = std::fs::read_to_string(".ralph/current-events")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| ".ralph/events.jsonl".to_string());
        let event_reader = EventReader::new(&events_path);

        Self {
            config,
            registry,
            bus,
            state: LoopState::new(),
            instruction_builder,
            ralph,
            robot_guidance: Vec::new(),
            event_reader,
            diagnostics,
            loop_context: None,
            skill_registry,
            robot_service: None,
        }
    }

    /// Injects a robot service for human-in-the-loop communication.
    ///
    /// Call this after construction to enable `human.interact` event handling,
    /// periodic check-ins, and question/response flow. The service is typically
    /// created by the CLI layer (e.g., `TelegramService`) and injected here,
    /// keeping the core event loop decoupled from any specific communication
    /// platform.
    pub fn set_robot_service(&mut self, service: Box<dyn RobotService>) {
        self.robot_service = Some(service);
    }

    /// Returns the loop context, if one was provided.
    pub fn loop_context(&self) -> Option<&LoopContext> {
        self.loop_context.as_ref()
    }

    /// Returns the current loop state.
    pub fn state(&self) -> &LoopState {
        &self.state
    }

    /// Resets the stale-loop topic counter.
    ///
    /// Call after processing wave results — multiple events with the same topic
    /// (e.g. `review.done` from parallel workers) are expected and should not
    /// trigger the stale loop detector.
    pub fn reset_stale_topic_counter(&mut self) {
        self.state.consecutive_same_signature = 0;
        self.state.last_emitted_signature = None;
    }

    /// Returns the configuration.
    pub fn config(&self) -> &RalphConfig {
        &self.config
    }

    /// Returns the hat registry.
    pub fn registry(&self) -> &HatRegistry {
        &self.registry
    }

    /// Records hook telemetry for diagnostics.
    pub fn log_hook_run_telemetry(&self, entry: crate::diagnostics::HookRunTelemetryEntry) {
        self.diagnostics.log_hook_run(entry);
    }

    /// Logs the full prompt for an iteration to the diagnostics session.
    pub fn log_prompt(&self, iteration: u32, hat: &str, prompt: &str) {
        self.diagnostics.log_prompt(iteration, hat, prompt);
    }

    /// Adds an observer that receives all published events.
    ///
    /// Multiple observers can be added (e.g., session recorder + TUI).
    /// Each observer is called before events are routed to subscribers.
    pub fn add_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        self.bus.add_observer(observer);
    }

    /// Sets a single observer, clearing any existing observers.
    ///
    /// Prefer `add_observer` when multiple observers are needed.
    #[deprecated(since = "2.0.0", note = "Use add_observer instead")]
    pub fn set_observer<F>(&mut self, observer: F)
    where
        F: Fn(&Event) + Send + 'static,
    {
        #[allow(deprecated)]
        self.bus.set_observer(observer);
    }

    /// Initializes the loop by publishing the start event.
    pub fn initialize(&mut self, prompt_content: &str) {
        // Use configured starting_event or default to task.start for backward compatibility
        let topic = self
            .config
            .event_loop
            .starting_event
            .clone()
            .unwrap_or_else(|| "task.start".to_string());
        self.initialize_with_topic(&topic, prompt_content);
    }

    /// Initializes the loop for resume mode by publishing task.resume.
    ///
    /// Per spec: "User can run `ralph resume` to restart reading existing scratchpad."
    /// The planner should read the existing scratchpad rather than doing fresh gap analysis.
    pub fn initialize_resume(&mut self, prompt_content: &str) {
        // Resume always uses task.resume regardless of starting_event config
        self.initialize_with_topic("task.resume", prompt_content);
    }

    /// Common initialization logic with configurable topic.
    fn initialize_with_topic(&mut self, topic: &str, prompt_content: &str) {
        // Store the objective so it persists across all iterations.
        // After iteration 1, bus.take_pending() consumes the start event,
        // so without this the objective would be invisible to later hats.
        self.ralph.set_objective(prompt_content.to_string());

        let start_event = Event::new(topic, prompt_content);
        self.bus.publish(start_event);
        debug!(topic = topic, "Published {} event", topic);
    }

    /// Returns a mutable reference to the event bus for direct event publishing.
    ///
    /// This is primarily used for planning sessions to inject user responses
    /// as events into the orchestration loop.
    pub fn bus(&mut self) -> &mut EventBus {
        &mut self.bus
    }

    /// Adds cost to the cumulative total.
    pub fn add_cost(&mut self, cost: f64) {
        self.state.cumulative_cost += cost;
    }
}
