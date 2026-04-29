use super::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::Mutex;

#[test]
fn test_resolve_loop_id_fresh_generates_new() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    let id = resolve_loop_id(&ctx, false, None);
    assert!(
        id.starts_with("primary-"),
        "Fresh run should generate primary-{{timestamp}}, got: {}",
        id
    );
}

#[test]
fn test_resolve_loop_id_continue_reuses_marker() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    // Write a marker from a "previous run"
    std::fs::write(
        ctx.ralph_dir().join("current-loop-id"),
        "primary-20260303-100000",
    )
    .unwrap();

    let id = resolve_loop_id(&ctx, true, None);
    assert_eq!(
        id, "primary-20260303-100000",
        "--continue should reuse existing loop ID"
    );
}

#[test]
fn test_resolve_loop_id_continue_explicit_overrides_marker() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    std::fs::write(
        ctx.ralph_dir().join("current-loop-id"),
        "primary-20260303-100000",
    )
    .unwrap();

    let id = resolve_loop_id(&ctx, true, Some("custom-loop-42"));
    assert_eq!(
        id, "custom-loop-42",
        "--loop-id should override the marker file"
    );
}

#[test]
fn test_resolve_loop_id_continue_no_marker_generates_new() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    // No marker file exists
    let id = resolve_loop_id(&ctx, true, None);
    assert!(
        id.starts_with("primary-"),
        "--continue without marker should fall back to generating new ID, got: {}",
        id
    );
}

#[test]
fn test_pty_only_enabled_for_tui_rpc_or_interactive() {
    let should_use_pty = |enable_tui: bool, enable_rpc: bool, user_interactive: bool| -> bool {
        enable_tui || enable_rpc || user_interactive
    };

    assert!(!should_use_pty(false, false, false));
    assert!(should_use_pty(true, false, false));
    assert!(should_use_pty(false, true, false));
    assert!(should_use_pty(false, false, true));
}

#[test]
fn test_user_interactive_mode_determination() {
    // user_interactive is determined by default_mode setting, not PTY.
    // PTY handles output streaming; user_interactive handles input forwarding.

    // Autonomous mode: no user input forwarding
    let autonomous_interactive = false;
    assert!(
        !autonomous_interactive,
        "Autonomous mode should not forward user input"
    );

    // Interactive mode with TTY: forward user input
    let interactive_with_tty = true;
    assert!(
        interactive_with_tty,
        "Interactive mode with TTY should forward user input"
    );
}

#[test]
fn test_prepare_tui_iteration_seeds_max_iterations() {
    let state = Arc::new(Mutex::new(ralph_tui::TuiState::new()));

    let lines = prepare_tui_iteration(&state, "Planner".to_string(), "claude".to_string(), 42);

    assert!(lines.is_some(), "should return a lines handle");
    let state = state.lock().expect("state lock");
    assert_eq!(state.max_iterations, Some(42));
    assert_eq!(state.total_iterations(), 1);
}

#[cfg(unix)]
fn write_fake_executable(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let script = format!("#!/bin/sh\n{}\n", body);
    std::fs::write(&path, script).expect("write script");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

#[cfg(unix)]
static FAKE_PATH_BACKEND_SERIAL: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(unix)]
static FAKE_PATH_BACKEND_BIN: std::sync::LazyLock<
    std::sync::Mutex<Option<std::path::PathBuf>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

#[cfg(unix)]
struct FakePathBackendsGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    _cwd_lock: crate::test_support::CwdSerialGuard,
    _temp_dir: tempfile::TempDir,
    installed_paths: Vec<std::path::PathBuf>,
}

#[cfg(unix)]
impl Drop for FakePathBackendsGuard {
    fn drop(&mut self) {
        for path in &self.installed_paths {
            let _ = std::fs::remove_file(path);
        }
        // Recover from PoisonError so one panicking test does not cascade
        // into every subsequent test that touches the fake-PATH backend.
        *FAKE_PATH_BACKEND_BIN
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(unix)]
fn install_fake_path_backends(backends: &[(&str, &str)]) -> FakePathBackendsGuard {
    // Recover from PoisonError so a panic in any prior test that held this
    // lock does not cascade PoisonError into every subsequent wave test.
    // Also acquire the cwd serial lock so wave tests (which snapshot
    // std::env::current_dir() for the PTY child's cwd) can not race with
    // test_support::CwdGuard-holding tests. The cwd lock is reentrant,
    // so nested wave-helper calls do not deadlock.
    let guard = FAKE_PATH_BACKEND_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("fake backend temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("fake backend bin dir");

    let mut installed_paths = Vec::with_capacity(backends.len());
    for (name, body) in backends {
        let path = bin_dir.join(name);
        assert!(
            !path.exists(),
            "expected fake backend slot to be free: {}",
            path.display()
        );
        installed_paths.push(write_fake_executable(&bin_dir, name, body));
    }
    *FAKE_PATH_BACKEND_BIN
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(bin_dir.clone());

    FakePathBackendsGuard {
        _guard: guard,
        _cwd_lock: cwd_lock,
        _temp_dir: temp_dir,
        installed_paths,
    }
}

#[cfg(test)]
struct MockAcpExecutionGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for MockAcpExecutionGuard {
    fn drop(&mut self) {
        MOCK_ACP_EXECUTIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

#[cfg(test)]
fn install_mock_acp_executions(executions: Vec<MockAcpExecution>) -> MockAcpExecutionGuard {
    let guard = MOCK_ACP_EXECUTION_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *MOCK_ACP_EXECUTIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = executions.into_iter().collect();
    MockAcpExecutionGuard { _guard: guard }
}

#[cfg(unix)]
fn hook_spec_with_command_and_on_error_and_suspend_mode(
    name: &str,
    command: Vec<String>,
    on_error: HookOnError,
    suspend_mode: Option<HookSuspendMode>,
) -> ralph_core::HookSpec {
    ralph_core::HookSpec {
        name: name.to_string(),
        command,
        cwd: None,
        env: std::collections::HashMap::new(),
        timeout_seconds: None,
        max_output_bytes: None,
        on_error: Some(on_error),
        suspend_mode,
        mutate: ralph_core::HookMutationConfig::default(),
        extra: std::collections::HashMap::new(),
    }
}

#[cfg(unix)]
fn hook_spec_with_command_and_on_error(
    name: &str,
    command: Vec<String>,
    on_error: HookOnError,
) -> ralph_core::HookSpec {
    hook_spec_with_command_and_on_error_and_suspend_mode(name, command, on_error, None)
}

#[cfg(unix)]
fn hook_spec_with_command(name: &str, command: Vec<String>) -> ralph_core::HookSpec {
    hook_spec_with_command_and_on_error(name, command, HookOnError::Warn)
}

#[cfg(unix)]
fn recording_hook(name: &str, log_path: &Path) -> ralph_core::HookSpec {
    hook_spec_with_command(
        name,
        vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"payload="$(cat)"
phase="$(printf '%s' "$payload" | grep -o '"phase_event":"[^"]*"' | cut -d'"' -f4)"
printf '%s|%s\n' "$1" "$phase" >> "$2""#
                .to_string(),
            "hook-recorder".to_string(),
            name.to_string(),
            log_path.to_string_lossy().into_owned(),
        ],
    )
}

#[cfg(unix)]
fn payload_recording_hook(name: &str, log_path: &Path) -> ralph_core::HookSpec {
    hook_spec_with_command(
        name,
        vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"payload="$(cat)"
printf '%s\n' "$payload" >> "$1""#
                .to_string(),
            "hook-payload-recorder".to_string(),
            log_path.to_string_lossy().into_owned(),
        ],
    )
}

#[cfg(unix)]
fn hook_engine_with_events(
    events: std::collections::HashMap<HookPhaseEvent, Vec<ralph_core::HookSpec>>,
) -> HookEngine {
    let hooks_config = ralph_core::HooksConfig {
        enabled: true,
        events,
        ..ralph_core::HooksConfig::default()
    };
    HookEngine::new(&hooks_config)
}

#[cfg(unix)]
fn dispatch_test_event_loop(workspace_root: &Path) -> EventLoop {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    EventLoop::new(config)
}

#[cfg(unix)]
fn dispatch_test_event_loop_with_context(workspace_root: &Path) -> (EventLoop, LoopContext) {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    let context = LoopContext::primary(workspace_root.to_path_buf());
    let event_loop = EventLoop::with_context(config, context.clone());
    (event_loop, context)
}

fn dispatch_test_event_loop_from_yaml_with_context(
    workspace_root: &Path,
    yaml: &str,
) -> (EventLoop, LoopContext) {
    let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("parse config");
    config.core.workspace_root = workspace_root.to_path_buf();
    let context = LoopContext::primary(workspace_root.to_path_buf());
    let event_loop = EventLoop::with_context(config, context.clone());
    (event_loop, context)
}

#[cfg(unix)]
fn dispatch_test_event_loop_with_diagnostics(workspace_root: &Path) -> EventLoop {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    let diagnostics =
        ralph_core::diagnostics::DiagnosticsCollector::with_enabled(workspace_root, true)
            .expect("create diagnostics collector");
    EventLoop::with_diagnostics(config, diagnostics)
}

#[cfg(unix)]
fn read_hook_run_telemetry_entries(workspace_root: &Path) -> Vec<HookRunTelemetryEntry> {
    let diagnostics_root = workspace_root.join(".ralph").join("diagnostics");
    let mut session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_root)
        .expect("read diagnostics root")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());

    let latest_session = session_dirs
        .last()
        .expect("at least one diagnostics session should exist");
    let hook_runs_path = latest_session.path().join("hook-runs.jsonl");
    let content = std::fs::read_to_string(&hook_runs_path).expect("read hook-runs.jsonl");

    content
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse hook run telemetry entry"))
        .collect()
}

#[cfg(unix)]
fn read_hook_log(log_path: &Path) -> Vec<String> {
    std::fs::read_to_string(log_path)
        .expect("read hook log")
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
fn read_hook_payload_log(log_path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log_path)
        .expect("read hook payload log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse hook payload JSON"))
        .collect()
}

fn suspend_outcome_with_mode(
    phase_event: HookPhaseEvent,
    hook_name: &str,
    suspend_mode: HookSuspendMode,
) -> HookDispatchOutcome {
    HookDispatchOutcome {
        phase_event,
        hook_name: hook_name.to_string(),
        disposition: HookDisposition::Suspend,
        suspend_mode,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }
}

fn suspend_outcome(phase_event: HookPhaseEvent, hook_name: &str) -> HookDispatchOutcome {
    suspend_outcome_with_mode(phase_event, hook_name, HookSuspendMode::WaitForResume)
}

fn block_on_test_future<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build tokio runtime")
        .block_on(future)
}

fn empty_hook_metadata() -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::new()
}

fn build_loop_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
) -> HookPayloadBuilderInput {
    super::build_loop_start_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        &empty_hook_metadata(),
    )
}

fn build_iteration_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
) -> HookPayloadBuilderInput {
    super::build_iteration_start_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        &empty_hook_metadata(),
    )
}

fn build_plan_created_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
) -> HookPayloadBuilderInput {
    super::build_plan_created_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        &empty_hook_metadata(),
    )
}

fn build_human_interact_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    human_interact: Option<serde_json::Value>,
) -> HookPayloadBuilderInput {
    super::build_human_interact_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        human_interact,
        &empty_hook_metadata(),
    )
}

fn build_loop_termination_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    termination_reason: &TerminationReason,
) -> HookPayloadBuilderInput {
    super::build_loop_termination_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        termination_reason,
        &empty_hook_metadata(),
    )
}

async fn dispatch_pre_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    let mut accumulated_hook_metadata = serde_json::Map::new();
    super::dispatch_pre_loop_termination_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        suspend_state_store,
        ctx,
        max_iterations,
        &mut accumulated_hook_metadata,
        reason,
    )
    .await
}

async fn dispatch_post_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    let mut accumulated_hook_metadata = serde_json::Map::new();
    super::dispatch_post_loop_termination_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        suspend_state_store,
        ctx,
        max_iterations,
        &mut accumulated_hook_metadata,
        reason,
    )
    .await
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_routes_by_phase_and_preserves_order() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("hook-dispatch.log");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![
            recording_hook("pre-iteration-first", &log_path),
            recording_hook("pre-iteration-second", &log_path),
        ],
    );
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![recording_hook("post-loop-only", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(
        read_hook_log(&log_path),
        vec![
            "pre-iteration-first|pre.iteration.start".to_string(),
            "pre-iteration-second|pre.iteration.start".to_string(),
        ]
    );

    dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(
        read_hook_log(&log_path),
        vec![
            "pre-iteration-first|pre.iteration.start".to_string(),
            "pre-iteration-second|pre.iteration.start".to_string(),
            "post-loop-only|post.loop.start".to_string(),
        ]
    );
}

#[cfg(unix)]
#[test]
fn test_ac13_mutation_disabled_json_output_is_inert_for_accumulator_and_downstream_payloads() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let payload_log_path = temp_dir
        .path()
        .join("hook-metadata-disabled-payloads.jsonl");

    let mut disabled_mutation_spec = hook_spec_with_command(
        "metadata-emitter",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' '{\"metadata\":{\"risk_score\":0.72}}'".to_string(),
        ],
    );
    disabled_mutation_spec.mutate = hook_mutation_config(false);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![disabled_mutation_spec]);
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![payload_recording_hook(
            "payload-recorder",
            &payload_log_path,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut accumulated_hook_metadata = serde_json::Map::new();
    accumulated_hook_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        super::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );

    assert_eq!(pre_outcomes.len(), 1);
    assert_eq!(pre_outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(pre_outcomes[0].failure, None);
    assert_eq!(
        pre_outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Disabled
    );

    let metadata_before_merge = accumulated_hook_metadata.clone();
    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &pre_outcomes,
    );
    assert_eq!(accumulated_hook_metadata, metadata_before_merge);

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        super::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &post_outcomes,
    );

    let payloads = read_hook_payload_log(&payload_log_path);
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        payloads[0]["metadata"]["accumulated"],
        serde_json::json!({"upstream":"preserved"})
    );

    let payload_accumulated = payloads[0]["metadata"]["accumulated"]
        .as_object()
        .expect("metadata.accumulated object");
    assert!(!payload_accumulated.contains_key("hook_metadata"));
}

#[cfg(unix)]
#[test]
fn test_ac14_mutation_enabled_updates_only_namespaced_metadata_in_downstream_payloads() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let payload_log_path = temp_dir.path().join("hook-metadata-enabled-payloads.jsonl");

    let mut mutation_spec = hook_spec_with_command(
        "metadata-emitter",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' '{\"metadata\":{\"risk_score\":0.72,\"gates\":[\"policy_check\"]}}'"
                .to_string(),
        ],
    );
    mutation_spec.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![mutation_spec]);
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![payload_recording_hook(
            "payload-recorder",
            &payload_log_path,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut accumulated_hook_metadata = serde_json::Map::new();
    accumulated_hook_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        super::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    assert!(matches!(
        pre_outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Parsed { .. }
    ));

    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &pre_outcomes,
    );
    assert_eq!(
        serde_json::Value::Object(accumulated_hook_metadata.clone()),
        serde_json::json!({
            "upstream": "preserved",
            "hook_metadata": {
                "metadata-emitter": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        super::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &post_outcomes,
    );

    let payloads = read_hook_payload_log(&payload_log_path);
    assert_eq!(payloads.len(), 1);
    let payload = &payloads[0];

    assert_eq!(payload["phase_event"], serde_json::json!("post.loop.start"));
    assert_eq!(
        payload["context"]["active_hat"],
        serde_json::json!("planner")
    );
    assert_eq!(
        payload["metadata"]["accumulated"],
        serde_json::json!({
            "upstream": "preserved",
            "hook_metadata": {
                "metadata-emitter": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );

    let payload_object = payload.as_object().expect("payload object");
    assert!(!payload_object.contains_key("prompt"));
    assert!(!payload_object.contains_key("events"));
    assert!(!payload_object.contains_key("config"));

    let context = payload["context"]
        .as_object()
        .expect("payload context object");
    assert!(!context.contains_key("prompt"));
    assert!(!context.contains_key("events"));
    assert!(!context.contains_key("config"));

    let payload_accumulated = payload["metadata"]["accumulated"]
        .as_object()
        .expect("metadata.accumulated object");
    assert!(!payload_accumulated.contains_key("risk_score"));
    assert!(!payload_accumulated.contains_key("gates"));
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_noop_when_disabled_or_unconfigured() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("hook-noop.log");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![recording_hook("should-not-run", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let empty_engine = hook_engine_with_events(std::collections::HashMap::new());
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let disabled_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        false,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    let empty_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &empty_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    let mismatched_phase_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert!(
        disabled_outcomes.is_empty(),
        "disabled hooks must be a no-op"
    );
    assert!(
        empty_outcomes.is_empty(),
        "empty hooks config must be a no-op"
    );
    assert!(
        mismatched_phase_outcomes.is_empty(),
        "dispatching a phase without hooks must be a no-op"
    );
    assert!(
        !log_path.exists(),
        "hook log should not be created on no-op paths"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_returns_dispositions_and_failure_context() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![
            hook_spec_with_command(
                "hook-pass",
                vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()],
            ),
            hook_spec_with_command(
                "hook-warn",
                vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()],
            ),
            hook_spec_with_command_and_on_error(
                "hook-block",
                vec!["sh".to_string(), "-c".to_string(), "exit 23".to_string()],
                HookOnError::Block,
            ),
        ],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 3);

    assert_eq!(outcomes[0].hook_name, "hook-pass");
    assert_eq!(outcomes[0].phase_event, HookPhaseEvent::PreLoopStart);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert!(outcomes[0].failure.is_none());

    assert_eq!(outcomes[1].hook_name, "hook-warn");
    assert_eq!(outcomes[1].disposition, HookDisposition::Warn);
    assert_eq!(
        outcomes[1].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(7),
            timed_out: false,
        })
    );

    assert_eq!(outcomes[2].hook_name, "hook-block");
    assert_eq!(outcomes[2].disposition, HookDisposition::Block);
    assert_eq!(
        outcomes[2].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(23),
            timed_out: false,
        })
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_maps_executor_failures_to_on_error_disposition() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![
            hook_spec_with_command(
                "warn-exec-error",
                vec!["definitely-not-a-real-exec-warn".to_string()],
            ),
            hook_spec_with_command_and_on_error(
                "block-exec-error",
                vec!["definitely-not-a-real-exec-block".to_string()],
                HookOnError::Block,
            ),
        ],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].hook_name, "warn-exec-error");
    assert_eq!(outcomes[0].disposition, HookDisposition::Warn);
    match &outcomes[0].failure {
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            assert!(
                message.contains("definitely-not-a-real-exec-warn"),
                "executor failure context should include missing command"
            );
        }
        other => panic!("expected execution error failure context, got {other:?}"),
    }

    assert_eq!(outcomes[1].hook_name, "block-exec-error");
    assert_eq!(outcomes[1].disposition, HookDisposition::Block);
    match &outcomes[1].failure {
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            assert!(
                message.contains("definitely-not-a-real-exec-block"),
                "executor failure context should include missing command"
            );
        }
        other => panic!("expected execution error failure context, got {other:?}"),
    }
}

// AC-15: JSON-only mutation format errors must flow through lifecycle on_error dispositions.
#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_warn_continues_through_block_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut warn_hook = hook_spec_with_command_and_on_error(
        "warn-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Warn,
    );
    warn_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![warn_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Warn);
    assert!(matches!(
        outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Invalid(_)
    ));
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));
    assert!(fail_if_blocking_loop_start_outcomes(&outcomes).is_ok());
}

#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_block_surfaces_invalid_output_reason()
{
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut block_hook = hook_spec_with_command_and_on_error(
        "block-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Block,
    );
    block_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![block_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Block);
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));

    let block_error = fail_if_blocking_loop_start_outcomes(&outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let block_message = block_error.to_string();
    assert!(block_message.contains("block-invalid-mutation"));
    assert!(block_message.contains("pre.loop.start"));
    assert!(block_message.contains("invalid mutation output"));
    assert!(block_message.contains("not valid JSON"));
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_runtime_failure_takes_precedence_over_mutation_parse_error()
{
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut block_hook = hook_spec_with_command_and_on_error(
        "block-runtime-failure",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'; exit 23".to_string(),
        ],
        HookOnError::Block,
    );
    block_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![block_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Block);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(23),
            timed_out: false,
        })
    );
    assert!(matches!(
        outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Invalid(_)
    ));

    let block_error = fail_if_blocking_loop_start_outcomes(&outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let block_message = block_error.to_string();
    assert!(block_message.contains("hook exited with code 23"));
    assert!(!block_message.contains("invalid mutation output"));
}

#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_suspend_uses_wait_for_resume_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut suspend_hook = hook_spec_with_command_and_on_error(
        "suspend-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Suspend,
    );
    suspend_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreIterationStart, vec![suspend_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));
    assert!(fail_if_blocking_iteration_start_outcomes(&outcomes).is_ok());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "suspend-state should be written before resume"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let suspend_state = resume_store
            .read_suspend_state()
            .expect("read suspend-state")
            .expect("suspend-state should exist while waiting");
        assert!(suspend_state.reason.contains("invalid mutation output"));
        assert!(suspend_state.reason.contains("not valid JSON"));

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(wait_result, None);
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after resume")
            .is_none(),
        "suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume-requested should be consumed after resume"
    );
}

#[cfg(unix)]
#[test]
fn test_loop_start_dispatch_warn_continues_and_block_aborts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error(
            "warn-pre-loop-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 17".to_string()],
            HookOnError::Warn,
        )],
    );
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![hook_spec_with_command_and_on_error(
            "block-post-loop-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 29".to_string()],
            HookOnError::Block,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 0, None),
    );

    assert_eq!(pre_loop_start_outcomes.len(), 1);
    assert_eq!(
        pre_loop_start_outcomes[0].disposition,
        HookDisposition::Warn
    );
    assert_eq!(
        pre_loop_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(17),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_loop_start_outcomes(&pre_loop_start_outcomes).is_ok(),
        "warn disposition should continue across loop.start boundary"
    );

    let post_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
        ),
    );

    assert_eq!(post_loop_start_outcomes.len(), 1);
    assert_eq!(
        post_loop_start_outcomes[0].disposition,
        HookDisposition::Block
    );
    let post_loop_start_error = fail_if_blocking_loop_start_outcomes(&post_loop_start_outcomes)
        .expect_err("block disposition should abort loop.start boundary");
    let post_loop_start_message = post_loop_start_error.to_string();
    assert!(post_loop_start_message.contains("block-post-loop-start"));
    assert!(post_loop_start_message.contains("post.loop.start"));
    assert!(post_loop_start_message.contains("hook exited with code 29"));
}

#[cfg(unix)]
#[test]
fn test_iteration_start_dispatch_warn_continues_and_block_aborts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "warn-pre-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 19".to_string()],
            HookOnError::Warn,
        )],
    );
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "block-post-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 31".to_string()],
            HookOnError::Block,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(pre_iteration_start_outcomes.len(), 1);
    assert_eq!(
        pre_iteration_start_outcomes[0].disposition,
        HookDisposition::Warn
    );
    assert_eq!(
        pre_iteration_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(19),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes).is_ok(),
        "warn disposition should continue across iteration.start boundary"
    );

    let post_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(post_iteration_start_outcomes.len(), 1);
    assert_eq!(
        post_iteration_start_outcomes[0].disposition,
        HookDisposition::Block
    );
    let post_iteration_start_error =
        fail_if_blocking_iteration_start_outcomes(&post_iteration_start_outcomes)
            .expect_err("block disposition should abort iteration.start boundary");
    let post_iteration_start_message = post_iteration_start_error.to_string();
    assert!(post_iteration_start_message.contains("block-post-iteration-start"));
    assert!(post_iteration_start_message.contains("post.iteration.start"));
    assert!(post_iteration_start_message.contains("hook exited with code 31"));
}

#[cfg(unix)]
#[test]
fn test_plan_created_lifecycle_hooks_dispatch_only_for_semantic_plan_batches() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"task.start","payload":"noop","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .expect("write non-plan event");
    events_file.flush().expect("flush non-plan event");

    let log_path = temp_dir.path().join("plan-created-hook-payloads.jsonl");
    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PrePlanCreated,
        vec![payload_recording_hook("pre-plan-created", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostPlanCreated,
        vec![payload_recording_hook("post-plan-created", &log_path)],
    );
    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();

    assert!(
        !event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek non-plan events"),
        "non-plan batches must not trigger pre.plan.created"
    );

    let processed_non_plan = event_loop
        .process_events_from_jsonl()
        .expect("process non-plan batch");
    assert!(processed_non_plan.had_events);
    assert!(
        !processed_non_plan.had_plan_events,
        "non-plan batches must not trigger post.plan.created"
    );
    assert!(
        !log_path.exists(),
        "plan.created hooks should not run for non-plan batches"
    );

    writeln!(
        events_file,
        r#"{{"topic":"plan.created","payload":"ready","ts":"2024-01-01T00:00:01Z"}}"#
    )
    .expect("write plan event");
    events_file.flush().expect("flush plan event");

    assert!(
        event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek plan events"),
        "plan.* batches should trigger pre.plan.created"
    );

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PrePlanCreated,
        build_plan_created_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
        ),
    );
    assert!(fail_if_blocking_plan_created_outcomes(&pre_outcomes).is_ok());

    let processed_plan = event_loop
        .process_events_from_jsonl()
        .expect("process plan batch");
    assert!(processed_plan.had_events);
    assert!(
        processed_plan.had_plan_events,
        "plan.* batches should trigger post.plan.created"
    );

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostPlanCreated,
        build_plan_created_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
        ),
    );
    assert!(fail_if_blocking_plan_created_outcomes(&post_outcomes).is_ok());

    let payloads = read_hook_payload_log(&log_path);
    let observed_phases: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["phase_event"]
                .as_str()
                .expect("phase_event should be present")
        })
        .collect();

    assert_eq!(
        observed_phases,
        vec!["pre.plan.created", "post.plan.created"],
        "plan.created hooks should dispatch exactly once around semantic plan batches"
    );
}

#[cfg(unix)]
#[test]
fn test_human_interact_lifecycle_hooks_dispatch_with_post_outcome_context() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"human.interact","payload":"Need approval?","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .expect("write human.interact event");
    events_file.flush().expect("flush human.interact event");

    let log_path = temp_dir.path().join("human-interact-hook-payloads.jsonl");
    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreHumanInteract,
        vec![payload_recording_hook("pre-human-interact", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostHumanInteract,
        vec![payload_recording_hook("post-human-interact", &log_path)],
    );
    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();

    let pending_context = event_loop
        .pending_human_interact_context_in_jsonl()
        .expect("peek pending human.interact context")
        .expect("pending human.interact context should exist");
    assert_eq!(
        pending_context["question"],
        serde_json::json!("Need approval?")
    );
    assert!(
        pending_context.get("outcome").is_none(),
        "pre human.interact context should not include an outcome"
    );

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreHumanInteract,
        build_human_interact_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
            Some(pending_context),
        ),
    );
    assert!(fail_if_blocking_human_interact_outcomes(&pre_outcomes).is_ok());

    let processed = event_loop
        .process_events_from_jsonl()
        .expect("process human.interact batch");
    let post_context = processed
        .human_interact_context
        .expect("processed context should include human.interact outcome");
    assert_eq!(
        post_context["question"],
        serde_json::json!("Need approval?")
    );
    assert_eq!(
        post_context["outcome"],
        serde_json::json!("no_robot_service")
    );

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostHumanInteract,
        build_human_interact_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
            Some(post_context),
        ),
    );
    assert!(fail_if_blocking_human_interact_outcomes(&post_outcomes).is_ok());

    let payloads = read_hook_payload_log(&log_path);
    assert_eq!(payloads.len(), 2);
    assert_eq!(
        payloads[0]["phase_event"],
        serde_json::json!("pre.human.interact")
    );
    assert_eq!(
        payloads[0]["context"]["human_interact"]["question"],
        serde_json::json!("Need approval?")
    );
    assert!(
        payloads[0]["context"]["human_interact"]
            .get("outcome")
            .is_none(),
        "pre.human.interact payload should not include outcome"
    );

    assert_eq!(
        payloads[1]["phase_event"],
        serde_json::json!("post.human.interact")
    );
    assert_eq!(
        payloads[1]["context"]["human_interact"]["question"],
        serde_json::json!("Need approval?")
    );
    assert_eq!(
        payloads[1]["context"]["human_interact"]["outcome"],
        serde_json::json!("no_robot_service")
    );
}

#[cfg(unix)]
#[test]
fn test_loop_termination_lifecycle_hooks_dispatch_complete_and_error_boundaries() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("loop-termination-hook-payloads.jsonl");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopComplete,
        vec![payload_recording_hook("pre-loop-complete", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostLoopComplete,
        vec![payload_recording_hook("post-loop-complete", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PreLoopError,
        vec![payload_recording_hook("pre-loop-error", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostLoopError,
        vec![payload_recording_hook("post-loop-error", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let completed_reason = block_on_test_future(dispatch_pre_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        TerminationReason::CompletionPromise,
    ))
    .expect("pre.loop.complete dispatch should succeed");
    let completed_reason = block_on_test_future(dispatch_post_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        completed_reason,
    ))
    .expect("post.loop.complete dispatch should succeed");
    assert_eq!(completed_reason, TerminationReason::CompletionPromise);

    let error_reason = block_on_test_future(dispatch_pre_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        TerminationReason::MaxRuntime,
    ))
    .expect("pre.loop.error dispatch should succeed");
    let error_reason = block_on_test_future(dispatch_post_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        error_reason,
    ))
    .expect("post.loop.error dispatch should succeed");
    assert_eq!(error_reason, TerminationReason::MaxRuntime);

    let payloads = read_hook_payload_log(&log_path);
    let phases: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["phase_event"]
                .as_str()
                .expect("phase_event should be present")
        })
        .collect();
    let reasons: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["context"]["termination_reason"]
                .as_str()
                .expect("termination_reason should be present")
        })
        .collect();

    assert_eq!(
        phases,
        vec![
            "pre.loop.complete",
            "post.loop.complete",
            "pre.loop.error",
            "post.loop.error"
        ]
    );
    assert_eq!(
        reasons,
        vec!["completed", "completed", "max_runtime", "max_runtime"]
    );
}

#[cfg(unix)]
#[test]
fn test_iteration_start_suspend_waits_for_resume_and_clears_artifacts_before_continuing() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "suspend-pre-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 41".to_string()],
            HookOnError::Suspend,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(pre_iteration_start_outcomes.len(), 1);
    assert_eq!(
        pre_iteration_start_outcomes[0].disposition,
        HookDisposition::Suspend
    );
    assert_eq!(
        pre_iteration_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes).is_ok(),
        "suspend disposition should not block iteration.start boundary"
    );

    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let wait_result = block_on_test_future(async {
        let wait_outcomes = pre_iteration_start_outcomes.clone();
        let wait_store = suspend_state_store.clone();
        let wait_handle = tokio::spawn(async move {
            wait_for_resume_if_suspended(&wait_outcomes, "loop-test", &wait_store).await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if suspend_state_store.suspend_state_path().exists() {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("suspend-state should be written before resume");

        let suspend_state = suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state")
            .expect("suspend-state should exist while waiting for resume");

        assert_eq!(suspend_state.loop_id, "loop-test");
        assert_eq!(suspend_state.phase_event, HookPhaseEvent::PreIterationStart);
        assert_eq!(suspend_state.hook_name, "suspend-pre-iteration-start");
        assert_eq!(suspend_state.suspend_mode, HookSuspendMode::WaitForResume);
        assert!(!suspend_state_store.resume_requested_path().exists());

        suspend_state_store
            .write_resume_requested()
            .expect("write resume signal");

        tokio::time::timeout(Duration::from_secs(2), wait_handle)
            .await
            .expect("wait_for_resume helper should complete after resume signal")
            .expect("wait_for_resume task should not panic")
    })
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after resume")
            .is_none(),
        "suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume-requested should be consumed after resume"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_recovers_before_exhaustion() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-pre-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
if [ "$attempt" -lt 3 ]; then
  exit 41
fi
exit 0"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop_with_diagnostics(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::RetryBackoff);
    assert_eq!(outcomes[0].failure, None);

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(attempts.trim(), "3", "hook should recover on third attempt");

    let telemetry_entries = read_hook_run_telemetry_entries(temp_dir.path());
    assert_eq!(telemetry_entries.len(), 3);
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.retry_attempt)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.retry_max_attempts == 4)
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.suspend_mode == HookSuspendMode::RetryBackoff)
    );
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.disposition)
            .collect::<Vec<_>>(),
        vec![
            HookDisposition::Suspend,
            HookDisposition::Suspend,
            HookDisposition::Pass,
        ]
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_exhausts_to_suspend() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-post-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 51"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::RetryBackoff);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(51),
            timed_out: false,
        })
    );

    let attempts: usize = std::fs::read_to_string(&attempts_path)
        .expect("read attempts")
        .trim()
        .parse()
        .expect("parse attempts");
    assert_eq!(
        attempts,
        RETRY_BACKOFF_DELAYS_MS.len() + 1,
        "retry_backoff should cap retries at the configured schedule"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_yields_to_stop_signal() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");
    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "")
        .expect("write stop signal");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-pre-loop-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 61"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "1",
        "stop signal should short-circuit retry_backoff retries"
    );

    let suspend_state_store = SuspendStateStore::new(temp_dir.path());
    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_recovers_after_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-pre-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
if [ "$attempt" -lt 2 ]; then
  exit 71
fi
exit 0"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop_with_diagnostics(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "wait_then_retry should persist suspend-state before waiting"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::WaitThenRetry);
    assert_eq!(outcomes[0].failure, None);

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "2",
        "wait_then_retry should run exactly one retry after resume"
    );
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after wait_then_retry")
            .is_none(),
        "suspend-state should be cleared after wait_then_retry resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume signal should be consumed under wait_then_retry"
    );

    let telemetry_entries = read_hook_run_telemetry_entries(temp_dir.path());
    assert_eq!(telemetry_entries.len(), 2);
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.retry_attempt)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.retry_max_attempts == 2)
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.suspend_mode == HookSuspendMode::WaitThenRetry)
    );
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.disposition)
            .collect::<Vec<_>>(),
        vec![HookDisposition::Suspend, HookDisposition::Pass]
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_retry_failure_remains_suspended() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-post-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 72"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "wait_then_retry should persist suspend-state before waiting"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::WaitThenRetry);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(72),
            timed_out: false,
        })
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "2",
        "wait_then_retry should run a single retry attempt after resume"
    );
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after wait_then_retry")
            .is_none(),
        "first wait_then_retry suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume signal should be consumed after wait_then_retry"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_prioritizes_stop_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");
    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "")
        .expect("write stop signal");
    std::fs::write(temp_dir.path().join(".ralph/resume-requested"), "")
        .expect("write resume signal");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-pre-loop-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 73"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "1",
        "stop signal should prevent wait_then_retry from running the retry"
    );

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_run_retry_backoff_policy_replays_configured_schedule_deterministically() {
    let mut observed_delays_ms = Vec::new();
    let mut observed_retry_attempts = Vec::new();

    let outcome = run_retry_backoff_policy(
        "pre.iteration.start",
        "retry-hook",
        &[3, 5, 8],
        |delay, retry_attempt| {
            observed_delays_ms.push(delay.as_millis() as u64);
            assert_eq!(retry_attempt, observed_delays_ms.len());
            RetryBackoffDelayOutcome::Elapsed
        },
        |retry_attempt| {
            observed_retry_attempts.push(retry_attempt);
            if retry_attempt == 4 {
                HookDispatchOutcome {
                    phase_event: HookPhaseEvent::PreIterationStart,
                    hook_name: "retry-hook".to_string(),
                    disposition: HookDisposition::Pass,
                    suspend_mode: HookSuspendMode::RetryBackoff,
                    failure: None,

                    mutation_parse_outcome: HookMutationParseOutcome::Disabled,
                }
            } else {
                suspend_outcome_with_mode(
                    HookPhaseEvent::PreIterationStart,
                    "retry-hook",
                    HookSuspendMode::RetryBackoff,
                )
            }
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PreIterationStart,
            "retry-hook",
            HookSuspendMode::RetryBackoff,
        ),
    );

    assert_eq!(observed_delays_ms, vec![3, 5, 8]);
    assert_eq!(observed_retry_attempts, vec![2, 3, 4]);
    assert_eq!(outcome.disposition, HookDisposition::Pass);
    assert_eq!(outcome.failure, None);
}

#[test]
fn test_run_retry_backoff_policy_exhausts_after_last_configured_delay() {
    let mut observed_retry_attempts = Vec::new();

    let outcome = run_retry_backoff_policy(
        "post.iteration.start",
        "retry-hook",
        &[11, 13],
        |_delay, _retry_attempt| RetryBackoffDelayOutcome::Elapsed,
        |retry_attempt| {
            observed_retry_attempts.push(retry_attempt);
            suspend_outcome_with_mode(
                HookPhaseEvent::PostIterationStart,
                "retry-hook",
                HookSuspendMode::RetryBackoff,
            )
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PostIterationStart,
            "retry-hook",
            HookSuspendMode::RetryBackoff,
        ),
    );

    assert_eq!(observed_retry_attempts, vec![2, 3]);
    assert_eq!(outcome.disposition, HookDisposition::Suspend);
    assert_eq!(
        outcome.failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
}

#[test]
fn test_run_retry_backoff_policy_stop_signal_short_circuits_before_retry_attempt() {
    let initial_outcome = suspend_outcome_with_mode(
        HookPhaseEvent::PreLoopStart,
        "retry-hook",
        HookSuspendMode::RetryBackoff,
    );
    let mut retry_attempt_called = false;

    let outcome = run_retry_backoff_policy(
        "pre.loop.start",
        "retry-hook",
        &[21, 34],
        |_delay, _retry_attempt| RetryBackoffDelayOutcome::StopRequested,
        |_retry_attempt| {
            retry_attempt_called = true;
            initial_outcome.clone()
        },
        initial_outcome.clone(),
    );

    assert!(!retry_attempt_called);
    assert_eq!(outcome, initial_outcome);
}

#[test]
fn test_run_wait_then_retry_policy_resume_retries_once_and_returns_retry_result() {
    let mut clear_suspend_calls = 0usize;
    let mut retry_calls = 0usize;

    let outcome = run_wait_then_retry_policy(
        "pre.iteration.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Resume),
        || {
            clear_suspend_calls += 1;
            Ok(())
        },
        || {
            retry_calls += 1;
            HookDispatchOutcome {
                phase_event: HookPhaseEvent::PreIterationStart,
                hook_name: "wait-hook".to_string(),
                disposition: HookDisposition::Pass,
                suspend_mode: HookSuspendMode::WaitThenRetry,
                failure: None,

                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PreIterationStart,
            "wait-hook",
            HookSuspendMode::WaitThenRetry,
        ),
    );

    assert_eq!(clear_suspend_calls, 1);
    assert_eq!(retry_calls, 1);
    assert_eq!(outcome.disposition, HookDisposition::Pass);
    assert_eq!(outcome.failure, None);
}

#[test]
fn test_run_wait_then_retry_policy_retry_failure_returns_suspend() {
    let mut clear_suspend_calls = 0usize;
    let mut retry_calls = 0usize;

    let outcome = run_wait_then_retry_policy(
        "post.iteration.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Resume),
        || {
            clear_suspend_calls += 1;
            Ok(())
        },
        || {
            retry_calls += 1;
            suspend_outcome_with_mode(
                HookPhaseEvent::PostIterationStart,
                "wait-hook",
                HookSuspendMode::WaitThenRetry,
            )
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PostIterationStart,
            "wait-hook",
            HookSuspendMode::WaitThenRetry,
        ),
    );

    assert_eq!(clear_suspend_calls, 1);
    assert_eq!(retry_calls, 1);
    assert_eq!(outcome.disposition, HookDisposition::Suspend);
    assert_eq!(
        outcome.failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
}

#[test]
fn test_run_wait_then_retry_policy_stop_skips_retry_path() {
    let initial_outcome = suspend_outcome_with_mode(
        HookPhaseEvent::PreLoopStart,
        "wait-hook",
        HookSuspendMode::WaitThenRetry,
    );
    let mut clear_suspend_called = false;
    let mut retry_called = false;

    let outcome = run_wait_then_retry_policy(
        "pre.loop.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Stop),
        || {
            clear_suspend_called = true;
            Ok(())
        },
        || {
            retry_called = true;
            HookDispatchOutcome {
                phase_event: HookPhaseEvent::PreLoopStart,
                hook_name: "wait-hook".to_string(),
                disposition: HookDisposition::Pass,
                suspend_mode: HookSuspendMode::WaitThenRetry,
                failure: None,

                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        },
        initial_outcome.clone(),
    );

    assert!(!clear_suspend_called);
    assert!(!retry_called);
    assert_eq!(outcome, initial_outcome);
}

#[test]
fn test_fail_if_blocking_loop_start_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreLoopStart,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(7),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostLoopStart,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_loop_start_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_loop_start_outcomes_surfaces_failure_context() {
    let blocked_exit_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostLoopStart,
        hook_name: "block-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(42),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exit_error = fail_if_blocking_loop_start_outcomes(&blocked_exit_outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let blocked_exit_message = blocked_exit_error.to_string();
    assert!(blocked_exit_message.contains("block-hook"));
    assert!(blocked_exit_message.contains("post.loop.start"));
    assert!(blocked_exit_message.contains("hook exited with code 42"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopStart,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_loop_start_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("pre.loop.start"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_fail_if_blocking_iteration_start_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreIterationStart,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(9),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostIterationStart,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_iteration_start_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_iteration_start_outcomes_surfaces_failure_context() {
    let blocked_timeout_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreIterationStart,
        hook_name: "block-timeout-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: None,
            timed_out: true,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_timeout_error =
        fail_if_blocking_iteration_start_outcomes(&blocked_timeout_outcomes)
            .expect_err("block disposition should fail iteration.start boundary");
    let blocked_timeout_message = blocked_timeout_error.to_string();
    assert!(blocked_timeout_message.contains("block-timeout-hook"));
    assert!(blocked_timeout_message.contains("pre.iteration.start"));
    assert!(blocked_timeout_message.contains("hook timed out"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostIterationStart,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_iteration_start_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail iteration.start boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("post.iteration.start"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_fail_if_blocking_human_interact_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreHumanInteract,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(9),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostHumanInteract,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_human_interact_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_human_interact_outcomes_surfaces_failure_context() {
    let blocked_timeout_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostHumanInteract,
        hook_name: "block-timeout-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: None,
            timed_out: true,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_timeout_error =
        fail_if_blocking_human_interact_outcomes(&blocked_timeout_outcomes)
            .expect_err("block disposition should fail human.interact boundary");
    let blocked_timeout_message = blocked_timeout_error.to_string();
    assert!(blocked_timeout_message.contains("block-timeout-hook"));
    assert!(blocked_timeout_message.contains("post.human.interact"));
    assert!(blocked_timeout_message.contains("hook timed out"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreHumanInteract,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_human_interact_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail human.interact boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("pre.human.interact"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_loop_termination_phase_events_maps_success_and_error_reasons() {
    assert_eq!(
        loop_termination_phase_events(&TerminationReason::CompletionPromise),
        (
            HookPhaseEvent::PreLoopComplete,
            HookPhaseEvent::PostLoopComplete
        )
    );
    assert_eq!(
        loop_termination_phase_events(&TerminationReason::MaxRuntime),
        (HookPhaseEvent::PreLoopError, HookPhaseEvent::PostLoopError)
    );
}

#[test]
fn test_build_loop_termination_payload_input_sets_termination_reason_context() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let payload_input = build_loop_termination_payload_input(
        "loop-test",
        &loop_ctx,
        42,
        7,
        Some("planner".to_string()),
        Some("builder".to_string()),
        Some("task-123".to_string()),
        &TerminationReason::RestartRequested,
    );

    assert_eq!(
        payload_input.context.termination_reason.as_deref(),
        Some("restart_requested")
    );
    assert_eq!(payload_input.context.active_hat.as_deref(), Some("planner"));
    assert_eq!(
        payload_input.context.selected_hat.as_deref(),
        Some("builder")
    );
    assert_eq!(
        payload_input.context.selected_task.as_deref(),
        Some("task-123")
    );
}

fn hook_mutation_config(enabled: bool) -> HookMutationConfig {
    HookMutationConfig {
        enabled,
        format: Some("json".to_string()),
        extra: std::collections::HashMap::new(),
    }
}

fn json_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().expect("json object")
}

#[test]
fn test_parse_hook_mutation_stdout_skips_when_disabled() {
    let outcome =
        parse_hook_mutation_stdout(&HookMutationConfig::default(), "env-guard", "not-json");

    assert_eq!(outcome, HookMutationParseOutcome::Disabled);
}

#[test]
fn test_parse_hook_mutation_stdout_accepts_metadata_only_payload_and_namespaces_by_hook() {
    let outcome = parse_hook_mutation_stdout(
        &hook_mutation_config(true),
        "env-guard",
        r#"{"metadata":{"risk_score":0.72,"gates":["policy_check"]}}"#,
    );

    let HookMutationParseOutcome::Parsed {
        namespaced_metadata,
    } = outcome
    else {
        panic!("expected parsed mutation payload");
    };

    assert_eq!(
        serde_json::Value::Object(namespaced_metadata),
        serde_json::json!({
            "hook_metadata": {
                "env-guard": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );
}

#[test]
fn test_parse_hook_mutation_stdout_rejects_non_json_payload_when_enabled() {
    let outcome = parse_hook_mutation_stdout(&hook_mutation_config(true), "env-guard", "oops");

    let HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidJson { message }) =
        outcome
    else {
        panic!("expected invalid-json mutation parse outcome");
    };

    assert!(message.contains("valid JSON"));
}

#[test]
fn test_parse_hook_mutation_stdout_rejects_non_metadata_payload_shape() {
    let outcome = parse_hook_mutation_stdout(
        &hook_mutation_config(true),
        "env-guard",
        r#"{"metadata":{"risk_score":0.72},"prompt":"inject"}"#,
    );

    let HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema { message }) =
        outcome
    else {
        panic!("expected invalid-schema mutation parse outcome");
    };

    assert!(message.contains("supports only"));
}

#[test]
fn test_merge_hook_metadata_namespace_merges_multiple_hook_entries() {
    let mut accumulated_metadata = serde_json::Map::new();
    accumulated_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "env-guard",
        json_object(serde_json::json!({"risk_score": 0.72})),
    )
    .expect("merge env-guard metadata");

    merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "policy-gate",
        json_object(serde_json::json!({"status": "pass"})),
    )
    .expect("merge policy-gate metadata");

    assert_eq!(
        accumulated_metadata["upstream"],
        serde_json::json!("preserved")
    );
    assert_eq!(
        accumulated_metadata["hook_metadata"]["env-guard"]["risk_score"],
        serde_json::json!(0.72)
    );
    assert_eq!(
        accumulated_metadata["hook_metadata"]["policy-gate"]["status"],
        serde_json::json!("pass")
    );
}

#[test]
fn test_merge_hook_metadata_namespace_rejects_non_object_namespace_value() {
    let mut accumulated_metadata = serde_json::Map::new();
    accumulated_metadata.insert(
        "hook_metadata".to_string(),
        serde_json::Value::String("invalid".to_string()),
    );

    let merge_result = merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "env-guard",
        json_object(serde_json::json!({"risk_score": 0.72})),
    );

    assert!(matches!(
        merge_result,
        Err(HookMutationParseError::InvalidSchema { message })
        if message.contains("must be a JSON object")
    ));
}

#[test]
fn test_fail_if_blocking_loop_termination_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreLoopComplete,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(9),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostLoopError,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_loop_termination_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_loop_termination_outcomes_surfaces_failure_context() {
    let blocked_timeout_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostLoopError,
        hook_name: "block-timeout-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: None,
            timed_out: true,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_timeout_error =
        fail_if_blocking_loop_termination_outcomes(&blocked_timeout_outcomes)
            .expect_err("block disposition should fail loop termination boundary");
    let blocked_timeout_message = blocked_timeout_error.to_string();
    assert!(blocked_timeout_message.contains("block-timeout-hook"));
    assert!(blocked_timeout_message.contains("post.loop.error"));
    assert!(blocked_timeout_message.contains("hook timed out"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopComplete,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_loop_termination_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail loop termination boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("pre.loop.complete"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_wait_for_resume_if_suspended_is_noop_without_suspend_dispositions() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopStart,
        hook_name: "warn-hook".to_string(),
        disposition: HookDisposition::Warn,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(7),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_wait_for_resume_if_suspended_resumes_and_clears_suspend_artifacts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PreLoopStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_wait_for_resume_if_suspended_prioritizes_stop_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "")
        .expect("write stop signal");
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PreIterationStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_wait_for_resume_if_suspended_prioritizes_restart_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/restart-requested"), "")
        .expect("write restart signal");
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PostIterationStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::RestartRequested));
    assert!(temp_dir.path().join(".ralph/restart-requested").exists());
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_idle_timeout_interactive_mode_continues() {
    // Given: interactive mode and IdleTimeout termination
    let termination_type = ralph_adapters::TerminationType::IdleTimeout;
    let interactive = true;

    // When: converting termination type
    let result = convert_termination_type(termination_type, interactive);

    // Then: should return None (allow iteration to continue)
    assert!(
        result.is_none(),
        "Interactive mode idle timeout should return None to allow iteration progression"
    );
}

#[test]
fn test_idle_timeout_autonomous_mode_stops() {
    // Given: autonomous mode and IdleTimeout termination
    let termination_type = ralph_adapters::TerminationType::IdleTimeout;
    let interactive = false;

    // When: converting termination type
    let result = convert_termination_type(termination_type, interactive);

    // Then: should return Some(Stopped)
    assert_eq!(
        result,
        Some(TerminationReason::Stopped),
        "Autonomous mode idle timeout should return Stopped"
    );
}

#[test]
fn test_natural_termination_always_continues() {
    // Given: Natural termination in any mode
    let termination_type = ralph_adapters::TerminationType::Natural;

    // When/Then: should return None regardless of mode
    assert!(
        convert_termination_type(termination_type.clone(), true).is_none(),
        "Natural termination should continue in interactive mode"
    );
    assert!(
        convert_termination_type(termination_type, false).is_none(),
        "Natural termination should continue in autonomous mode"
    );
}

#[test]
fn test_user_interrupt_always_terminates() {
    // Given: UserInterrupt termination in any mode
    let termination_type = ralph_adapters::TerminationType::UserInterrupt;

    // When/Then: should return Interrupted regardless of mode
    assert_eq!(
        convert_termination_type(termination_type.clone(), true),
        Some(TerminationReason::Interrupted),
        "UserInterrupt should terminate in interactive mode"
    );
    assert_eq!(
        convert_termination_type(termination_type, false),
        Some(TerminationReason::Interrupted),
        "UserInterrupt should terminate in autonomous mode"
    );
}

#[test]
fn test_force_kill_always_terminates() {
    // Given: ForceKill termination in any mode
    let termination_type = ralph_adapters::TerminationType::ForceKill;

    // When/Then: should return Interrupted regardless of mode
    assert_eq!(
        convert_termination_type(termination_type.clone(), true),
        Some(TerminationReason::Interrupted),
        "ForceKill should terminate in interactive mode"
    );
    assert_eq!(
        convert_termination_type(termination_type, false),
        Some(TerminationReason::Interrupted),
        "ForceKill should terminate in autonomous mode"
    );
}

#[test]
fn test_detect_solo_output_completion_requires_hatless_mode() {
    let registry = HatRegistry::new();
    assert!(detect_solo_output_completion(
        &registry,
        "done\nLOOP_COMPLETE\n",
        "LOOP_COMPLETE"
    ));

    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = HatRegistry::from_config(&config);
    assert!(
        !detect_solo_output_completion(&registry, "done\nLOOP_COMPLETE\n", "LOOP_COMPLETE"),
        "text completion should not terminate multi-hat workflows"
    );
}

#[test]
fn test_detect_solo_output_completion_requires_final_non_empty_line() {
    let registry = HatRegistry::new();
    assert!(!detect_solo_output_completion(
        &registry,
        "LOOP_COMPLETE\nMore text after\n",
        "LOOP_COMPLETE"
    ));
    assert!(!detect_solo_output_completion(
        &registry,
        "I think LOOP_COMPLETE but not really",
        "LOOP_COMPLETE"
    ));
}

#[test]
fn test_normalize_cli_output_for_parsing_extracts_claude_text_blocks() {
    let raw = concat!(
        "{\"type\":\"system\",\"session_id\":\"abc\",\"model\":\"claude-opus-4-6\",\"tools\":[]}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"First line\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"Bash\",\"input\":{\"command\":\"pytest\"}}]}}\n",
        "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"tool_1\",\"content\":\"ok\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"LOOP_COMPLETE\"}]}}\n",
        "{\"type\":\"result\",\"duration_ms\":1,\"total_cost_usd\":0.0,\"num_turns\":1,\"is_error\":false}\n"
    );

    assert_eq!(
        normalize_cli_output_for_parsing(BackendOutputFormat::StreamJson, raw),
        "First line\nLOOP_COMPLETE\n"
    );
}

#[test]
fn test_normalize_cli_output_for_parsing_extracts_pi_text_deltas() {
    let raw = concat!(
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"hello \"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_delta\",\"contentIndex\":0,\"delta\":\"hidden\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"LOOP_COMPLETE\"}}\n",
        "{\"type\":\"turn_end\",\"message\":{\"usage\":{\"input\":1,\"output\":1,\"cache_read\":0,\"cache_write\":0,\"cost\":{\"input\":0.0,\"output\":0.0,\"total\":0.0}}}}\n"
    );

    assert_eq!(
        normalize_cli_output_for_parsing(BackendOutputFormat::PiStreamJson, raw),
        "hello LOOP_COMPLETE"
    );
}

#[test]
fn test_normalize_cli_output_for_parsing_extracts_copilot_stream_text() {
    let raw = concat!(
        "{\"type\":\"assistant.turn_start\",\"data\":{\"turnId\":\"0\"}}\n",
        "{\"type\":\"assistant.message\",\"data\":{\"content\":\"First line\"}}\n",
        "{\"type\":\"assistant.message\",\"data\":{\"content\":\"LOOP_COMPLETE\"}}\n",
        "{\"type\":\"result\",\"exitCode\":0}\n"
    );

    assert_eq!(
        normalize_cli_output_for_parsing(BackendOutputFormat::CopilotStreamJson, raw),
        "First line\nLOOP_COMPLETE\n"
    );
}

#[test]
fn test_wave_worker_execution_mode_supports_all_backend_formats() {
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::Text),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::StreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::PiStreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::CopilotStreamJson),
        WaveWorkerExecutionMode::Pty
    );
    assert_eq!(
        wave_worker_execution_mode(BackendOutputFormat::Acp),
        WaveWorkerExecutionMode::Acp
    );
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_matches_supported_named_backend_roster() {
    for (name, expected_output_format, expected_mode, marker_id) in [
        (
            "claude",
            BackendOutputFormat::StreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:claude",
        ),
        (
            "pi",
            BackendOutputFormat::PiStreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:pi",
        ),
        (
            "kiro-acp",
            BackendOutputFormat::Acp,
            WaveWorkerExecutionMode::Acp,
            "execution-mode:named:kiro-acp",
        ),
        (
            "kiro",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:kiro",
        ),
        (
            "gemini",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:gemini",
        ),
        (
            "codex",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:codex",
        ),
        (
            "amp",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:amp",
        ),
        (
            "copilot",
            BackendOutputFormat::CopilotStreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:copilot",
        ),
        (
            "opencode",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:opencode",
        ),
        (
            "roo",
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:named:roo",
        ),
    ] {
        let backend = CliBackend::from_name(name).expect("supported named backend");
        assert_eq!(
            backend.output_format, expected_output_format,
            "unexpected output format for {name}"
        );
        assert_eq!(
            wave_worker_execution_mode(backend.output_format),
            expected_mode,
            "unexpected wave worker execution mode for {name}"
        );
        emit_wave_validation_marker(marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_matches_supported_hat_backend_families() {
    for (hat_backend, expected_output_format, expected_mode, marker_id) in [
        (
            ralph_core::HatBackend::Named("claude".to_string()),
            BackendOutputFormat::StreamJson,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:named-claude",
        ),
        (
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "opencode".to_string(),
                args: vec!["--from-hat-backend".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:named-with-args",
        ),
        (
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:kiro-agent",
        ),
        (
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--unused-extra".to_string()],
            },
            BackendOutputFormat::Acp,
            WaveWorkerExecutionMode::Acp,
            "execution-mode:hat:kiro-acp-agent",
        ),
        (
            ralph_core::HatBackend::Custom {
                command: "/tmp/custom-wave-worker".to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            BackendOutputFormat::Text,
            WaveWorkerExecutionMode::Pty,
            "execution-mode:hat:custom",
        ),
    ] {
        let backend =
            CliBackend::from_hat_backend(&hat_backend).expect("supported hat backend");
        assert_eq!(
            backend.output_format, expected_output_format,
            "unexpected output format for {hat_backend:?}"
        );
        assert_eq!(
            wave_worker_execution_mode(backend.output_format),
            expected_mode,
            "unexpected wave worker execution mode for {hat_backend:?}"
        );
        emit_wave_validation_marker(marker_id, &["backend"]);
    }
}

#[test]
fn test_extract_readable_delta_handles_pi_stream_events() {
    let text_delta = extract_readable_delta(
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"Hello from Pi\"}}",
        BackendOutputFormat::PiStreamJson,
    );
    assert_eq!(text_delta.as_deref(), Some("Hello from Pi"));

    let tool_delta = extract_readable_delta(
        "{\"type\":\"tool_execution_start\",\"toolCallId\":\"toolu_1\",\"toolName\":\"bash\",\"args\":{\"command\":\"echo hi\"}}",
        BackendOutputFormat::PiStreamJson,
    )
    .expect("pi tool start delta");
    assert!(tool_delta.contains("⚙ bash"));
    assert!(tool_delta.contains("echo hi"));

    let result_delta = extract_readable_delta(
        "{\"type\":\"tool_execution_end\",\"toolCallId\":\"toolu_1\",\"toolName\":\"bash\",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\\n\"}]},\"isError\":false}",
        BackendOutputFormat::PiStreamJson,
    );
    assert_eq!(result_delta.as_deref(), Some("→ hi\n\n"));
}

#[cfg(unix)]
fn make_test_wave(publishes: Vec<String>) -> ralph_core::DetectedWave {
    make_test_wave_with_timeout(publishes, 30)
}

#[cfg(unix)]
fn make_test_wave_with_timeout(
    publishes: Vec<String>,
    timeout_secs: u32,
) -> ralph_core::DetectedWave {
    make_test_wave_with_timeout_and_payload(
        publishes,
        timeout_secs,
        "ROLE: Validate this backend".to_string(),
    )
}

#[cfg(unix)]
fn make_test_wave_with_timeout_and_payload(
    publishes: Vec<String>,
    timeout_secs: u32,
    payload: String,
) -> ralph_core::DetectedWave {
    let event = ralph_core::Event {
        topic: "review.perspective".to_string(),
        payload: Some(payload),
        ts: "2026-01-01T00:00:00Z".to_string(),
        wave_id: Some("w-test".to_string()),
        wave_index: Some(0),
        wave_total: Some(1),
    };

    ralph_core::DetectedWave {
        wave_id: "w-test".to_string(),
        target_hat: "reviewer".into(),
        hat_config: ralph_core::HatConfig {
            name: "Reviewer".to_string(),
            description: Some("Wave worker test".to_string()),
            triggers: vec!["review.perspective".to_string()],
            publishes,
            instructions: "Emit review.done when finished.".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            disallowed_tools: vec![],
            timeout: Some(timeout_secs),
            concurrency: 1,
            aggregate: None,
            scratchpad: None,
        },
        events: vec![event],
        total: 1,
    }
}

#[cfg(unix)]
async fn run_wave_for_backend(
    output_format: BackendOutputFormat,
    body: &str,
) -> ralph_core::CompletedWave {
    run_wave_for_backend_with_timeout(output_format, body, 30).await
}

#[cfg(unix)]
async fn run_wave_for_backend_with_timeout(
    output_format: BackendOutputFormat,
    body: &str,
    timeout_secs: u32,
) -> ralph_core::CompletedWave {
    run_wave_for_backend_with_test_env(output_format, body, timeout_secs, vec![]).await
}

#[cfg(unix)]
async fn run_wave_for_backend_with_test_env(
    output_format: BackendOutputFormat,
    body: &str,
    timeout_secs: u32,
    env_vars: Vec<(&str, &str)>,
) -> ralph_core::CompletedWave {
    // Serialize with test_support::CwdGuard tests — see note on
    // run_wave_for_named_backend_with_capture_and_task_payload.
    let _cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let worker_path = write_fake_executable(&bin_dir, "wave-worker", body);

    let backend = CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format,
        env_vars: env_vars
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    };

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave_with_timeout(vec!["review.done".to_string()], timeout_secs);
    execute_wave(&wave, &backend, &events_file, false, false, None, None)
        .await
        .expect("wave execution")
}

#[cfg(unix)]
async fn run_wave_for_named_backend(name: &str, body: &str) -> ralph_core::CompletedWave {
    // Serialize with test_support::CwdGuard tests — see note on
    // run_wave_for_named_backend_with_capture_and_task_payload.
    let _cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");

    let mut backend = CliBackend::from_name(name).expect("named backend");
    let executable_name = Path::new(&backend.command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(backend.command.as_str())
        .to_string();
    write_fake_executable(&bin_dir, &executable_name, body);

    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_value = if existing_path.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{}", bin_dir.display(), existing_path)
    };
    backend.env_vars.push(("PATH".to_string(), path_value));

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave(vec!["review.done".to_string()]);
    execute_wave(&wave, &backend, &events_file, false, false, None, None)
        .await
        .expect("wave execution")
}

#[cfg(unix)]
#[derive(Debug, serde::Deserialize)]
struct CapturedWaveInvocation {
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    prompt: String,
}

#[cfg(unix)]
async fn run_wave_for_named_backend_with_capture(
    name: &str,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    run_wave_for_named_backend_with_capture_and_task_payload(
        name,
        payload,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_named_backend_with_capture_and_task_payload(
    name: &str,
    payload: &str,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    // execute_wave snapshots std::env::current_dir() for the PTY child's
    // cwd. Serialize with test_support::CwdGuard tests (which mutate
    // process cwd) so the snapshot can not see a deleted tempdir and
    // PTY spawn does not race-fail with "No such file or directory".
    let _cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");

    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let mut backend = CliBackend::from_name(name).expect("named backend");
    let executable_name = Path::new(&backend.command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(backend.command.as_str())
        .to_string();
    write_fake_executable(
        &bin_dir,
        &executable_name,
        &invocation_capture_backend_body(payload),
    );

    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path_value = if existing_path.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{}", bin_dir.display(), existing_path)
    };
    backend.env_vars.push(("PATH".to_string(), path_value));

    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    let completed = execute_wave(&wave, &backend, &events_file, false, false, None, None)
        .await
        .expect("wave execution");
    let captured: CapturedWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured invocation"),
    )
    .expect("parse captured invocation");
    (completed, captured)
}

#[cfg(unix)]
fn missing_global_wave_backend() -> CliBackend {
    let mut backend = CliBackend {
        command: "/definitely/missing-wave-worker".to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    if let Some(bin_dir) = FAKE_PATH_BACKEND_BIN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let path_value = if existing_path.is_empty() {
            bin_dir.display().to_string()
        } else {
            format!("{}:{}", bin_dir.display(), existing_path)
        };
        backend.env_vars.push(("PATH".to_string(), path_value));
    }

    backend
}

#[cfg(unix)]
async fn run_wave_for_hat_backend(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    global_backend: CliBackend,
) -> ralph_core::CompletedWave {
    // Serialize with test_support::CwdGuard tests — see note on
    // run_wave_for_named_backend_with_capture_and_task_payload. The cwd
    // lock is reentrant, so callers that already hold it via
    // install_fake_path_backends are not deadlocked.
    let _cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave(vec!["review.done".to_string()]);
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    execute_wave(
        &wave,
        &global_backend,
        &events_file,
        false,
        false,
        None,
        None,
    )
    .await
    .expect("wave execution")
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_capture(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    run_wave_for_hat_backend_with_capture_and_task_payload(
        hat_backend,
        backend_args,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_capture_and_task_payload(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedWaveInvocation) {
    // Serialize with test_support::CwdGuard tests — see note on
    // run_wave_for_named_backend_with_capture_and_task_payload. Reentrant
    // via a thread-local depth counter, so callers that already hold the
    // cwd lock via install_fake_path_backends are not deadlocked.
    let _cwd_lock = crate::test_support::cwd_serial_guard();
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured invocation"),
    )
    .expect("parse captured invocation");
    (completed, captured)
}

#[cfg(unix)]
#[derive(Debug, serde::Deserialize)]
struct CapturedAcpWaveInvocation {
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    prompt: String,
}

#[cfg(unix)]
async fn run_wave_for_named_acp_backend_with_capture(
    backend_args: Option<Vec<String>>,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", payload)],
    )]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave(vec!["review.done".to_string()]);
    wave.hat_config.backend_args = backend_args;
    let backend = CliBackend::from_name("kiro-acp").expect("named ACP backend");

    let completed = execute_wave(&wave, &backend, &events_file, false, false, None, None)
        .await
        .expect("wave execution");
    let captured: CapturedAcpWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured ACP invocation"),
    )
    .expect("parse captured ACP invocation");
    (completed, captured)
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_acp_capture(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    run_wave_for_hat_backend_with_acp_capture_and_task_payload(
        hat_backend,
        backend_args,
        payload,
        "ROLE: Validate this backend",
    )
    .await
}

#[cfg(unix)]
async fn run_wave_for_hat_backend_with_acp_capture_and_task_payload(
    hat_backend: ralph_core::HatBackend,
    backend_args: Option<Vec<String>>,
    payload: &str,
    task_payload: &str,
) -> (ralph_core::CompletedWave, CapturedAcpWaveInvocation) {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", payload)],
    )]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_capture_path = temp_dir.path().join("wave-w-test-0.jsonl.capture");
    let events_file = temp_dir.path().join("events.jsonl");
    let mut wave = make_test_wave_with_timeout_and_payload(
        vec!["review.done".to_string()],
        30,
        task_payload.to_string(),
    );
    wave.hat_config.backend = Some(hat_backend);
    wave.hat_config.backend_args = backend_args;

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
    )
    .await
    .expect("wave execution");
    let captured: CapturedAcpWaveInvocation = serde_json::from_str(
        &std::fs::read_to_string(&worker_capture_path).expect("read captured ACP invocation"),
    )
    .expect("parse captured ACP invocation");
    (completed, captured)
}

#[cfg(unix)]
fn make_worker_event(topic: &str, payload: &str) -> ralph_core::Event {
    ralph_core::Event {
        topic: topic.to_string(),
        payload: Some(payload.to_string()),
        ts: "2026-01-01T00:00:00Z".to_string(),
        wave_id: None,
        wave_index: None,
        wave_total: None,
    }
}

#[cfg(unix)]
fn text_backend_body(payload: &str) -> String {
    format!(
        r#"printf 'plain text from worker\n'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z"}}
EOF"#,
    )
}

#[cfg(unix)]
fn claude_backend_body(payload: &str) -> String {
    format!(
        r#"printf '%s\n' \
'{{"type":"assistant","message":{{"content":[{{"type":"text","text":"hello from named claude"}}]}}}}' \
'{{"type":"result","duration_ms":1,"total_cost_usd":0.0,"num_turns":1,"is_error":false}}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z"}}
EOF"#,
    )
}

#[cfg(unix)]
fn pi_backend_body(payload: &str) -> String {
    format!(
        r#"printf '%s\n' \
'{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":"hello from named pi"}}}}' \
'{{"type":"tool_execution_start","toolCallId":"toolu_1","toolName":"bash","args":{{"command":"echo hi"}}}}' \
'{{"type":"tool_execution_end","toolCallId":"toolu_1","toolName":"bash","result":{{"content":[{{"type":"text","text":"hi\n"}}]}},"isError":false}}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z"}}
EOF"#,
    )
}

#[cfg(unix)]
fn invocation_capture_backend_body(payload: &str) -> String {
    format!(
        r#"python3 -c '
import json
import os
import pathlib
import select
import sys

args = sys.argv[1:]
prompt = ""
if "--prompt-file" in args:
    prompt_flag_index = args.index("--prompt-file")
    prompt = pathlib.Path(args[prompt_flag_index + 1]).read_text()
elif "--print" in args:
    chunks = []
    fd = sys.stdin.fileno()
    while True:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            break
        chunk = os.read(fd, 4096)
        if not chunk:
            break
        chunks.append(chunk)
    prompt = b"".join(chunks).decode()
elif args:
    prompt = args[-1]
    temp_file_prefix = "Please read and execute the task in "
    if prompt.startswith(temp_file_prefix):
        prompt = pathlib.Path(prompt[len(temp_file_prefix):]).read_text()

pathlib.Path(os.environ["RALPH_EVENTS_FILE"] + ".capture").write_text(json.dumps({{
    "args": args,
    "env": {{
        "RALPH_WAVE_WORKER": os.environ.get("RALPH_WAVE_WORKER", ""),
        "RALPH_WAVE_ID": os.environ.get("RALPH_WAVE_ID", ""),
        "RALPH_WAVE_INDEX": os.environ.get("RALPH_WAVE_INDEX", ""),
        "RALPH_EVENTS_FILE": os.environ.get("RALPH_EVENTS_FILE", ""),
        "TERM": os.environ.get("TERM", ""),
        "NO_COLOR": os.environ.get("NO_COLOR", ""),
    }},
    "prompt": prompt,
}}))
' "$@"
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{{"topic":"review.done","payload":"{payload}","ts":"2026-01-01T00:00:00Z"}}
EOF"#,
    )
}

#[cfg(unix)]
enum PromptDeliveryExpectation {
    Flag(&'static str),
    Positional,
    Stdin,
    TempFileFlag(&'static str),
    TempFilePositional,
    PromptFile,
}

#[cfg(unix)]
fn assert_captured_wave_prompt(prompt: &str) {
    assert!(
        prompt.contains("# Instructions"),
        "missing instructions: {prompt}"
    );
    assert!(
        prompt.contains("Emit review.done when finished."),
        "missing worker instructions: {prompt}"
    );
    assert!(
        prompt.contains("# Wave Context"),
        "missing wave context: {prompt}"
    );
    assert!(
        prompt.contains("worker **1/1**"),
        "missing worker index: {prompt}"
    );
    assert!(prompt.contains("w-test"), "missing wave id: {prompt}");
    assert!(
        prompt.contains("# Your Task"),
        "missing task section: {prompt}"
    );
    assert!(
        prompt.contains("ROLE: Validate this backend"),
        "missing task payload: {prompt}"
    );
    assert!(
        prompt.contains("ralph emit review.done"),
        "missing publishing guidance: {prompt}"
    );
    assert!(prompt.contains("DO NOT"), "missing constraints: {prompt}");
}

#[cfg(unix)]
fn assert_captured_wave_env(
    env: &std::collections::BTreeMap<String, String>,
    expect_terminal_env: bool,
) {
    assert_eq!(env.get("RALPH_WAVE_WORKER").map(String::as_str), Some("1"));
    assert_eq!(env.get("RALPH_WAVE_ID").map(String::as_str), Some("w-test"));
    assert_eq!(env.get("RALPH_WAVE_INDEX").map(String::as_str), Some("0"));
    if expect_terminal_env {
        assert_eq!(env.get("TERM").map(String::as_str), Some("dumb"));
        assert_eq!(env.get("NO_COLOR").map(String::as_str), Some("1"));
    }
    assert!(
        env.get("RALPH_EVENTS_FILE")
            .is_some_and(|path| path.ends_with("wave-w-test-0.jsonl")),
        "missing wave events file env: {:?}",
        env
    );
}

#[cfg(unix)]
fn assert_temp_file_prompt_instruction(instruction: &str, captured_prompt: &str) {
    let prefix = "Please read and execute the task in ";
    assert!(
        instruction.starts_with(prefix),
        "expected temp-file handoff instruction, got {instruction:?}"
    );
    let path = &instruction[prefix.len()..];
    assert!(
        !path.is_empty(),
        "missing temp-file path in {instruction:?}"
    );
    assert!(
        path.starts_with('/'),
        "expected absolute temp-file path, got {instruction:?}"
    );
    assert_ne!(
        captured_prompt, instruction,
        "captured prompt should contain temp-file contents, not the handoff instruction"
    );
}

#[cfg(unix)]
fn assert_named_backend_invocation_contract(
    captured: &CapturedWaveInvocation,
    expected_prefix: &[&str],
    prompt_delivery: PromptDeliveryExpectation,
) {
    let args = captured.args.iter().map(String::as_str).collect::<Vec<_>>();

    assert_eq!(
        &args[..expected_prefix.len()],
        expected_prefix,
        "unexpected fixed args: {:?}",
        captured.args
    );

    match prompt_delivery {
        PromptDeliveryExpectation::Flag(flag) => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(args[expected_prefix.len()], flag, "missing prompt flag");
            assert_eq!(
                captured.prompt,
                args[expected_prefix.len() + 1],
                "prompt arg should match captured prompt"
            );
        }
        PromptDeliveryExpectation::Positional => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 1,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(
                captured.prompt,
                args[expected_prefix.len()],
                "positional prompt should match captured prompt"
            );
        }
        PromptDeliveryExpectation::Stdin => {
            assert_eq!(
                args.len(),
                expected_prefix.len(),
                "unexpected arg count: {:?}",
                captured.args
            );
            assert!(
                !captured.prompt.is_empty(),
                "stdin-delivered prompt should be captured"
            );
        }
        PromptDeliveryExpectation::TempFileFlag(flag) => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(args[expected_prefix.len()], flag, "missing prompt flag");
            assert_temp_file_prompt_instruction(
                args[expected_prefix.len() + 1],
                &captured.prompt,
            );
        }
        PromptDeliveryExpectation::TempFilePositional => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 1,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_temp_file_prompt_instruction(args[expected_prefix.len()], &captured.prompt);
        }
        PromptDeliveryExpectation::PromptFile => {
            assert_eq!(
                args.len(),
                expected_prefix.len() + 2,
                "unexpected arg count: {:?}",
                captured.args
            );
            assert_eq!(
                args[expected_prefix.len()],
                "--prompt-file",
                "missing roo prompt file flag"
            );
            assert!(
                !args[expected_prefix.len() + 1].is_empty(),
                "missing roo prompt file path"
            );
            assert!(
                args[expected_prefix.len() + 1].contains("tmp")
                    || args[expected_prefix.len() + 1].contains("Temp"),
                "expected temp prompt file path, got {:?}",
                captured.args
            );
        }
    }

    assert_captured_wave_prompt(&captured.prompt);
    assert_captured_wave_env(&captured.env, true);
}

#[cfg(unix)]
fn assert_acp_invocation_contract(
    captured: &CapturedAcpWaveInvocation,
    expected_args: &[&str],
) {
    assert_eq!(captured.command, "kiro-cli");
    assert_eq!(
        captured.args.iter().map(String::as_str).collect::<Vec<_>>(),
        expected_args,
        "unexpected ACP args: {:?}",
        captured.args
    );
    assert_captured_wave_prompt(&captured.prompt);
    assert_captured_wave_env(&captured.env, false);
}

#[cfg(unix)]
fn body_with_post_event_sleep(body: String) -> String {
    format!("{body}\npython3 - <<'PY'\nimport time\ntime.sleep(2)\nPY")
}

#[cfg(unix)]
macro_rules! named_text_wave_backend_test {
    ($test_name:ident, $backend_name:literal, $payload:literal) => {
        #[tokio::test]
        async fn $test_name() {
            let completed =
                run_wave_for_named_backend($backend_name, &text_backend_body($payload)).await;
            assert_single_success_marked(
                &completed,
                $payload,
                concat!("named-backend:", $backend_name),
            );
        }
    };
}

#[cfg(unix)]
fn assert_single_success(completed: &ralph_core::CompletedWave, expected_payload: &str) {
    assert!(
        completed.failures.is_empty(),
        "unexpected failures: {:?}",
        completed.failures
    );
    assert_eq!(
        completed.results.len(),
        1,
        "unexpected results: {:?}",
        completed.results
    );
    assert_eq!(completed.results[0].events.len(), 1);
    assert_eq!(completed.results[0].events[0].topic.as_str(), "review.done");
    assert_eq!(completed.results[0].events[0].payload, expected_payload);
}

#[cfg(unix)]
fn emit_wave_validation_marker(id: &str, tags: &[&str]) {
    println!("WAVE_VALIDATION_MARKER id={id} tags={}", tags.join(","));
}

#[cfg(unix)]
fn assert_single_success_marked(
    completed: &ralph_core::CompletedWave,
    expected_payload: &str,
    marker_id: &str,
) {
    assert_single_success(completed, expected_payload);
    emit_wave_validation_marker(marker_id, &["backend"]);
}

#[cfg(unix)]
fn assert_single_failure_with_synthetic_events_marked(
    completed: &ralph_core::CompletedWave,
    expected_error: &str,
    marker_id: &str,
) {
    assert!(
        completed.results.is_empty(),
        "unexpected results: {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        1,
        "unexpected failures: {completed:?}"
    );
    assert!(
        completed.failures[0].error.contains(expected_error),
        "unexpected failure: {:?}",
        completed.failures[0]
    );

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    merge_wave_results_to_events_file(
        completed,
        &merged_events_path,
        &["review.done".to_string(), "review.audit".to_string()],
    )
    .expect("merge wave failure results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 3, "unexpected merged records: {records:?}");
    assert!(records.iter().any(|record| {
        record["topic"] == "wave.worker.failed"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains(expected_error))
            && record["wave_index"] == 0
    }));
    for topic in ["review.done", "review.audit"] {
        assert!(records.iter().any(|record| {
            record["topic"] == topic
                && record["payload"].as_str().is_some_and(|payload| {
                    payload.contains("## Worker 0 (FAILED)") && payload.contains(expected_error)
                })
        }));
    }

    emit_wave_validation_marker(marker_id, &["backend", "error", "synthetic"]);
}

#[cfg(unix)]
fn assert_partial_timeout_events_visible_marked(
    completed: &ralph_core::CompletedWave,
    expected_payload: &str,
    marker_id: &str,
) {
    assert!(
        completed.failures.is_empty(),
        "unexpected failures: {completed:?}"
    );
    assert_eq!(
        completed.results.len(),
        1,
        "unexpected results: {completed:?}"
    );
    assert_eq!(
        completed.results[0].events.len(),
        1,
        "unexpected result events: {completed:?}"
    );
    assert_eq!(completed.results[0].events[0].topic.as_str(), "review.done");
    assert_eq!(completed.results[0].events[0].payload, expected_payload);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    merge_wave_results_to_events_file(
        completed,
        &merged_events_path,
        &["review.done".to_string()],
    )
    .expect("merge partial-timeout results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 1, "unexpected merged records: {records:?}");
    assert_eq!(records[0]["topic"], "review.done");
    assert_eq!(records[0]["payload"], expected_payload);
    assert!(
        records
            .iter()
            .all(|record| record["topic"] != "wave.worker.failed"),
        "partial timeout should not synthesize worker failures: {records:?}"
    );

    emit_wave_validation_marker(marker_id, &["backend", "error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_text_backend() {
    let completed = run_wave_for_backend(
        BackendOutputFormat::Text,
        r#"printf 'plain text from worker\n'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"text backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )
    .await;

    assert_single_success_marked(&completed, "text backend ok", "output-format:text");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_claude_stream_json_backend() {
    let completed = run_wave_for_backend(
        BackendOutputFormat::StreamJson,
        r#"printf '%s\n' \
'{"type":"assistant","message":{"content":[{"type":"text","text":"hello from claude stream"}]}}' \
'{"type":"result","duration_ms":1,"total_cost_usd":0.0,"num_turns":1,"is_error":false}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"claude stream ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )
    .await;

    assert_single_success_marked(
        &completed,
        "claude stream ok",
        "output-format:claude-stream-json",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_pi_stream_json_backend() {
    let completed = run_wave_for_backend(
        BackendOutputFormat::PiStreamJson,
        r#"printf '%s\n' \
'{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello from pi"}}' \
'{"type":"tool_execution_start","toolCallId":"toolu_1","toolName":"bash","args":{"command":"echo hi"}}' \
'{"type":"tool_execution_end","toolCallId":"toolu_1","toolName":"bash","result":{"content":[{"type":"text","text":"hi\n"}]},"isError":false}' \
'{"type":"turn_end","message":{"usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"cost":{"total":0.0}}}}'
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"pi stream ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )
    .await;

    assert_single_success_marked(&completed, "pi stream ok", "output-format:pi-stream-json");
}

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_kiro_backend,
    "kiro",
    "kiro backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_gemini_backend,
    "gemini",
    "gemini backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_codex_backend,
    "codex",
    "codex backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_amp_backend,
    "amp",
    "amp backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_copilot_backend,
    "copilot",
    "copilot backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_opencode_backend,
    "opencode",
    "opencode backend ok"
);

#[cfg(unix)]
named_text_wave_backend_test!(
    test_execute_wave_supports_named_roo_backend,
    "roo",
    "roo backend ok"
);

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_claude_backend() {
    let completed =
        run_wave_for_named_backend("claude", &claude_backend_body("claude backend ok")).await;
    assert_single_success_marked(&completed, "claude backend ok", "named-backend:claude");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_pi_backend() {
    let completed = run_wave_for_named_backend("pi", &pi_backend_body("pi backend ok")).await;
    assert_single_success_marked(&completed, "pi backend ok", "named-backend:pi");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_named_kiro_acp_backend() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event("review.done", "kiro-acp backend ok")],
    )]);

    let completed = run_wave_for_named_backend("kiro-acp", &text_backend_body("unused")).await;
    assert_single_success_marked(&completed, "kiro-acp backend ok", "named-backend:kiro-acp");
}

#[cfg(unix)]
fn large_wave_task_payload() -> String {
    format!(
        "ROLE: Validate this backend\n{}",
        "large-temp-file-wave-payload ".repeat(320)
    )
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_named_backend_invocation_contracts() {
    struct NamedBackendInvocationCase {
        name: &'static str,
        success_payload: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        marker_id: &'static str,
    }

    for case in [
        NamedBackendInvocationCase {
            name: "claude",
            success_payload: "claude invocation contract ok",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            marker_id: "invocation-contract:named:claude",
        },
        NamedBackendInvocationCase {
            name: "pi",
            success_payload: "pi invocation contract ok",
            expected_prefix: &["-p", "--mode", "json", "--no-session"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:pi",
        },
        NamedBackendInvocationCase {
            name: "kiro",
            success_payload: "kiro invocation contract ok",
            expected_prefix: &["chat", "--no-interactive", "--trust-all-tools"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:kiro",
        },
        NamedBackendInvocationCase {
            name: "gemini",
            success_payload: "gemini invocation contract ok",
            expected_prefix: &["--yolo"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
            marker_id: "invocation-contract:named:gemini",
        },
        NamedBackendInvocationCase {
            name: "codex",
            success_payload: "codex invocation contract ok",
            expected_prefix: &["exec", "--yolo"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:codex",
        },
        NamedBackendInvocationCase {
            name: "amp",
            success_payload: "amp invocation contract ok",
            expected_prefix: &["--dangerously-allow-all"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-x"),
            marker_id: "invocation-contract:named:amp",
        },
        NamedBackendInvocationCase {
            name: "copilot",
            success_payload: "copilot invocation contract ok",
            expected_prefix: &["--allow-all-tools", "--output-format", "json"],
            prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
            marker_id: "invocation-contract:named:copilot",
        },
        NamedBackendInvocationCase {
            name: "opencode",
            success_payload: "opencode invocation contract ok",
            expected_prefix: &["run"],
            prompt_delivery: PromptDeliveryExpectation::Positional,
            marker_id: "invocation-contract:named:opencode",
        },
        NamedBackendInvocationCase {
            name: "roo",
            success_payload: "roo invocation contract ok",
            expected_prefix: &["--print", "--ephemeral"],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            marker_id: "invocation-contract:named:roo",
        },
    ] {
        let (completed, captured) =
            run_wave_for_named_backend_with_capture(case.name, case.success_payload).await;
        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_named_backend_large_prompt_contracts() {
    struct NamedBackendLargePromptCase {
        name: &'static str,
        success_payload: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        marker_id: &'static str,
    }

    let task_payload = large_wave_task_payload();
    assert!(task_payload.len() > 7000, "expected large task payload");

    for case in [
        NamedBackendLargePromptCase {
            name: "claude",
            success_payload: "claude large prompt contract ok",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            marker_id: "large-prompt-contract:named:claude",
        },
        NamedBackendLargePromptCase {
            name: "pi",
            success_payload: "pi large prompt contract ok",
            expected_prefix: &["-p", "--mode", "json", "--no-session"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:pi",
        },
        NamedBackendLargePromptCase {
            name: "kiro",
            success_payload: "kiro large prompt contract ok",
            expected_prefix: &["chat", "--no-interactive", "--trust-all-tools"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:kiro",
        },
        NamedBackendLargePromptCase {
            name: "gemini",
            success_payload: "gemini large prompt contract ok",
            expected_prefix: &["--yolo"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            marker_id: "large-prompt-contract:named:gemini",
        },
        NamedBackendLargePromptCase {
            name: "codex",
            success_payload: "codex large prompt contract ok",
            expected_prefix: &["exec", "--yolo"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:codex",
        },
        NamedBackendLargePromptCase {
            name: "amp",
            success_payload: "amp large prompt contract ok",
            expected_prefix: &["--dangerously-allow-all"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            marker_id: "large-prompt-contract:named:amp",
        },
        NamedBackendLargePromptCase {
            name: "copilot",
            success_payload: "copilot large prompt contract ok",
            expected_prefix: &["--allow-all-tools", "--output-format", "json"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            marker_id: "large-prompt-contract:named:copilot",
        },
        NamedBackendLargePromptCase {
            name: "opencode",
            success_payload: "opencode large prompt contract ok",
            expected_prefix: &["run"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            marker_id: "large-prompt-contract:named:opencode",
        },
    ] {
        let (completed, captured) = run_wave_for_named_backend_with_capture_and_task_payload(
            case.name,
            case.success_payload,
            &task_payload,
        )
        .await;
        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for {}",
            case.name
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_hat_backend_invocation_contracts() {
    {
        let body = invocation_capture_backend_body("hat named invocation contract ok");
        let _fake = install_fake_path_backends(&[("gemini", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::Named("gemini".to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat named invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--yolo", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Flag("-p"),
        );
        emit_wave_validation_marker("invocation-contract:hat:named", &["backend"]);
    }

    {
        let body =
            invocation_capture_backend_body("hat named-with-args invocation contract ok");
        let _fake = install_fake_path_backends(&[("opencode", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "opencode".to_string(),
                args: vec!["--from-hat-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat named-with-args invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["run", "--from-hat-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:named-with-args", &["backend"]);
    }

    {
        struct HatNamedWithArgsInvocationCase {
            backend_type: &'static str,
            executable_name: &'static str,
            extra_args: &'static [&'static str],
            expected_prefix: &'static [&'static str],
            prompt_delivery: PromptDeliveryExpectation,
            success_payload: &'static str,
            marker_id: &'static str,
        }

        for case in [
            HatNamedWithArgsInvocationCase {
                backend_type: "claude",
                executable_name: "claude",
                extra_args: &["--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "--dangerously-skip-permissions",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--setting-sources",
                    "project,local",
                    "--print",
                    "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Stdin,
                success_payload: "hat claude named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:claude",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "pi",
                executable_name: "pi",
                extra_args: &["--provider", "anthropic", "--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "-p",
                    "--mode",
                    "json",
                    "--no-session",
                    "--provider",
                    "anthropic",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat pi named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:pi",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "kiro",
                executable_name: "kiro-cli",
                extra_args: &["--profile", "reviewer"],
                expected_prefix: &[
                    "chat",
                    "--no-interactive",
                    "--trust-all-tools",
                    "--profile",
                    "reviewer",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat kiro named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:kiro",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "gemini",
                executable_name: "gemini",
                extra_args: &["--model", "gemini-2.5-pro"],
                expected_prefix: &["--yolo", "--model", "gemini-2.5-pro", "--hat-runtime-arg"],
                prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
                success_payload: "hat gemini named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:gemini",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "codex",
                executable_name: "codex",
                extra_args: &["--dangerously-bypass-approvals-and-sandbox"],
                expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
                prompt_delivery: PromptDeliveryExpectation::Positional,
                success_payload: "hat codex named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:codex",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "amp",
                executable_name: "amp",
                extra_args: &["--model", "gpt-5"],
                expected_prefix: &[
                    "--dangerously-allow-all",
                    "--model",
                    "gpt-5",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Flag("-x"),
                success_payload: "hat amp named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:amp",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "copilot",
                executable_name: "copilot",
                extra_args: &["--model", "gpt-5"],
                expected_prefix: &[
                    "--allow-all-tools",
                    "--output-format",
                    "json",
                    "--model",
                    "gpt-5",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::Flag("-p"),
                success_payload: "hat copilot named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:copilot",
            },
            HatNamedWithArgsInvocationCase {
                backend_type: "roo",
                executable_name: "roo",
                extra_args: &["--model", "claude-sonnet-4"],
                expected_prefix: &[
                    "--print",
                    "--ephemeral",
                    "--model",
                    "claude-sonnet-4",
                    "--hat-runtime-arg",
                ],
                prompt_delivery: PromptDeliveryExpectation::PromptFile,
                success_payload: "hat roo named-with-args invocation contract ok",
                marker_id: "invocation-contract:hat:named-with-args:roo",
            },
        ] {
            let body = invocation_capture_backend_body(case.success_payload);
            let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
            let (completed, captured) = run_wave_for_hat_backend_with_capture(
                ralph_core::HatBackend::NamedWithArgs {
                    backend_type: case.backend_type.to_string(),
                    args: case
                        .extra_args
                        .iter()
                        .map(|arg| (*arg).to_string())
                        .collect(),
                },
                Some(vec!["--hat-runtime-arg".to_string()]),
            )
            .await;

            assert_single_success(&completed, case.success_payload);
            assert_named_backend_invocation_contract(
                &captured,
                case.expected_prefix,
                case.prompt_delivery,
            );
            emit_wave_validation_marker(case.marker_id, &["backend"]);
        }
    }

    {
        let body = invocation_capture_backend_body("hat kiro agent invocation contract ok");
        let _fake = install_fake_path_backends(&[("kiro-cli", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat kiro agent invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--agent",
                "reviewer-agent",
                "--kiro-extra",
                "--hat-runtime-arg",
            ],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:kiro-agent", &["backend"]);
    }

    {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let body = invocation_capture_backend_body("hat custom invocation contract ok");
        let worker_path = write_fake_executable(temp_dir.path(), "custom-wave-worker", &body);
        let (completed, captured) = run_wave_for_hat_backend_with_capture(
            ralph_core::HatBackend::Custom {
                command: worker_path.display().to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
        )
        .await;

        assert_single_success(&completed, "hat custom invocation contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--from-custom-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::Positional,
        );
        emit_wave_validation_marker("invocation-contract:hat:custom", &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_hat_backend_large_prompt_contracts() {
    struct HatNamedLargePromptCase {
        backend_type: &'static str,
        executable_name: &'static str,
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        success_payload: &'static str,
        marker_id: &'static str,
    }

    struct HatNamedWithArgsLargePromptCase {
        backend_type: &'static str,
        executable_name: &'static str,
        extra_args: &'static [&'static str],
        expected_prefix: &'static [&'static str],
        prompt_delivery: PromptDeliveryExpectation,
        success_payload: &'static str,
        marker_id: &'static str,
    }

    let task_payload = large_wave_task_payload();
    assert!(task_payload.len() > 7000, "expected large task payload");

    for case in [
        HatNamedLargePromptCase {
            backend_type: "claude",
            executable_name: "claude",
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            success_payload: "hat claude named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:claude",
        },
        HatNamedLargePromptCase {
            backend_type: "pi",
            executable_name: "pi",
            expected_prefix: &["-p", "--mode", "json", "--no-session", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat pi named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:pi",
        },
        HatNamedLargePromptCase {
            backend_type: "kiro",
            executable_name: "kiro-cli",
            expected_prefix: &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat kiro named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:kiro",
        },
        HatNamedLargePromptCase {
            backend_type: "gemini",
            executable_name: "gemini",
            expected_prefix: &["--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat gemini named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:gemini",
        },
        HatNamedLargePromptCase {
            backend_type: "codex",
            executable_name: "codex",
            expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat codex named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:codex",
        },
        HatNamedLargePromptCase {
            backend_type: "amp",
            executable_name: "amp",
            expected_prefix: &["--dangerously-allow-all", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            success_payload: "hat amp named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:amp",
        },
        HatNamedLargePromptCase {
            backend_type: "copilot",
            executable_name: "copilot",
            expected_prefix: &[
                "--allow-all-tools",
                "--output-format",
                "json",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat copilot named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:copilot",
        },
        HatNamedLargePromptCase {
            backend_type: "opencode",
            executable_name: "opencode",
            expected_prefix: &["run", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat opencode named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:opencode",
        },
        HatNamedLargePromptCase {
            backend_type: "roo",
            executable_name: "roo",
            expected_prefix: &["--print", "--ephemeral", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            success_payload: "hat roo named large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named:roo",
        },
    ] {
        let body = invocation_capture_backend_body(case.success_payload);
        let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::Named(case.backend_type.to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for {}",
            case.backend_type
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture_and_task_payload(
            ralph_core::HatBackend::Named("kiro-acp".to_string()),
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat kiro-acp named large prompt contract ok",
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro-acp named large prompt contract ok");
        assert_acp_invocation_contract(&captured, &["acp", "--hat-runtime-arg"]);
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for kiro-acp named hat"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:named:kiro-acp", &["backend"]);
    }

    for case in [
        HatNamedWithArgsLargePromptCase {
            backend_type: "claude",
            executable_name: "claude",
            extra_args: &["--model", "claude-sonnet-4"],
            expected_prefix: &[
                "--dangerously-skip-permissions",
                "--verbose",
                "--output-format",
                "stream-json",
                "--setting-sources",
                "project,local",
                "--print",
                "--disallowedTools=TodoWrite,TaskCreate,TaskUpdate,TaskList,TaskGet",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::Stdin,
            success_payload: "hat claude named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:claude",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "pi",
            executable_name: "pi",
            extra_args: &["--provider", "anthropic", "--model", "claude-sonnet-4"],
            expected_prefix: &[
                "-p",
                "--mode",
                "json",
                "--no-session",
                "--provider",
                "anthropic",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat pi named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:pi",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "kiro",
            executable_name: "kiro-cli",
            extra_args: &["--profile", "reviewer"],
            expected_prefix: &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--profile",
                "reviewer",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat kiro named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:kiro",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "gemini",
            executable_name: "gemini",
            extra_args: &["--model", "gemini-2.5-pro"],
            expected_prefix: &["--yolo", "--model", "gemini-2.5-pro", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat gemini named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:gemini",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "codex",
            executable_name: "codex",
            extra_args: &["--dangerously-bypass-approvals-and-sandbox"],
            expected_prefix: &["exec", "--yolo", "--hat-runtime-arg"],
            prompt_delivery: PromptDeliveryExpectation::TempFilePositional,
            success_payload: "hat codex named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:codex",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "amp",
            executable_name: "amp",
            extra_args: &["--model", "gpt-5"],
            expected_prefix: &[
                "--dangerously-allow-all",
                "--model",
                "gpt-5",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-x"),
            success_payload: "hat amp named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:amp",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "copilot",
            executable_name: "copilot",
            extra_args: &["--model", "gpt-5"],
            expected_prefix: &[
                "--allow-all-tools",
                "--output-format",
                "json",
                "--model",
                "gpt-5",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::TempFileFlag("-p"),
            success_payload: "hat copilot named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:copilot",
        },
        HatNamedWithArgsLargePromptCase {
            backend_type: "roo",
            executable_name: "roo",
            extra_args: &["--model", "claude-sonnet-4"],
            expected_prefix: &[
                "--print",
                "--ephemeral",
                "--model",
                "claude-sonnet-4",
                "--hat-runtime-arg",
            ],
            prompt_delivery: PromptDeliveryExpectation::PromptFile,
            success_payload: "hat roo named-with-args large prompt contract ok",
            marker_id: "large-prompt-contract:hat:named-with-args:roo",
        },
    ] {
        let body = invocation_capture_backend_body(case.success_payload);
        let _fake = install_fake_path_backends(&[(case.executable_name, body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: case.backend_type.to_string(),
                args: case
                    .extra_args
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .collect(),
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, case.success_payload);
        assert_named_backend_invocation_contract(
            &captured,
            case.expected_prefix,
            case.prompt_delivery,
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for {}",
            case.backend_type
        );
        emit_wave_validation_marker(case.marker_id, &["backend"]);
    }

    {
        let body = invocation_capture_backend_body("hat kiro agent large prompt contract ok");
        let _fake = install_fake_path_backends(&[("kiro-cli", body.as_str())]);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--kiro-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro agent large prompt contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &[
                "chat",
                "--no-interactive",
                "--trust-all-tools",
                "--agent",
                "reviewer-agent",
                "--kiro-extra",
                "--hat-runtime-arg",
            ],
            PromptDeliveryExpectation::TempFilePositional,
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for kiro agent hat"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:kiro-agent:kiro", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture_and_task_payload(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--ignored-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat kiro-acp agent large prompt contract ok",
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat kiro-acp agent large prompt contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--agent", "reviewer-agent", "--hat-runtime-arg"],
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for kiro-acp agent hat"
        );
        emit_wave_validation_marker(
            "large-prompt-contract:hat:kiro-agent:kiro-acp",
            &["backend"],
        );
    }

    {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let body = invocation_capture_backend_body("hat custom large prompt contract ok");
        let worker_path = write_fake_executable(temp_dir.path(), "custom-wave-worker", &body);
        let (completed, captured) = run_wave_for_hat_backend_with_capture_and_task_payload(
            ralph_core::HatBackend::Custom {
                command: worker_path.display().to_string(),
                args: vec!["--from-custom-backend".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            &task_payload,
        )
        .await;

        assert_single_success(&completed, "hat custom large prompt contract ok");
        assert_named_backend_invocation_contract(
            &captured,
            &["--from-custom-backend", "--hat-runtime-arg"],
            PromptDeliveryExpectation::TempFilePositional,
        );
        assert!(
            captured.prompt.contains(&task_payload),
            "captured prompt should include full large task payload for custom backend"
        );
        emit_wave_validation_marker("large-prompt-contract:hat:custom", &["backend"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_acp_backend_invocation_contracts() {
    {
        let (completed, captured) = run_wave_for_named_acp_backend_with_capture(
            Some(vec!["--hat-runtime-arg".to_string()]),
            "named ACP invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "named ACP invocation contract ok");
        assert_acp_invocation_contract(&captured, &["acp", "--hat-runtime-arg"]);
        emit_wave_validation_marker("invocation-contract:acp:named:kiro-acp", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture(
            ralph_core::HatBackend::KiroAgent {
                backend_type: "kiro-acp".to_string(),
                agent: "reviewer-agent".to_string(),
                args: vec!["--ignored-extra".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat ACP kiro-agent invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "hat ACP kiro-agent invocation contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--agent", "reviewer-agent", "--hat-runtime-arg"],
        );
        emit_wave_validation_marker("invocation-contract:acp:hat:kiro-agent", &["backend"]);
    }

    {
        let (completed, captured) = run_wave_for_hat_backend_with_acp_capture(
            ralph_core::HatBackend::NamedWithArgs {
                backend_type: "kiro-acp".to_string(),
                args: vec!["--model".to_string(), "claude-sonnet-4".to_string()],
            },
            Some(vec!["--hat-runtime-arg".to_string()]),
            "hat ACP named-with-args invocation contract ok",
        )
        .await;

        assert_single_success(&completed, "hat ACP named-with-args invocation contract ok");
        assert_acp_invocation_contract(
            &captured,
            &["acp", "--model", "claude-sonnet-4", "--hat-runtime-arg"],
        );
        emit_wave_validation_marker(
            "invocation-contract:acp:hat:named-with-args",
            &["backend"],
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_surfaces_named_kiro_acp_executor_error_with_synthetic_events() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::error(
        "mock acp executor exploded",
        vec![],
    )]);

    let completed = run_wave_for_named_backend("kiro-acp", &text_backend_body("unused")).await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock acp executor exploded",
        "acp:named-executor-error",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_surfaces_hat_kiro_acp_timeout_without_events_with_synthetic_events()
{
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::timeout(vec![])]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro-acp".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--unused-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "Worker timed out after 30s without emitting events",
        "acp:hat-timeout-without-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_named_with_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "gemini",
        r#"found_hat_arg=0
for arg in "$@"; do
  if [ "$arg" = "--hat-runtime-arg" ]; then
found_hat_arg=1
  fi
done
if [ "$found_hat_arg" -ne 1 ]; then
  echo "missing --hat-runtime-arg: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat named backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Named("gemini".to_string()),
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(&completed, "hat named backend ok", "hat-backend:named");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_named_with_args_and_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "opencode",
        r#"found_hat_backend_arg=0
found_hat_runtime_arg=0
for arg in "$@"; do
  if [ "$arg" = "--from-hat-backend" ]; then
found_hat_backend_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
found_hat_runtime_arg=1
  fi
done
if [ "$found_hat_backend_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat named-with-args backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::NamedWithArgs {
            backend_type: "opencode".to_string(),
            args: vec!["--from-hat-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat named-with-args backend ok",
        "hat-backend:named-with-args",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_kiro_agent_with_backend_args() {
    let _fake = install_fake_path_backends(&[(
        "kiro-cli",
        r#"found_agent=0
found_kiro_arg=0
found_hat_runtime_arg=0
prev=''
for arg in "$@"; do
  if [ "$prev" = "--agent" ] && [ "$arg" = "reviewer-agent" ]; then
found_agent=1
  fi
  if [ "$arg" = "--kiro-extra" ]; then
found_kiro_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
found_hat_runtime_arg=1
  fi
  prev="$arg"
done
if [ "$found_agent" -ne 1 ] || [ "$found_kiro_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected kiro args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat kiro agent backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--kiro-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat kiro agent backend ok",
        "hat-backend:kiro-agent",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_hat_backend_kiro_acp_agent() {
    let _mock = install_mock_acp_executions(vec![MockAcpExecution::success(
        true,
        vec![make_worker_event(
            "review.done",
            "hat kiro-acp agent backend ok",
        )],
    )]);

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::KiroAgent {
            backend_type: "kiro-acp".to_string(),
            agent: "reviewer-agent".to_string(),
            args: vec!["--unused-extra".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat kiro-acp agent backend ok",
        "hat-backend:kiro-acp-agent",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_supports_custom_hat_backend_with_backend_args() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_path = write_fake_executable(
        temp_dir.path(),
        "custom-wave-worker",
        r#"found_custom_arg=0
found_hat_runtime_arg=0
for arg in "$@"; do
  if [ "$arg" = "--from-custom-backend" ]; then
found_custom_arg=1
  fi
  if [ "$arg" = "--hat-runtime-arg" ]; then
found_hat_runtime_arg=1
  fi
done
if [ "$found_custom_arg" -ne 1 ] || [ "$found_hat_runtime_arg" -ne 1 ]; then
  echo "missing expected custom args: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat custom backend ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    );

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Custom {
            command: worker_path.display().to_string(),
            args: vec!["--from-custom-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_success_marked(&completed, "hat custom backend ok", "hat-backend:custom");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_missing_custom_hat_backend_command() {
    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Custom {
            command: "/definitely/missing-custom-wave-worker".to_string(),
            args: vec!["--from-custom-backend".to_string()],
        },
        Some(vec!["--hat-runtime-arg".to_string()]),
        missing_global_wave_backend(),
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "missing-custom-wave-worker",
        "hat-backend:custom-missing-command",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_missing_text_backend_command() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let wave = make_test_wave(vec!["review.done".to_string()]);

    let completed = execute_wave(
        &wave,
        &missing_global_wave_backend(),
        &events_file,
        false,
        false,
        None,
        None,
    )
    .await
    .expect("wave execution");

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "missing-wave-worker",
        "execution-mode:pty-spawn-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_pty_open_failure() {
    let completed = run_wave_for_backend_with_test_env(
        BackendOutputFormat::Text,
        &text_backend_body("unused"),
        30,
        vec![("RALPH_TEST_FORCE_PTY_OPEN_FAIL", "mock openpty exploded")],
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock openpty exploded",
        "pty:open-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_synthesizes_failure_events_for_pty_reader_failure() {
    let completed = run_wave_for_backend_with_test_env(
        BackendOutputFormat::Text,
        &text_backend_body("unused"),
        30,
        vec![(
            "RALPH_TEST_FORCE_PTY_READER_FAIL",
            "mock reader clone exploded",
        )],
    )
    .await;

    assert_single_failure_with_synthetic_events_marked(
        &completed,
        "mock reader clone exploded",
        "pty:reader-failure-visible",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_falls_back_to_global_backend_when_hat_backend_is_invalid() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_path = write_fake_executable(
        temp_dir.path(),
        "wave-worker",
        r#"found_fallback_arg=0
for arg in "$@"; do
  if [ "$arg" = "--fallback-arg" ]; then
found_fallback_arg=1
  fi
done
if [ "$found_fallback_arg" -ne 1 ]; then
  echo "missing --fallback-arg: $*" >&2
  exit 1
fi
cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"review.done","payload":"hat backend fallback ok","ts":"2026-01-01T00:00:00Z"}
EOF"#,
    );
    let global_backend = CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    let completed = run_wave_for_hat_backend(
        ralph_core::HatBackend::Named("definitely-invalid-backend".to_string()),
        Some(vec!["--fallback-arg".to_string()]),
        global_backend,
    )
    .await;

    assert_single_success_marked(
        &completed,
        "hat backend fallback ok",
        "hat-backend:invalid-fallback",
    );
    emit_wave_validation_marker("hat-backend:invalid-fallback", &["error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_run_wave_worker_acp_timeout_with_partial_events_keeps_events_visible() {
    let _mock =
        install_mock_acp_executions(vec![MockAcpExecution::timeout(vec![make_worker_event(
            "review.done",
            "partial acp result",
        )])]);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let worker_events_path = temp_dir.path().join("wave-events.jsonl");
    let merged_events_path = temp_dir.path().join("events.jsonl");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = CliBackend::kiro_acp();

    let (_index, outcome) = run_wave_worker_acp(
        0,
        &backend,
        "prompt",
        &worker_events_path,
        Duration::from_millis(1),
        tx,
        None,
        None,
    )
    .await;

    let (events, _duration, success) =
        outcome.expect("partial ACP timeout should preserve emitted events");
    assert!(!success, "timed out worker should not be marked successful");
    assert_eq!(events.len(), 1, "unexpected partial events: {events:?}");
    assert_eq!(events[0].topic.as_str(), "review.done");
    assert_eq!(events[0].payload.as_deref(), Some("partial acp result"));

    let completed = ralph_core::CompletedWave {
        wave_id: "w-acp".to_string(),
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: events.into_iter().map(ralph_proto::Event::from).collect(),
        }],
        failures: vec![],
        duration: Duration::from_millis(1),
    };

    merge_wave_results_to_events_file(
        &completed,
        &merged_events_path,
        &["review.done".to_string()],
    )
    .expect("merge partial ACP results");

    let merged = std::fs::read_to_string(&merged_events_path).expect("read merged events");
    let records: Vec<serde_json::Value> = merged
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 1, "unexpected merged records: {records:?}");
    assert_eq!(records[0]["topic"], "review.done");
    assert_eq!(records[0]["payload"], "partial acp result");
    assert!(
        records
            .iter()
            .all(|record| record["topic"] != "wave.worker.failed"),
        "partial ACP timeout should not synthesize worker failures: {records:?}"
    );
    emit_wave_validation_marker("acp:partial-timeout-visible-events", &["backend", "error"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_text_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::Text,
        &body_with_post_event_sleep(text_backend_body("text partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "text partial timeout ok",
        "pty:text-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_claude_stream_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::StreamJson,
        &body_with_post_event_sleep(claude_backend_body("claude partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "claude partial timeout ok",
        "pty:claude-stream-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_wave_keeps_pi_stream_partial_timeout_events_visible() {
    let completed = run_wave_for_backend_with_timeout(
        BackendOutputFormat::PiStreamJson,
        &body_with_post_event_sleep(pi_backend_body("pi partial timeout ok")),
        1,
    )
    .await;

    assert_partial_timeout_events_visible_marked(
        &completed,
        "pi partial timeout ok",
        "pty:pi-stream-partial-timeout-visible-events",
    );
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_uses_acp_for_kiro_acp_backend() {
    let backend = CliBackend::kiro_acp();
    assert_eq!(
        wave_worker_execution_mode(backend.output_format),
        WaveWorkerExecutionMode::Acp
    );
    emit_wave_validation_marker("execution-mode:kiro-acp-backend", &["backend"]);
}

#[cfg(unix)]
#[test]
fn test_wave_worker_execution_mode_uses_acp_for_kiro_acp_hat_backend() {
    let backend = CliBackend::from_hat_backend(&ralph_core::HatBackend::KiroAgent {
        backend_type: "kiro-acp".to_string(),
        agent: "reviewer".to_string(),
        args: vec![],
    })
    .expect("kiro-acp backend");
    assert_eq!(
        wave_worker_execution_mode(backend.output_format),
        WaveWorkerExecutionMode::Acp
    );
    emit_wave_validation_marker("execution-mode:kiro-acp-hat-backend", &["backend"]);
}

#[cfg(unix)]
#[tokio::test]
async fn test_run_wave_worker_pty_surfaces_spawn_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let missing_cmd = temp_dir.path().join("missing-wave-worker");
    let worker_events_path = temp_dir.path().join("wave-events.jsonl");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let backend = CliBackend {
        command: missing_cmd.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![],
    };

    let (_index, outcome) = run_wave_worker_pty(
        0,
        &backend,
        "prompt",
        &worker_events_path,
        Duration::from_secs(1),
        tx,
        None,
        None,
    )
    .await;

    let (error, _duration) = outcome.expect_err("missing worker should fail to spawn");
    assert!(
        error.contains("PTY spawn failed"),
        "unexpected error: {error}"
    );
    emit_wave_validation_marker("pty:spawn-failure", &["error"]);
}

#[cfg(unix)]
#[test]
fn test_merge_wave_results_to_events_file_synthesizes_failure_events() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let events_file = temp_dir.path().join("events.jsonl");
    let completed = ralph_core::CompletedWave {
        wave_id: "w-test".to_string(),
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![ralph_proto::Event::new("review.done", "worker ok")],
        }],
        failures: vec![ralph_core::WaveFailure {
            index: 1,
            error: "PTY spawn failed: missing-worker".to_string(),
            duration: Duration::from_secs(1),
        }],
        duration: Duration::from_secs(1),
    };

    merge_wave_results_to_events_file(
        &completed,
        &events_file,
        &["review.done".to_string(), "review.audit".to_string()],
    )
    .expect("merge wave results");

    let content = std::fs::read_to_string(&events_file).expect("read merged events");
    let records: Vec<serde_json::Value> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("json event"))
        .collect();

    assert_eq!(records.len(), 4, "unexpected merged records: {records:?}");
    assert!(records.iter().any(|record| {
        record["topic"] == "wave.worker.failed"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("PTY spawn failed: missing-worker"))
            && record["wave_index"] == 1
    }));
    assert!(records.iter().any(|record| {
        record["topic"] == "review.done"
            && record["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("## Worker 1 (FAILED)"))
    }));
    assert!(records.iter().any(|record| {
        record["topic"] == "review.audit"
            && record["payload"].as_str().is_some_and(|payload| {
                payload.contains("Error: PTY spawn failed: missing-worker")
            })
    }));
    emit_wave_validation_marker(
        "merge-wave-results:synthetic-failure-events",
        &["error", "synthetic"],
    );
}

#[cfg(unix)]
#[test]
fn test_get_last_commit_info_returns_none_without_git() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let _cwd = CwdGuard::set(temp_dir.path());
    let missing_git = temp_dir.path().join("git");
    assert!(get_last_commit_info_with_cmd(missing_git.as_os_str()).is_none());
}

#[cfg(unix)]
#[test]
fn test_get_last_commit_info_reads_last_commit() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo_root)
        .status()
        .expect("git init");

    std::fs::write(repo_root.join("README.md"), "hello").expect("write file");
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .status()
        .expect("git add");

    Command::new("git")
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "Initial commit",
            "--quiet",
        ])
        .current_dir(repo_root)
        .status()
        .expect("git commit");

    let _cwd = CwdGuard::set(repo_root);
    let info = get_last_commit_info_with_cmd(OsStr::new("git")).expect("commit info");
    assert!(
        info.contains("Initial commit"),
        "unexpected commit info: {info}"
    );
}

#[test]
fn test_process_pending_merges_handles_missing_preset() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    process_pending_merges(repo_root);
}

#[cfg(unix)]
#[test]
fn test_process_pending_merges_spawns_for_queue_entry() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    let queue_file = repo_root.join(".ralph/merge-queue/loop-1234.json");
    std::fs::write(
        &queue_file,
        r#"{"loop_id":"1234","state":"queued","created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .expect("queue file");

    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(&bin_dir, "ralph", "exit 0");

    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());
}

#[test]
fn test_process_pending_merges_missing_command_keeps_queue() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("loop-9999", "merge prompt").expect("enqueue");

    process_pending_merges_with_command(repo_root, OsStr::new("ralph-command-missing-12345"));

    let config_path = repo_root.join(".ralph/merge-loop-config.yml");
    assert!(config_path.exists());
    let entries = queue
        .list_by_state(ralph_core::merge_queue::MergeState::Queued)
        .expect("list queued");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].loop_id, "loop-9999");
}

#[test]
fn test_process_pending_merges_with_empty_queue_no_config_written() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();
    std::fs::create_dir_all(repo_root.join(".ralph/merge-queue")).expect("queue dir");

    let config_path = repo_root.join(".ralph/merge-loop-config.yml");
    assert!(!config_path.exists());

    process_pending_merges_with_command(repo_root, OsStr::new("ralph"));

    assert!(!config_path.exists());
}

#[cfg(unix)]
#[test]
fn test_process_pending_merges_redirects_subprocess_output_to_log_file() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    // Enqueue a merge entry using the proper API
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("test-loop", "merge prompt").expect("enqueue");

    // Create a fake ralph that writes to both stdout and stderr
    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(
        &bin_dir,
        "ralph",
        "echo 'stdout output' && echo 'stderr output' >&2 && sleep 0.1",
    );

    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());

    // Wait for subprocess to finish writing
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify a log file was created under .ralph/diagnostics/logs/
    let logs_dir = repo_root.join(".ralph/diagnostics/logs");
    assert!(logs_dir.exists(), "diagnostics logs directory should exist");

    let log_files: Vec<_> = std::fs::read_dir(&logs_dir)
        .expect("read logs dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ralph-merge-"))
        .collect();
    assert!(
        !log_files.is_empty(),
        "should have at least one merge subprocess log file"
    );

    // Verify the log file contains the subprocess output
    let log_content = std::fs::read_to_string(log_files[0].path()).expect("read log file");
    assert!(
        log_content.contains("stdout output"),
        "log file should contain stdout, got: {log_content}"
    );
    assert!(
        log_content.contains("stderr output"),
        "log file should contain stderr, got: {log_content}"
    );
}

#[cfg(unix)]
#[test]
fn test_process_pending_merges_falls_back_to_null_on_log_creation_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    // Block log file creation by placing a regular file where the logs directory would be
    let diagnostics_dir = repo_root.join(".ralph/diagnostics");
    std::fs::create_dir_all(&diagnostics_dir).expect("diagnostics dir");
    std::fs::write(diagnostics_dir.join("logs"), "not a directory").expect("block logs dir");

    // Enqueue a merge entry using the proper API
    let queue = ralph_core::merge_queue::MergeQueue::new(repo_root);
    queue.enqueue("test-loop", "merge prompt").expect("enqueue");

    // Create a fake ralph
    let bin_dir = repo_root.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ralph_path = write_fake_executable(&bin_dir, "ralph", "exit 0");

    // Should not panic even though log file creation fails
    process_pending_merges_with_command(repo_root, ralph_path.as_os_str());
}

#[test]
fn test_resolve_prompt_content_inline_precedence() {
    let mut config = RalphConfig::default();
    config.event_loop.prompt = Some("inline prompt".to_string());
    config.event_loop.prompt_file = "missing.md".to_string();

    let resolved = resolve_prompt_content(&config.event_loop).expect("inline prompt");
    assert_eq!(resolved, "inline prompt");
}

#[test]
fn test_resolve_prompt_content_from_file() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let prompt_path = temp_dir.path().join("PROMPT.md");
    std::fs::write(&prompt_path, "file prompt").expect("write prompt");

    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();

    let resolved = resolve_prompt_content(&config.event_loop).expect("file prompt");
    assert_eq!(resolved, "file prompt");
}

#[test]
fn test_resolve_prompt_content_missing_file_errors() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let missing_path = temp_dir.path().join("missing.md");

    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = missing_path.to_string_lossy().to_string();

    let err = resolve_prompt_content(&config.event_loop).expect_err("missing prompt");
    assert!(
        err.to_string().contains("Prompt file"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_resolve_prompt_content_no_prompt_errors() {
    let mut config = RalphConfig::default();
    config.event_loop.prompt = None;
    config.event_loop.prompt_file = String::new();

    let err = resolve_prompt_content(&config.event_loop).expect_err("missing prompt");
    assert!(
        err.to_string().contains("No prompt specified"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_log_events_from_output_records_orphan_event() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let mut registry = HatRegistry::new();
    let mut hat = Hat::new("planner", "Planner");
    hat.subscriptions.push(Topic::new("task.start"));
    registry.register(hat);

    let output = "<event topic=\"task.start\">start</event>\n\
<event topic=\"unknown.event\">oops</event>";
    let hat_id = HatId::new("tester");

    log_events_from_output(&mut logger, 1, &hat_id, output, &registry);

    let content = std::fs::read_to_string(&log_path).expect("read events");
    let records: Vec<EventRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect();

    let topics: std::collections::HashSet<String> =
        records.iter().map(|record| record.topic.clone()).collect();
    assert!(topics.contains("task.start"));
    assert!(topics.contains("unknown.event"));
    assert!(topics.contains("event.orphaned"));

    let triggered = records
        .iter()
        .find(|record| record.topic == "task.start")
        .and_then(|record| record.triggered.clone());
    assert_eq!(triggered.as_deref(), Some("planner"));
}

#[test]
fn test_log_terminate_event_writes_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("events.jsonl");
    let mut logger = EventLogger::new(&log_path);

    let event = Event::new("loop.terminate", "done");
    log_terminate_event(&mut logger, 7, &event);

    let content = std::fs::read_to_string(&log_path).expect("read events");
    let records: Vec<EventRecord> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("record"))
        .collect();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].topic, "loop.terminate");
    assert_eq!(records[0].hat, "loop");
    assert_eq!(records[0].iteration, 7);
}

#[test]
fn test_check_planning_session_responses_publishes_user_response() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx.clone());

    let conversation_path = ctx.planning_conversation_path(&session_id);
    std::fs::create_dir_all(conversation_path.parent().expect("parent"))
        .expect("create conversation dir");

    let prompt_entry = ConversationEntry {
        entry_type: ConversationType::UserPrompt,
        id: "prompt-1".to_string(),
        text: "Which option?".to_string(),
        ts: "2026-01-31T00:00:00Z".to_string(),
    };
    let response_entry = ConversationEntry {
        entry_type: ConversationType::UserResponse,
        id: "response-1".to_string(),
        text: "Option A".to_string(),
        ts: "2026-01-31T00:00:01Z".to_string(),
    };
    let conversation = format!(
        "{}\n{}\n",
        serde_json::to_string(&prompt_entry).expect("serialize prompt"),
        serde_json::to_string(&response_entry).expect("serialize response")
    );
    std::fs::write(&conversation_path, conversation).expect("write conversation");

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("check responses");
    {
        let events = published.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].topic.as_str(), "user.response");
        assert!(events[0].payload.contains("response-1"));
    }

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("dedup responses");
    let events = published.lock().unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn test_check_planning_session_responses_for_session_no_context_is_ok() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, "session-no-context")
        .expect("check responses");

    assert!(published.lock().unwrap().is_empty());
}

#[test]
fn test_check_planning_session_responses_skips_invalid_json() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    );

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx.clone());

    let conversation_path = ctx.planning_conversation_path(&session_id);
    std::fs::create_dir_all(conversation_path.parent().expect("parent"))
        .expect("create conversation dir");

    let prompt_entry = ConversationEntry {
        entry_type: ConversationType::UserPrompt,
        id: "prompt-1".to_string(),
        text: "Choose one".to_string(),
        ts: "2026-01-31T00:00:00Z".to_string(),
    };
    let conversation = format!(
        "not-json\n{}\n",
        serde_json::to_string(&prompt_entry).expect("serialize prompt")
    );
    std::fs::write(&conversation_path, conversation).expect("write conversation");

    let published = std::sync::Arc::new(Mutex::new(Vec::new()));
    let published_clone = std::sync::Arc::clone(&published);
    event_loop
        .bus()
        .add_observer(move |event| published_clone.lock().unwrap().push(event.clone()));

    check_planning_session_responses_for_session(&mut event_loop, &session_id)
        .expect("check responses");

    assert!(published.lock().unwrap().is_empty());
}

#[test]
fn test_recover_late_events_before_fallback_routes_pending_work() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.rejected", "hypothesis.confirmed", "fix.verified"]
    publishes: ["hypothesis.test", "fix.propose", "DEBUG_COMPLETE"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
    publishes: ["hypothesis.confirmed", "hypothesis.rejected"]
"#;
    let (mut event_loop, loop_ctx) =
        dispatch_test_event_loop_from_yaml_with_context(temp_dir.path(), yaml);
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"hypothesis.test","payload":"Race condition suspected","ts":"2026-03-08T00:00:00Z"}}"#
    )
    .expect("write late event");
    events_file.flush().expect("flush late event");

    let outcome =
        recover_late_events_before_fallback(&mut event_loop).expect("recover late events");
    assert_eq!(outcome, LateEventRecovery::PendingWork);
    assert_eq!(
        event_loop.next_hat().map(|hat| hat.as_str()),
        Some("ralph"),
        "late downstream work should route the next iteration to Ralph in multi-hat mode"
    );

    let tester_id = HatId::new("tester");
    let tester_pending = event_loop
        .bus()
        .peek_pending(&tester_id)
        .cloned()
        .unwrap_or_default();
    assert_eq!(tester_pending.len(), 1);
    assert_eq!(tester_pending[0].topic.as_str(), "hypothesis.test");
}

#[test]
fn test_recover_late_events_before_fallback_honors_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
    )
    .expect("write completion event");
    events_file.flush().expect("flush completion event");

    let outcome =
        recover_late_events_before_fallback(&mut event_loop).expect("recover completion");
    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

#[test]
fn test_recover_late_events_before_fallback_polls_for_delayed_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let delayed_events_path = events_path.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let mut events_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&delayed_events_path)
            .expect("open delayed events file");
        writeln!(
            events_file,
            r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
        )
        .expect("write delayed completion event");
        events_file.flush().expect("flush delayed completion event");
    });

    let outcome =
        recover_late_events_before_fallback(&mut event_loop).expect("recover completion");
    writer.join().expect("join delayed event writer");

    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

#[test]
fn test_recover_expected_emit_after_output_polls_for_delayed_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let delayed_events_path = events_path.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let mut events_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&delayed_events_path)
            .expect("open delayed events file");
        writeln!(
            events_file,
            r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
        )
        .expect("write delayed completion event");
        events_file.flush().expect("flush delayed completion event");
    });

    let outcome =
        recover_expected_emit_after_output(&mut event_loop).expect("recover expected emit");
    writer.join().expect("join delayed event writer");

    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

#[test]
fn test_resolve_display_hat_for_execution_prefers_prompt_selected_hat_for_ralph() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

#[test]
fn test_resolve_display_hat_for_execution_ignores_targeted_task_resume_noise() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["task.resume", "debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("task.resume", "Recovery").with_target("investigator"));
    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

#[test]
fn test_resolve_display_hat_for_execution_prefers_downstream_event_over_start_event() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus()
        .publish(Event::new("debug.start", "Investigate the bug"));
    event_loop
        .bus()
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));
    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("ralph"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "tester");
}

#[test]
fn test_resolve_display_hat_for_execution_keeps_explicit_non_ralph_hat() {
    let event_loop = EventLoop::new(RalphConfig::default());

    let display_hat = resolve_display_hat_for_execution(
        &event_loop,
        &HatId::new("fixer"),
        &HatId::new("investigator"),
    );

    assert_eq!(display_hat.as_str(), "fixer");
}

#[test]
fn test_output_mentions_ralph_emit_detects_tool_call_output() {
    assert!(output_mentions_ralph_emit(
        r#"[Tool] Bash: ralph emit "hypothesis.test" "payload""#
    ));
    assert!(!output_mentions_ralph_emit("[Tool] Bash: cargo test"));
}
