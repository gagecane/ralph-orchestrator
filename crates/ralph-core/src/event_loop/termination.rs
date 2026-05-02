//! Termination reasons, related checks, and teardown support for the event loop.

use super::EventLoop;
use ralph_proto::Event;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tracing::{info, warn};

/// Reason the event loop terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationReason {
    /// Completion promise was detected in output.
    CompletionPromise,
    /// Maximum iterations reached.
    MaxIterations,
    /// Maximum runtime exceeded.
    MaxRuntime,
    /// Maximum cost exceeded.
    MaxCost,
    /// Too many consecutive failures.
    ConsecutiveFailures,
    /// Loop thrashing detected (repeated blocked events).
    LoopThrashing,
    /// Stale loop detected (same topic emitted 3+ times consecutively).
    LoopStale,
    /// Too many consecutive malformed JSONL lines in events file.
    ValidationFailure,
    /// Manually stopped.
    Stopped,
    /// Interrupted by signal (SIGINT/SIGTERM).
    Interrupted,
    /// Restart requested via Telegram `/restart` command.
    RestartRequested,
    /// Workspace directory (worktree) was removed externally.
    WorkspaceGone,
    /// Loop was cancelled gracefully via loop.cancel event (human rejection, timeout).
    Cancelled,
}

impl TerminationReason {
    /// Returns the exit code for this termination reason per spec.
    ///
    /// Per spec "Loop Termination" section:
    /// - 0: Completion promise detected (success)
    /// - 1: Consecutive failures or unrecoverable error (failure)
    /// - 2: Max iterations, max runtime, or max cost exceeded (limit)
    /// - 130: User interrupt (SIGINT = 128 + 2)
    pub fn exit_code(&self) -> i32 {
        match self {
            TerminationReason::CompletionPromise => 0,
            TerminationReason::ConsecutiveFailures
            | TerminationReason::LoopThrashing
            | TerminationReason::LoopStale
            | TerminationReason::ValidationFailure
            | TerminationReason::Stopped
            | TerminationReason::WorkspaceGone => 1,
            TerminationReason::MaxIterations
            | TerminationReason::MaxRuntime
            | TerminationReason::MaxCost => 2,
            TerminationReason::Interrupted => 130,
            // Restart uses exit code 3 to signal the caller to exec-replace
            TerminationReason::RestartRequested => 3,
            // Cancelled is a clean exit (0) — the loop stopped intentionally
            TerminationReason::Cancelled => 0,
        }
    }

    /// Returns the reason string for use in loop.terminate event payload.
    ///
    /// Per spec event payload format:
    /// `completed | max_iterations | max_runtime | consecutive_failures | interrupted | error`
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminationReason::CompletionPromise => "completed",
            TerminationReason::MaxIterations => "max_iterations",
            TerminationReason::MaxRuntime => "max_runtime",
            TerminationReason::MaxCost => "max_cost",
            TerminationReason::ConsecutiveFailures => "consecutive_failures",
            TerminationReason::LoopThrashing => "loop_thrashing",
            TerminationReason::LoopStale => "loop_stale",
            TerminationReason::ValidationFailure => "validation_failure",
            TerminationReason::Stopped => "stopped",
            TerminationReason::Interrupted => "interrupted",
            TerminationReason::RestartRequested => "restart_requested",
            TerminationReason::WorkspaceGone => "workspace_gone",
            TerminationReason::Cancelled => "cancelled",
        }
    }

    /// Returns true if this is a successful completion (not an error or limit).
    pub fn is_success(&self) -> bool {
        matches!(self, TerminationReason::CompletionPromise)
    }
}

/// Formats a duration as human-readable string.
pub(super) fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Returns a human-readable status based on termination reason.
pub(super) fn termination_status_text(reason: &TerminationReason) -> &'static str {
    match reason {
        TerminationReason::CompletionPromise => "All tasks completed successfully.",
        TerminationReason::MaxIterations => "Stopped at iteration limit.",
        TerminationReason::MaxRuntime => "Stopped at runtime limit.",
        TerminationReason::MaxCost => "Stopped at cost limit.",
        TerminationReason::ConsecutiveFailures => "Too many consecutive failures.",
        TerminationReason::LoopThrashing => {
            "Loop thrashing detected - same hat repeatedly blocked."
        }
        TerminationReason::LoopStale => {
            "Stale loop detected - same topic emitted 3+ times consecutively."
        }
        TerminationReason::ValidationFailure => "Too many consecutive malformed JSONL events.",
        TerminationReason::Stopped => "Manually stopped.",
        TerminationReason::Interrupted => "Interrupted by signal.",
        TerminationReason::RestartRequested => "Restarting by human request.",
        TerminationReason::WorkspaceGone => "Workspace directory removed externally.",
        TerminationReason::Cancelled => "Cancelled gracefully (human rejection or timeout).",
    }
}

impl EventLoop {
    /// Checks if any termination condition is met.
    pub fn check_termination(&self) -> Option<TerminationReason> {
        let cfg = &self.config.event_loop;

        if self.state.iteration >= cfg.max_iterations {
            return Some(TerminationReason::MaxIterations);
        }

        if self.state.elapsed().as_secs() >= cfg.max_runtime_seconds {
            return Some(TerminationReason::MaxRuntime);
        }

        if let Some(max_cost) = cfg.max_cost_usd
            && self.state.cumulative_cost >= max_cost
        {
            return Some(TerminationReason::MaxCost);
        }

        if self.state.consecutive_failures >= cfg.max_consecutive_failures {
            return Some(TerminationReason::ConsecutiveFailures);
        }

        // Check for loop thrashing: planner keeps dispatching abandoned tasks
        if self.state.abandoned_task_redispatches >= 3 {
            return Some(TerminationReason::LoopThrashing);
        }

        // Check for validation failures: too many consecutive malformed JSONL lines
        if self.state.consecutive_malformed_events >= 3 {
            return Some(TerminationReason::ValidationFailure);
        }

        // Check for stale loop: same event signature emitted 3+ times in a row
        if self.state.consecutive_same_signature >= 3 {
            let topic = self
                .state
                .last_emitted_signature
                .as_ref()
                .map(|signature| signature.topic.as_str())
                .unwrap_or("?");
            warn!(
                topic,
                count = self.state.consecutive_same_signature,
                "Stale loop detected: same event signature emitted consecutively"
            );
            return Some(TerminationReason::LoopStale);
        }

        // Check for stop signal from Telegram /stop or CLI stop-requested
        let stop_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/stop-requested");
        if stop_path.exists() {
            let _ = std::fs::remove_file(&stop_path);
            return Some(TerminationReason::Stopped);
        }

        // Check for restart signal from Telegram /restart command
        let restart_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/restart-requested");
        if restart_path.exists() {
            return Some(TerminationReason::RestartRequested);
        }

        // Check if workspace directory has been removed (zombie worktree detection)
        if !std::path::Path::new(&self.config.core.workspace_root).is_dir() {
            return Some(TerminationReason::WorkspaceGone);
        }

        None
    }

    /// Check if a loop.cancel event was detected.
    ///
    /// Unlike check_completion_event(), this does NOT validate required_events.
    /// Cancellation is an explicit abort — it doesn't need the workflow to be complete.
    pub fn check_cancellation_event(&mut self) -> Option<TerminationReason> {
        if !self.state.cancellation_requested {
            return None;
        }
        self.state.cancellation_requested = false;
        info!("Loop cancelled gracefully via loop.cancel event");

        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "cancelled".to_string(),
            },
        );

        Some(TerminationReason::Cancelled)
    }

    /// Checks if a completion event was received and returns termination reason.
    ///
    /// Completion is only accepted via JSONL events (e.g., `ralph emit`).
    pub fn check_completion_event(&mut self) -> Option<TerminationReason> {
        if !self.state.completion_requested {
            return None;
        }

        // Event chain validation: check required events were seen
        let required = &self.config.event_loop.required_events;
        if !required.is_empty() {
            let missing = self.state.missing_required_events(required);
            if !missing.is_empty() {
                warn!(
                    missing = ?missing,
                    "Rejecting LOOP_COMPLETE: required events not seen during loop lifetime"
                );
                self.state.completion_requested = false;

                // Inject task.resume so the loop continues
                let resume_payload = format!(
                    "LOOP_COMPLETE rejected: missing required events: {:?}. \
                     The agent must complete all workflow phases before emitting LOOP_COMPLETE. \
                     Use loop.cancel to abort the workflow instead.",
                    missing
                );
                self.bus.publish(Event::new("task.resume", resume_payload));
                return None;
            }
        }

        self.state.completion_requested = false;

        // In persistent mode, suppress completion and keep the loop alive
        if self.config.event_loop.persistent {
            info!("Completion event suppressed - persistent mode active, loop staying alive");

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "loop",
                crate::diagnostics::OrchestrationEvent::LoopTerminated {
                    reason: "completion_event_suppressed_persistent".to_string(),
                },
            );

            // Inject a task.resume event so the loop continues with an idle prompt
            let resume_event = Event::new(
                "task.resume",
                "Persistent mode: loop staying alive after completion signal. \
                 Check for new tasks or await human guidance.",
            );
            self.bus.publish(resume_event);

            return None;
        }

        // Runtime tasks are the canonical queue when memories/tasks mode is enabled.
        if self.config.memories.enabled {
            if let Ok(false) = self.verify_tasks_complete() {
                let open_tasks = self.get_open_task_list();
                warn!(
                    open_tasks = ?open_tasks,
                    "Rejecting completion event with {} open task(s)",
                    open_tasks.len()
                );
                self.bus.publish(Event::new(
                    "task.resume",
                    format!(
                        "Completion rejected: runtime tasks remain open: {:?}. Close, fail, or reopen outstanding tasks before emitting the completion promise.",
                        open_tasks
                    ),
                ));
                return None;
            }
        } else if let Ok(false) = self.verify_scratchpad_complete() {
            warn!("Completion event with pending scratchpad tasks - trusting agent decision");
        }

        info!("Completion event detected - terminating");

        // Log loop terminated
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::LoopTerminated {
                reason: "completion_event".to_string(),
            },
        );

        Some(TerminationReason::CompletionPromise)
    }

    /// Checks output from Ralph for the completion promise.
    pub fn check_ralph_completion(&self, output: &str) -> bool {
        let events = crate::event_parser::EventParser::new().parse(output);
        events
            .iter()
            .any(|event| event.topic.as_str() == self.config.event_loop.completion_promise)
    }

    /// Publishes the loop.terminate system event to observers.
    ///
    /// Per spec: "Published by the orchestrator (not agents) when the loop exits."
    /// This is an observer-only event—hats cannot trigger on it.
    ///
    /// Returns the event for logging purposes.
    pub fn publish_terminate_event(&mut self, reason: &TerminationReason) -> Event {
        // Stop the robot service if it was running
        self.stop_robot_service();

        let elapsed = self.state.elapsed();
        let duration_str = format_duration(elapsed);

        let payload = format!(
            "## Reason\n{}\n\n## Status\n{}\n\n## Summary\n- Iterations: {}\n- Duration: {}\n- Exit code: {}",
            reason.as_str(),
            termination_status_text(reason),
            self.state.iteration,
            duration_str,
            reason.exit_code()
        );

        let event = Event::new("loop.terminate", &payload);

        // Publish to bus for observers (but no hat can trigger on this)
        self.bus.publish(event.clone());

        info!(
            reason = %reason.as_str(),
            iterations = self.state.iteration,
            duration = %duration_str,
            "Wrapping up: {}. {} iterations in {}.",
            reason.as_str(),
            self.state.iteration,
            duration_str
        );

        event
    }

    /// Returns the robot service's shutdown flag, if active.
    ///
    /// Signal handlers can set this flag to interrupt `wait_for_response()`
    /// without waiting for the full timeout.
    pub fn robot_shutdown_flag(&self) -> Option<Arc<AtomicBool>> {
        self.robot_service.as_ref().map(|s| s.shutdown_flag())
    }

    /// Stops the robot service if it's running.
    ///
    /// Called during loop termination to cleanly shut down the communication backend.
    pub(super) fn stop_robot_service(&mut self) {
        if let Some(service) = self.robot_service.take() {
            service.stop();
        }
    }

    pub(super) fn is_restart_request_payload(payload: &str) -> bool {
        let payload = payload.to_ascii_lowercase();
        payload.contains("restart yourself") || payload.contains("restart ralph")
    }

    pub(super) fn is_restart_request_event(event: &Event) -> bool {
        matches!(event.topic.as_str(), "human.response" | "user.prompt")
            && Self::is_restart_request_payload(&event.payload)
    }

    pub(super) fn mark_restart_requested(&self, source: &str) {
        let restart_path =
            std::path::Path::new(&self.config.core.workspace_root).join(".ralph/restart-requested");

        if let Some(parent) = restart_path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            warn!(
                error = %err,
                path = %parent.display(),
                "Failed to create restart-requested parent directory"
            );
            return;
        }

        if let Err(err) = std::fs::write(&restart_path, source) {
            warn!(
                error = %err,
                path = %restart_path.display(),
                "Failed to write restart-requested signal"
            );
            return;
        }

        info!(
            source,
            path = %restart_path.display(),
            "Restart requested from human text"
        );
    }
}
