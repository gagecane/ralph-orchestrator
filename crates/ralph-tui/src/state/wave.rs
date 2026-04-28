//! Wave execution tracking for the TUI header and drill-down view.
//!
//! A `WaveInfo` is created when a hat spawns parallel workers. Each
//! worker has its own `IterationBuffer` (from `state::iteration`) so
//! the user can scroll through per-worker output via the wave view.
//!
//! The wave-view navigation methods (enter/exit, cycle workers,
//! current buffer) live on `TuiState` (see `tui_state.rs`).

use super::iteration::IterationBuffer;
use std::time::Instant;

/// Tracks active wave execution for header display and per-worker output.
///
/// Manual `Debug` impl because `worker_buffers` contains `Arc<Mutex<>>` fields.
pub struct WaveInfo {
    pub hat_name: String,
    pub total: u32,
    pub completed: u32,
    pub started_at: Instant,
    /// Per-worker output buffers (indexed by worker_index).
    pub worker_buffers: Vec<IterationBuffer>,
}

impl std::fmt::Debug for WaveInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaveInfo")
            .field("hat_name", &self.hat_name)
            .field("total", &self.total)
            .field("completed", &self.completed)
            .field("started_at", &self.started_at)
            .field("worker_buffers_len", &self.worker_buffers.len())
            .finish()
    }
}

impl WaveInfo {
    /// Creates a new WaveInfo with N empty worker buffers.
    pub fn new(hat_name: String, total: u32) -> Self {
        let worker_buffers = (0..total)
            .map(|i| {
                let mut buf = IterationBuffer::new(i + 1);
                buf.hat_display = Some(format!("Worker {}/{}", i + 1, total));
                buf
            })
            .collect();
        Self {
            hat_name,
            total,
            completed: 0,
            started_at: Instant::now(),
            worker_buffers,
        }
    }
}
