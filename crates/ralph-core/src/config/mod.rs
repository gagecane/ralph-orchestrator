//! Configuration types for the Ralph Orchestrator.
//!
//! This module supports both v1.x flat configuration format and v2.0 nested format.
//! Users can switch from Python v1.x to Rust v2.0 with zero config changes.
//!
//! The configuration is split across several submodules grouped by responsibility:
//!
//! | Submodule     | Responsibility                                         |
//! |---------------|--------------------------------------------------------|
//! | `ralph_config`| Top-level [`RalphConfig`], YAML loading, validation    |
//! | `event_loop`  | [`EventLoopConfig`] — iterations, prompts, completion  |
//! | `core`        | [`CoreConfig`] — scratchpad, specs dir, guardrails     |
//! | `cli`         | [`CliConfig`], [`AdaptersConfig`], [`AdapterSettings`] |
//! | `tui`         | [`TuiConfig`], [`InjectMode`]                          |
//! | `memories`    | [`MemoriesConfig`], [`MemoriesFilter`], [`TasksConfig`]|
//! | `hooks`       | [`HooksConfig`] and all hook spec/validation types     |
//! | `skills`      | [`SkillsConfig`], [`SkillOverride`]                    |
//! | `features`    | [`FeaturesConfig`], [`PreflightConfig`]                |
//! | `hats`        | [`HatConfig`], [`HatBackend`], wave [`AggregateConfig`]|
//! | `robot`       | [`RobotConfig`], [`TelegramBotConfig`]                 |
//! | `scratchpad`  | [`ScratchpadConfig`] and its custom deserializers      |
//! | `errors`      | [`ConfigError`]                                        |
//! | `warnings`    | [`ConfigWarning`]                                      |

mod cli;
mod core;
mod defaults;
mod errors;
mod event_loop;
mod features;
mod hats;
mod hooks;
mod memories;
mod ralph_config;
mod robot;
mod scratchpad;
mod skills;
mod tui;
mod warnings;

// Re-export the full public configuration surface. Several types
// (e.g. `AdapterSettings`, `TasksConfig`) are not referenced elsewhere in
// the crate today but must remain re-exported here for downstream callers
// and forward compatibility with the pre-split `config.rs` API.
#[allow(unused_imports)]
pub use cli::{AdapterSettings, AdaptersConfig, CliConfig};
#[allow(unused_imports)]
pub use core::CoreConfig;
#[allow(unused_imports)]
pub use errors::ConfigError;
#[allow(unused_imports)]
pub use event_loop::EventLoopConfig;
#[allow(unused_imports)]
pub use features::{FeaturesConfig, PreflightConfig};
#[allow(unused_imports)]
pub use hats::{AggregateConfig, AggregateMode, EventMetadata, HatBackend, HatConfig};
#[allow(unused_imports)]
pub use hooks::{
    HookDefaults, HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec, HookSuspendMode,
    HooksConfig,
};
#[allow(unused_imports)]
pub use memories::{MemoriesConfig, MemoriesFilter, TasksConfig};
#[allow(unused_imports)]
pub use ralph_config::RalphConfig;
#[allow(unused_imports)]
pub use robot::{RobotConfig, TelegramBotConfig};
#[allow(unused_imports)]
pub use scratchpad::ScratchpadConfig;
#[allow(unused_imports)]
pub use skills::{SkillOverride, SkillsConfig};
#[allow(unused_imports)]
pub use tui::{InjectMode, TuiConfig};
#[allow(unused_imports)]
pub use warnings::ConfigWarning;

#[cfg(test)]
mod tests;
