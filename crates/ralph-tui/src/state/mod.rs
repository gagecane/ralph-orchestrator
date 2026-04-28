//! State management for the TUI.
//!
//! This module is split into focused submodules:
//!
//! - [`task`] — `TaskSummary`, `TaskCounts` (task tracking display types)
//! - [`search`] — `SearchState` (search input + match cursor)
//! - [`guidance`] — `GuidanceMode`, `GuidanceResult` (human-in-the-loop input enums)
//! - [`update`] — `UpdateStatus` (version-check footer status)
//! - [`wave`] — `WaveInfo` (wave execution + per-worker buffers)
//! - [`iteration`] — `IterationBuffer` (per-iteration output + scroll)
//! - [`tui_state`] — `TuiState` (the big aggregate struct + all its methods)
//!
//! Everything is re-exported at the module root so existing callers that
//! use `crate::state::TuiState` (etc.) keep working unchanged.

pub mod guidance;
pub mod iteration;
pub mod search;
pub mod task;
pub mod tui_state;
pub mod update;
pub mod wave;

pub use guidance::{GuidanceMode, GuidanceResult};
pub use iteration::IterationBuffer;
pub use search::SearchState;
pub use task::{TaskCounts, TaskSummary};
pub use tui_state::TuiState;
pub use update::UpdateStatus;
pub use wave::WaveInfo;

#[cfg(test)]
mod tests;
