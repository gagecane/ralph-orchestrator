//! Lifecycle hook dispatch, suspend/resume coordination, and mutation parsing.
//!
//! This module owns the hook execution subsystem used by the main loop:
//!
//! - **Dispatch**: `dispatch_phase_event_hooks`, `dispatch_pre_loop_termination_hooks`,
//!   `dispatch_post_loop_termination_hooks` — invoke hooks with suspend/retry policies.
//! - **Mutation parsing**: `parse_hook_mutation_stdout`, `merge_hook_metadata_namespace`,
//!   and friends — interpret structured hook stdout mutations.
//! - **Blocking gates**: `fail_if_blocking_*_outcomes` — convert blocking dispositions
//!   into errors for specific lifecycle events.
//! - **Suspend/resume**: `wait_for_resume_if_suspended` — polls for resume, stop, or
//!   restart signals when a hook dispositions as `Suspend`.
//! - **Retry policies**: `run_retry_backoff_policy`, `run_wait_then_retry_policy` —
//!   generic retry loops consumed by dispatch helpers.
//!
//! Types exposed to tests via `#[cfg(test)]` re-exports in `loop_runner/mod.rs`
//! include `HookDispatchOutcome`, `HookDispatchFailure`, `HookMutationParseOutcome`,
//! `HookMutationParseError`, and `RetryBackoffDelayOutcome`.

use anyhow::{Context, Result};
use ralph_core::diagnostics::{HookDisposition, HookRunTelemetryEntry};
use ralph_core::{
    EventLoop, HookEngine, HookExecutor, HookExecutorContract, HookMutationConfig, HookOnError,
    HookPayloadBuilderInput, HookPhaseEvent, HookRunRequest, HookRunResult, HookSuspendMode,
    LoopContext, SuspendStateRecord, SuspendStateStore, TerminationReason,
};
use std::fs;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use super::payload::build_loop_termination_payload_input;

pub(super) fn loop_termination_phase_events(
    reason: &TerminationReason,
) -> (HookPhaseEvent, HookPhaseEvent) {
    if reason.is_success() {
        (
            HookPhaseEvent::PreLoopComplete,
            HookPhaseEvent::PostLoopComplete,
        )
    } else {
        (HookPhaseEvent::PreLoopError, HookPhaseEvent::PostLoopError)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_pre_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: TerminationReason,
) -> impl std::future::Future<Output = Result<TerminationReason>> + Send {
    let outcomes = collect_loop_termination_hook_outcomes(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        ctx,
        max_iterations,
        accumulated_hook_metadata,
        &reason,
        true,
    );
    let loop_id = loop_id.to_string();
    let suspend_state_store = suspend_state_store.clone();

    async move {
        resolve_loop_termination_hook_outcomes(&outcomes, &loop_id, &suspend_state_store, reason)
            .await
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_post_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: TerminationReason,
) -> impl std::future::Future<Output = Result<TerminationReason>> + Send {
    let outcomes = collect_loop_termination_hook_outcomes(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        ctx,
        max_iterations,
        accumulated_hook_metadata,
        &reason,
        false,
    );
    let loop_id = loop_id.to_string();
    let suspend_state_store = suspend_state_store.clone();

    async move {
        resolve_loop_termination_hook_outcomes(&outcomes, &loop_id, &suspend_state_store, reason)
            .await
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_loop_termination_hook_outcomes(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: &TerminationReason,
    is_pre_phase: bool,
) -> Vec<HookDispatchOutcome> {
    let (pre_phase_event, post_phase_event) = loop_termination_phase_events(reason);
    let phase_event = if is_pre_phase {
        pre_phase_event
    } else {
        post_phase_event
    };

    let active_hat = event_loop.get_active_hat_id().as_str().to_string();
    let outcomes = dispatch_phase_event_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        phase_event,
        build_loop_termination_payload_input(
            loop_id,
            ctx,
            max_iterations,
            event_loop.state().iteration,
            Some(active_hat.clone()),
            Some(active_hat),
            None,
            reason,
            accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(accumulated_hook_metadata, &outcomes);
    outcomes
}

pub(super) async fn resolve_loop_termination_hook_outcomes(
    outcomes: &[HookDispatchOutcome],
    loop_id: &str,
    suspend_state_store: &SuspendStateStore,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    fail_if_blocking_loop_termination_outcomes(outcomes)?;

    if let Some(termination_reason) =
        wait_for_resume_if_suspended(outcomes, loop_id, suspend_state_store).await?
    {
        return Ok(termination_reason);
    }

    Ok(reason)
}

pub(super) const RETRY_BACKOFF_DELAYS_MS: [u64; 3] = [100, 200, 400];
pub(super) const RETRY_BACKOFF_SIGNAL_POLL_INTERVAL_MS: u64 = 100;
pub(super) const SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS: u64 = 250;
pub(super) const HOOK_MUTATION_PAYLOAD_METADATA_KEY: &str = "metadata";
pub(super) const HOOK_MUTATION_METADATA_NAMESPACE_KEY: &str = "hook_metadata";

#[derive(Debug, Clone, PartialEq)]
pub(super) enum HookMutationParseOutcome {
    Disabled,
    Parsed {
        namespaced_metadata: serde_json::Map<String, serde_json::Value>,
    },
    Invalid(HookMutationParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HookMutationParseError {
    InvalidJson { message: String },
    InvalidSchema { message: String },
}

pub(super) fn format_hook_mutation_parse_error(error: &HookMutationParseError) -> String {
    match error {
        HookMutationParseError::InvalidJson { message }
        | HookMutationParseError::InvalidSchema { message } => message.clone(),
    }
}

pub(super) fn parse_hook_mutation_stdout(
    mutate: &HookMutationConfig,
    hook_name: &str,
    stdout: &str,
) -> HookMutationParseOutcome {
    if !mutate.enabled {
        return HookMutationParseOutcome::Disabled;
    }

    let parsed = match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        Ok(parsed) => parsed,
        Err(error) => {
            return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidJson {
                message: format!("mutation stdout is not valid JSON: {error}"),
            });
        }
    };

    let Some(payload_object) = parsed.as_object() else {
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: "mutation payload must be a JSON object".to_string(),
        });
    };

    if payload_object.len() != 1 || !payload_object.contains_key(HOOK_MUTATION_PAYLOAD_METADATA_KEY)
    {
        let keys = payload_object.keys().cloned().collect::<Vec<_>>();
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: format!(
                "mutation payload supports only '{{\"{HOOK_MUTATION_PAYLOAD_METADATA_KEY}\": {{...}}}}'; found keys: {keys:?}"
            ),
        });
    }

    let Some(metadata) = payload_object
        .get(HOOK_MUTATION_PAYLOAD_METADATA_KEY)
        .and_then(serde_json::Value::as_object)
        .cloned()
    else {
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: "mutation payload key 'metadata' must contain a JSON object".to_string(),
        });
    };

    let mut namespaced_metadata = serde_json::Map::new();
    if let Err(error) = merge_hook_metadata_namespace(&mut namespaced_metadata, hook_name, metadata)
    {
        return HookMutationParseOutcome::Invalid(error);
    }

    HookMutationParseOutcome::Parsed {
        namespaced_metadata,
    }
}

pub(super) fn merge_hook_metadata_namespace(
    accumulated_metadata: &mut serde_json::Map<String, serde_json::Value>,
    hook_name: &str,
    metadata: serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), HookMutationParseError> {
    if hook_name.trim().is_empty() {
        return Err(HookMutationParseError::InvalidSchema {
            message: "hook metadata namespace requires non-empty hook name".to_string(),
        });
    }

    let namespace = accumulated_metadata
        .entry(HOOK_MUTATION_METADATA_NAMESPACE_KEY.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    let Some(namespace_object) = namespace.as_object_mut() else {
        return Err(HookMutationParseError::InvalidSchema {
            message: format!(
                "metadata namespace '{HOOK_MUTATION_METADATA_NAMESPACE_KEY}' must be a JSON object"
            ),
        });
    };

    namespace_object.insert(hook_name.to_string(), serde_json::Value::Object(metadata));
    Ok(())
}

pub(super) fn merge_namespaced_hook_metadata(
    accumulated_metadata: &mut serde_json::Map<String, serde_json::Value>,
    namespaced_metadata: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), HookMutationParseError> {
    let Some(namespace_object) = namespaced_metadata
        .get(HOOK_MUTATION_METADATA_NAMESPACE_KEY)
        .and_then(serde_json::Value::as_object)
    else {
        return Err(HookMutationParseError::InvalidSchema {
            message: format!(
                "parsed mutation metadata must contain object key '{HOOK_MUTATION_METADATA_NAMESPACE_KEY}'"
            ),
        });
    };

    for (hook_name, metadata_value) in namespace_object {
        let Some(metadata_object) = metadata_value.as_object().cloned() else {
            return Err(HookMutationParseError::InvalidSchema {
                message: format!(
                    "parsed metadata entry for hook '{hook_name}' must be a JSON object"
                ),
            });
        };

        merge_hook_metadata_namespace(accumulated_metadata, hook_name, metadata_object)?;
    }

    Ok(())
}

pub(super) fn merge_accumulated_hook_metadata_from_outcomes(
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    outcomes: &[HookDispatchOutcome],
) {
    for outcome in outcomes {
        let HookMutationParseOutcome::Parsed {
            namespaced_metadata,
        } = &outcome.mutation_parse_outcome
        else {
            continue;
        };

        if let Err(error) =
            merge_namespaced_hook_metadata(accumulated_hook_metadata, namespaced_metadata)
        {
            warn!(
                phase_event = %outcome.phase_event,
                hook_name = %outcome.hook_name,
                error = ?error,
                "Failed to merge parsed hook mutation metadata; ignoring mutation output"
            );
        }
    }
}

pub(super) fn mutation_parse_failure(
    mutation_parse_outcome: &HookMutationParseOutcome,
) -> Option<HookDispatchFailure> {
    let HookMutationParseOutcome::Invalid(error) = mutation_parse_outcome else {
        return None;
    };

    Some(HookDispatchFailure::InvalidMutationOutput {
        message: format_hook_mutation_parse_error(error),
    })
}

pub(super) fn max_retry_attempts_for_suspend_mode(suspend_mode: HookSuspendMode) -> u32 {
    match suspend_mode {
        HookSuspendMode::WaitForResume => 1,
        HookSuspendMode::RetryBackoff => RETRY_BACKOFF_DELAYS_MS.len() as u32 + 1,
        HookSuspendMode::WaitThenRetry => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SuspendWaitOutcome {
    Resume,
    Stop,
    Restart,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct HookDispatchOutcome {
    pub(super) phase_event: HookPhaseEvent,
    pub(super) hook_name: String,
    pub(super) disposition: HookDisposition,
    pub(super) suspend_mode: HookSuspendMode,
    pub(super) failure: Option<HookDispatchFailure>,
    pub(super) mutation_parse_outcome: HookMutationParseOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HookDispatchFailure {
    HookRunFailed {
        exit_code: Option<i32>,
        timed_out: bool,
    },
    HookExecutionError {
        message: String,
    },
    InvalidMutationOutput {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryBackoffDelayOutcome {
    Elapsed,
    StopRequested,
    RestartRequested,
}

pub(super) fn dispatch_phase_event_hooks(
    event_loop: &EventLoop,
    hooks_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    phase_event: HookPhaseEvent,
    payload_input: HookPayloadBuilderInput,
) -> Vec<HookDispatchOutcome> {
    if !hooks_enabled {
        return Vec::new();
    }

    let resolved_hooks = hook_engine.resolve_phase_event(phase_event);
    if resolved_hooks.is_empty() {
        return Vec::new();
    }

    let workspace_root = payload_input.workspace.clone();
    let payload = hook_engine.build_payload(phase_event, payload_input);
    let stdin_payload = match serde_json::to_value(&payload) {
        Ok(value) => value,
        Err(error) => {
            warn!(
                phase_event = %phase_event,
                error = %error,
                "Failed to serialize lifecycle hook payload; skipping phase-event dispatch"
            );
            return Vec::new();
        }
    };

    let mut outcomes = Vec::with_capacity(resolved_hooks.len());

    for hook in resolved_hooks {
        let hook_name = hook.name.clone();
        let phase_event_key = hook.phase_event.as_str().to_string();

        let request = HookRunRequest {
            phase_event: phase_event_key.clone(),
            hook_name: hook_name.clone(),
            command: hook.command.clone(),
            workspace_root: workspace_root.clone(),
            cwd: hook.cwd.clone(),
            env: hook.env.clone(),
            timeout_seconds: hook.timeout_seconds,
            max_output_bytes: hook.max_output_bytes,
            stdin_payload: stdin_payload.clone(),
        };

        let outcome = dispatch_hook_with_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            &phase_event_key,
            hook.phase_event,
            &hook_name,
            hook.on_error,
            hook.suspend_mode,
            &hook.mutate,
            &request,
        );
        outcomes.push(outcome);
    }

    outcomes
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_hook_with_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    request: &HookRunRequest,
) -> HookDispatchOutcome {
    let retry_max_attempts = max_retry_attempts_for_suspend_mode(suspend_mode);
    let outcome = execute_hook_attempt(
        event_loop,
        hook_executor,
        loop_id,
        phase_event_key,
        phase_event,
        hook_name,
        on_error,
        suspend_mode,
        mutate,
        1,
        retry_max_attempts,
        request,
    );

    if outcome.disposition != HookDisposition::Suspend {
        return outcome;
    }

    match suspend_mode {
        HookSuspendMode::WaitForResume => outcome,
        HookSuspendMode::RetryBackoff => dispatch_retry_backoff_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            phase_event_key,
            phase_event,
            hook_name,
            on_error,
            suspend_mode,
            mutate,
            retry_max_attempts,
            request,
            outcome,
        ),
        HookSuspendMode::WaitThenRetry => dispatch_wait_then_retry_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            phase_event_key,
            phase_event,
            hook_name,
            on_error,
            suspend_mode,
            mutate,
            retry_max_attempts,
            request,
            outcome,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_retry_backoff_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_max_attempts: u32,
    request: &HookRunRequest,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome {
    run_retry_backoff_policy(
        phase_event_key,
        hook_name,
        &RETRY_BACKOFF_DELAYS_MS,
        |backoff_delay, _retry_attempt| {
            wait_for_retry_backoff_delay_with_signal_poll(
                request.workspace_root.as_path(),
                backoff_delay,
            )
        },
        |retry_attempt| {
            execute_hook_attempt(
                event_loop,
                hook_executor,
                loop_id,
                phase_event_key,
                phase_event,
                hook_name,
                on_error,
                suspend_mode,
                mutate,
                retry_attempt,
                retry_max_attempts,
                request,
            )
        },
        outcome,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_wait_then_retry_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_max_attempts: u32,
    request: &HookRunRequest,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome {
    let suspend_state_store = SuspendStateStore::new(&request.workspace_root);
    let reason = format_suspending_hook_reason(&outcome);
    let suspend_state = SuspendStateRecord::new(
        loop_id,
        phase_event,
        hook_name,
        reason,
        suspend_mode,
        chrono::Utc::now(),
    );

    if let Err(error) = suspend_state_store.write_suspend_state(&suspend_state) {
        warn!(
            phase_event = %phase_event_key,
            hook_name = %hook_name,
            error = %error,
            "Failed to persist suspend-state for wait_then_retry; deferring to standard suspend handling"
        );
        return outcome;
    }

    warn!(
        phase_event = %phase_event_key,
        hook_name = %hook_name,
        "Lifecycle hook requested suspend(wait_then_retry); entering wait-for-resume gate before single retry"
    );

    run_wait_then_retry_policy(
        phase_event_key,
        hook_name,
        || wait_for_suspend_signal_with_poll(&suspend_state_store),
        || {
            suspend_state_store
                .clear_suspend_state()
                .context("Failed to clear wait_then_retry suspend-state after resume")?;
            Ok(())
        },
        || {
            execute_hook_attempt(
                event_loop,
                hook_executor,
                loop_id,
                phase_event_key,
                phase_event,
                hook_name,
                on_error,
                suspend_mode,
                mutate,
                2,
                retry_max_attempts,
                request,
            )
        },
        outcome,
    )
}

pub(super) fn run_retry_backoff_policy<FWaitForDelay, FRunRetryAttempt>(
    phase_event_key: &str,
    hook_name: &str,
    backoff_delays_ms: &[u64],
    mut wait_for_delay: FWaitForDelay,
    mut run_retry_attempt: FRunRetryAttempt,
    mut outcome: HookDispatchOutcome,
) -> HookDispatchOutcome
where
    FWaitForDelay: FnMut(Duration, usize) -> RetryBackoffDelayOutcome,
    FRunRetryAttempt: FnMut(u32) -> HookDispatchOutcome,
{
    for (retry_attempt, backoff_delay_ms) in backoff_delays_ms.iter().copied().enumerate() {
        match wait_for_delay(Duration::from_millis(backoff_delay_ms), retry_attempt + 1) {
            RetryBackoffDelayOutcome::Elapsed => {}
            RetryBackoffDelayOutcome::StopRequested => {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    retry_attempt = retry_attempt + 1,
                    "Stop requested while waiting for retry_backoff retry; deferring to suspend termination handling"
                );
                break;
            }
            RetryBackoffDelayOutcome::RestartRequested => {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    retry_attempt = retry_attempt + 1,
                    "Restart requested while waiting for retry_backoff retry; deferring to suspend termination handling"
                );
                break;
            }
        }

        outcome = run_retry_attempt(retry_attempt as u32 + 2);

        if outcome.disposition == HookDisposition::Pass {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                retry_attempt = retry_attempt + 1,
                "Lifecycle hook recovered under retry_backoff"
            );
            return outcome;
        }

        if outcome.disposition != HookDisposition::Suspend {
            return outcome;
        }
    }

    warn!(
        phase_event = %phase_event_key,
        hook_name = %hook_name,
        retry_attempts = backoff_delays_ms.len(),
        "Lifecycle hook retry_backoff policy exhausted; entering suspended wait_for_resume fallback"
    );

    outcome
}

pub(super) fn run_wait_then_retry_policy<FWaitForSignal, FClearSuspendState, FRunRetryAttempt>(
    phase_event_key: &str,
    hook_name: &str,
    mut wait_for_signal: FWaitForSignal,
    mut clear_suspend_state: FClearSuspendState,
    mut run_retry_attempt: FRunRetryAttempt,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome
where
    FWaitForSignal: FnMut() -> Result<SuspendWaitOutcome>,
    FClearSuspendState: FnMut() -> Result<()>,
    FRunRetryAttempt: FnMut() -> HookDispatchOutcome,
{
    let wait_outcome = match wait_for_signal() {
        Ok(wait_outcome) => wait_outcome,
        Err(error) => {
            warn!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                error = %error,
                "wait_then_retry gate failed while polling suspend signals; deferring to standard suspend handling"
            );
            return outcome;
        }
    };

    match wait_outcome {
        SuspendWaitOutcome::Stop => {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                "Stop requested while waiting under wait_then_retry; deferring to suspend termination handling"
            );
            outcome
        }
        SuspendWaitOutcome::Restart => {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                "Restart requested while waiting under wait_then_retry; deferring to suspend termination handling"
            );
            outcome
        }
        SuspendWaitOutcome::Resume => {
            if let Err(error) = clear_suspend_state() {
                warn!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    error = %error,
                    "Failed to clear wait_then_retry suspend-state after resume; deferring to standard suspend handling"
                );
                return outcome;
            }

            let retry_outcome = run_retry_attempt();

            if retry_outcome.disposition == HookDisposition::Pass {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    "Lifecycle hook recovered under wait_then_retry"
                );
            }

            retry_outcome
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_hook_attempt(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_attempt: u32,
    retry_max_attempts: u32,
    request: &HookRunRequest,
) -> HookDispatchOutcome {
    match hook_executor.run(request.clone()) {
        Ok(run_result) => {
            let run_disposition = classify_hook_disposition(on_error, &run_result);
            let mutation_parse_outcome =
                parse_hook_mutation_stdout(mutate, hook_name, &run_result.stdout.content);
            let mutation_failure = if run_disposition == HookDisposition::Pass {
                mutation_parse_failure(&mutation_parse_outcome)
            } else {
                None
            };

            let disposition = if mutation_failure.is_some() {
                disposition_from_on_error(on_error)
            } else {
                run_disposition
            };

            let failure = if let Some(mutation_failure) = mutation_failure {
                Some(mutation_failure)
            } else if run_disposition == HookDisposition::Pass {
                None
            } else {
                Some(HookDispatchFailure::HookRunFailed {
                    exit_code: run_result.exit_code,
                    timed_out: run_result.timed_out,
                })
            };

            event_loop.log_hook_run_telemetry(HookRunTelemetryEntry::from_run_result(
                loop_id,
                phase_event_key,
                hook_name,
                disposition,
                suspend_mode,
                retry_attempt,
                retry_max_attempts,
                &run_result,
            ));

            if disposition == HookDisposition::Pass {
                debug!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    duration_ms = run_result.duration_ms,
                    "Lifecycle hook executed successfully"
                );
            } else {
                let failure_detail = format_hook_failure_detail(failure.as_ref());
                warn!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    disposition = ?disposition,
                    exit_code = ?run_result.exit_code,
                    timed_out = run_result.timed_out,
                    failure = %failure_detail,
                    "Lifecycle hook returned non-pass disposition; continuing"
                );
            }

            HookDispatchOutcome {
                phase_event,
                hook_name: hook_name.to_string(),
                disposition,
                suspend_mode,
                failure,
                mutation_parse_outcome,
            }
        }
        Err(error) => {
            let disposition = disposition_from_on_error(on_error);

            warn!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                disposition = ?disposition,
                error = %error,
                "Lifecycle hook execution failed; continuing"
            );

            HookDispatchOutcome {
                phase_event,
                hook_name: hook_name.to_string(),
                disposition,
                suspend_mode,
                failure: Some(HookDispatchFailure::HookExecutionError {
                    message: error.to_string(),
                }),
                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        }
    }
}

pub(super) fn wait_for_retry_backoff_delay_with_signal_poll(
    workspace_root: &Path,
    backoff_delay: Duration,
) -> RetryBackoffDelayOutcome {
    if backoff_delay.is_zero() {
        return RetryBackoffDelayOutcome::Elapsed;
    }

    let poll_interval = Duration::from_millis(RETRY_BACKOFF_SIGNAL_POLL_INTERVAL_MS);
    let sleep_started_at = std::time::Instant::now();

    loop {
        if is_stop_requested(workspace_root) {
            return RetryBackoffDelayOutcome::StopRequested;
        }

        if is_restart_requested(workspace_root) {
            return RetryBackoffDelayOutcome::RestartRequested;
        }

        let elapsed = sleep_started_at.elapsed();
        if elapsed >= backoff_delay {
            return RetryBackoffDelayOutcome::Elapsed;
        }

        let remaining = backoff_delay.saturating_sub(elapsed);
        std::thread::sleep(std::cmp::min(remaining, poll_interval));
    }
}

pub(super) fn wait_for_suspend_signal_with_poll(
    suspend_state_store: &SuspendStateStore,
) -> Result<SuspendWaitOutcome> {
    let poll_interval = Duration::from_millis(SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS);

    loop {
        if is_stop_requested(suspend_state_store.workspace_root()) {
            return Ok(SuspendWaitOutcome::Stop);
        }

        if is_restart_requested(suspend_state_store.workspace_root()) {
            return Ok(SuspendWaitOutcome::Restart);
        }

        if suspend_state_store
            .consume_resume_requested()
            .context("Failed to consume resume signal while suspended")?
        {
            return Ok(SuspendWaitOutcome::Resume);
        }

        std::thread::sleep(poll_interval);
    }
}

pub(super) fn fail_if_blocking_loop_start_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked loop.start boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub(super) fn fail_if_blocking_iteration_start_outcomes(
    outcomes: &[HookDispatchOutcome],
) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked iteration.start boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub(super) fn fail_if_blocking_plan_created_outcomes(
    outcomes: &[HookDispatchOutcome],
) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked plan.created boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub(super) fn fail_if_blocking_human_interact_outcomes(
    outcomes: &[HookDispatchOutcome],
) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked human.interact boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub(super) fn fail_if_blocking_loop_termination_outcomes(
    outcomes: &[HookDispatchOutcome],
) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked loop termination boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub(super) async fn wait_for_resume_if_suspended(
    outcomes: &[HookDispatchOutcome],
    loop_id: &str,
    suspend_state_store: &SuspendStateStore,
) -> Result<Option<TerminationReason>> {
    let Some(suspending_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Suspend)
    else {
        return Ok(None);
    };

    let reason = format_suspending_hook_reason(suspending_outcome);
    let suspend_state = SuspendStateRecord::new(
        loop_id,
        suspending_outcome.phase_event,
        &suspending_outcome.hook_name,
        &reason,
        suspending_outcome.suspend_mode,
        chrono::Utc::now(),
    );

    suspend_state_store
        .write_suspend_state(&suspend_state)
        .with_context(|| {
            format!(
                "Failed to persist suspend-state for hook '{}' at '{}'",
                suspending_outcome.hook_name,
                suspending_outcome.phase_event.as_str()
            )
        })?;

    warn!(
        phase_event = %suspending_outcome.phase_event,
        hook_name = %suspending_outcome.hook_name,
        suspend_mode = ?suspending_outcome.suspend_mode,
        reason = %reason,
        "Lifecycle hook requested suspend; entering wait_for_resume gate"
    );

    loop {
        if consume_stop_requested_signal(suspend_state_store.workspace_root())? {
            clear_suspend_wait_artifacts(suspend_state_store)?;
            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Stop requested while suspended; terminating loop"
            );
            return Ok(Some(TerminationReason::Stopped));
        }

        if is_restart_requested(suspend_state_store.workspace_root()) {
            clear_suspend_wait_artifacts(suspend_state_store)?;
            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Restart requested while suspended; terminating loop for restart"
            );
            return Ok(Some(TerminationReason::RestartRequested));
        }

        if suspend_state_store
            .consume_resume_requested()
            .context("Failed to consume resume signal while suspended")?
        {
            suspend_state_store
                .clear_suspend_state()
                .context("Failed to clear suspend-state after resume signal")?;

            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Resume signal consumed; leaving suspended wait_for_resume state"
            );
            return Ok(None);
        }

        tokio::time::sleep(Duration::from_millis(SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS)).await;
    }
}

pub(super) fn clear_suspend_wait_artifacts(suspend_state_store: &SuspendStateStore) -> Result<()> {
    suspend_state_store
        .clear_suspend_state()
        .context("Failed to clear suspend-state artifact")?;
    suspend_state_store
        .consume_resume_requested()
        .context("Failed to clear stale resume signal")?;
    Ok(())
}

pub(super) fn is_stop_requested(workspace_root: &Path) -> bool {
    workspace_root.join(".ralph/stop-requested").exists()
}

pub(super) fn consume_stop_requested_signal(workspace_root: &Path) -> Result<bool> {
    let stop_path = workspace_root.join(".ralph/stop-requested");
    match fs::remove_file(&stop_path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
            format!(
                "Failed to consume stop signal while suspended: {}",
                stop_path.display()
            )
        }),
    }
}

pub(super) fn is_restart_requested(workspace_root: &Path) -> bool {
    workspace_root.join(".ralph/restart-requested").exists()
}

pub(super) fn format_suspending_hook_reason(outcome: &HookDispatchOutcome) -> String {
    format!(
        "Lifecycle hook '{}' suspended orchestration at '{}': {}",
        outcome.hook_name,
        outcome.phase_event.as_str(),
        format_hook_failure_detail(outcome.failure.as_ref())
    )
}

pub(super) fn format_blocking_hook_reason(outcome: &HookDispatchOutcome) -> String {
    format!(
        "Lifecycle hook '{}' blocked orchestration at '{}': {}",
        outcome.hook_name,
        outcome.phase_event.as_str(),
        format_hook_failure_detail(outcome.failure.as_ref())
    )
}

pub(super) fn format_hook_failure_detail(failure: Option<&HookDispatchFailure>) -> String {
    match failure {
        Some(HookDispatchFailure::HookRunFailed {
            exit_code,
            timed_out,
        }) => {
            if *timed_out {
                "hook timed out".to_string()
            } else if let Some(code) = exit_code {
                format!("hook exited with code {code}")
            } else {
                "hook exited unsuccessfully".to_string()
            }
        }
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            format!("hook execution failed: {message}")
        }
        Some(HookDispatchFailure::InvalidMutationOutput { message }) => {
            format!("invalid mutation output: {message}")
        }
        None => "hook failed without failure details".to_string(),
    }
}

pub(super) fn classify_hook_disposition(
    on_error: HookOnError,
    run_result: &HookRunResult,
) -> HookDisposition {
    if !run_result.timed_out && run_result.exit_code == Some(0) {
        HookDisposition::Pass
    } else {
        disposition_from_on_error(on_error)
    }
}

pub(super) fn disposition_from_on_error(on_error: HookOnError) -> HookDisposition {
    match on_error {
        HookOnError::Warn => HookDisposition::Warn,
        HookOnError::Block => HookDisposition::Block,
        HookOnError::Suspend => HookDisposition::Suspend,
    }
}
