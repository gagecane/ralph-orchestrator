//! Wave execution: intra-loop parallelism for hats with `concurrency > 1`.
//!
//! When a hat emits wave events, the loop spawns multiple parallel backend
//! workers (up to the hat's `concurrency` limit), each processing one
//! payload with isolated events files. Results are merged back into the
//! main events file so the aggregator hat can pick them up.
//!
//! This module contains:
//! - [`handle_wave_events`] — detect + orchestrate wave dispatch
//! - [`execute_wave`] — spawn N workers in parallel
//! - [`run_wave_worker`] / [`run_wave_worker_acp`] / [`run_wave_worker_pty`] —
//!   per-worker execution strategies
//! - [`merge_wave_results_to_events_file`] — merge worker outputs back to
//!   the main events file
//! - Mock infrastructure (`#[cfg(test)]`) for ACP worker testing

use anyhow::{Context, Result};
use ralph_adapters::{
    AcpExecutor, CliBackend, CopilotStreamParser,
    OutputFormat as BackendOutputFormat, PiAssistantEvent, PiContentBlock, PiStreamEvent,
    PiStreamParser, StreamHandler,
};
use ralph_core::LoopContext;
use ralph_proto::RpcEvent;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::display::{
    print_wave_header, print_wave_summary, print_wave_worker_done,
};


/// Push a styled line to a TUI wave worker's output buffer.
fn push_to_wave_worker_buffer(
    state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    worker_idx: usize,
    lines: &[Line<'static>],
) {
    let Ok(s) = state.lock() else { return };
    let Some(ref wave) = s.wave_active else {
        return;
    };
    let Some(buffer) = wave.worker_buffers.get(worker_idx) else {
        return;
    };
    let handle = buffer.lines_handle();
    let Ok(mut buf_lines) = handle.lock() else {
        return;
    };
    buf_lines.extend_from_slice(lines);
}

/// Push a line to the latest iteration's output in the TUI.
fn push_to_tui_iteration(state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>, line: Line<'static>) {
    let Ok(s) = state.lock() else { return };
    let Some(handle) = s.latest_iteration_lines_handle() else {
        return;
    };
    let Ok(mut lines) = handle.lock() else { return };
    lines.push(line);
}

/// Bundled output channels for wave progress reporting.
struct WaveOutputs<'a> {
    use_colors: bool,
    /// Show wave progress on CLI (no TUI, no RPC).
    show_cli: bool,
    rpc_tx: Option<&'a tokio::sync::mpsc::Sender<RpcEvent>>,
    tui: Option<&'a Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

/// Handle wave events: detect, execute, merge results, and update UI.
///
/// Orchestrates the full wave lifecycle — detection, parallel execution,
/// result merging back to the events file, and re-reading for aggregator
/// activation. Updates CLI, RPC, and TUI outputs as appropriate.
pub(super) async fn handle_wave_events(
    wave_events: &[ralph_core::Event],
    event_loop: &mut ralph_core::EventLoop,
    backend: &CliBackend,
    ctx: &LoopContext,
    use_colors: bool,
    enable_rpc: bool,
    rpc_event_tx: Option<&tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<&Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) {
    let Some(detected) = ralph_core::detect_wave_events(wave_events, event_loop.registry()) else {
        return;
    };

    info!(
        wave_id = %detected.wave_id,
        total = detected.total,
        hat = %detected.target_hat,
        concurrency = detected.hat_config.concurrency,
        "Wave detected, executing parallel workers"
    );

    let out = WaveOutputs {
        use_colors,
        show_cli: tui_state.is_none() && !enable_rpc,
        rpc_tx: rpc_event_tx,
        tui: tui_state,
    };

    let wave_timeout_secs = detected.timeout_secs();

    // Announce wave start to CLI / RPC / TUI
    if out.show_cli {
        print_wave_header(
            &detected.hat_config.name,
            detected.total as usize,
            wave_timeout_secs,
            out.use_colors,
        );
    }
    if let Some(tx) = out.rpc_tx {
        let _ = tx.try_send(RpcEvent::WaveStarted {
            hat_name: detected.hat_config.name.clone(),
            worker_count: detected.total,
            timeout_secs: wave_timeout_secs,
        });
    }
    if let Some(state) = out.tui {
        if let Ok(mut s) = state.lock() {
            info!(
                hat = %detected.hat_config.name,
                workers = detected.total,
                "Setting wave_active on TUI state"
            );
            s.wave_active = Some(ralph_tui::state::WaveInfo::new(
                detected.hat_config.name.clone(),
                detected.total,
            ));
            s.wave_active_iteration_idx = Some(s.iterations.len().saturating_sub(1));
            if let Some(ref wave) = s.wave_active {
                for (i, buffer) in wave.worker_buffers.iter().enumerate() {
                    if let Ok(mut buf_lines) = buffer.lines_handle().lock() {
                        buf_lines.push(Line::from(Span::styled(
                            format!("Worker {}/{}: launching...", i + 1, detected.total),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }
        }
        let header_line = Line::from(vec![
            Span::styled("── WAVE: ", Style::default().fg(Color::Magenta)),
            Span::styled(
                format!(
                    "{} | {} workers | timeout {}s",
                    detected.hat_config.name, detected.total, wave_timeout_secs
                ),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                " ──────────────────────",
                Style::default().fg(Color::Magenta),
            ),
        ]);
        push_to_tui_iteration(state, header_line);
    }

    let main_events_file = super::resolve_current_events_path(ctx);
    let wave_result = execute_wave(
        &detected,
        backend,
        &main_events_file,
        out.show_cli,
        out.use_colors,
        out.rpc_tx.cloned(),
        out.tui.map(Arc::clone),
    )
    .await;

    match wave_result {
        Ok(completed) => {
            // Report completion to CLI / RPC / TUI
            if out.show_cli {
                print_wave_summary(
                    completed.results.len(),
                    completed.failures.len(),
                    completed.duration,
                    out.use_colors,
                );
            }
            if let Some(tx) = out.rpc_tx {
                let _ = tx.try_send(RpcEvent::WaveCompleted {
                    succeeded: completed.results.len(),
                    failed: completed.failures.len(),
                    duration_ms: completed.duration.as_millis() as u64,
                });
            }
            if let Some(state) = out.tui {
                if let Ok(mut s) = state.lock() {
                    let wave_iter_idx = s.wave_active_iteration_idx.take();
                    if let Some(wave) = s.wave_active.take() {
                        let target_idx =
                            wave_iter_idx.unwrap_or(s.iterations.len().saturating_sub(1));
                        if let Some(buf) = s.iterations.get_mut(target_idx) {
                            buf.wave_info = Some(wave);
                        }
                    }
                }
                let secs = completed.duration.as_secs();
                let color = if completed.failures.is_empty() {
                    Color::Green
                } else {
                    Color::Yellow
                };
                let line = Line::from(Span::styled(
                    format!(
                        "── Wave complete: {} succeeded, {} failed ({}s) ──────────────────────",
                        completed.results.len(),
                        completed.failures.len(),
                        secs,
                    ),
                    Style::default().fg(color),
                ));
                push_to_tui_iteration(state, line);
            }

            info!(
                wave_id = %completed.wave_id,
                results = completed.results.len(),
                failures = completed.failures.len(),
                duration_ms = completed.duration.as_millis() as u64,
                "Wave completed"
            );

            // Merge result events into main events file so aggregator hat picks them up
            if let Err(e) = merge_wave_results_to_events_file(
                &completed,
                &main_events_file,
                &detected.hat_config.publishes,
            ) {
                warn!(error = %e, "Failed to merge wave results to events file");
            }

            // Re-read events file to publish wave results to the bus.
            // The EventReader's position was before the merge, so it picks up
            // only the newly appended events (e.g. review.done). These target
            // the aggregator hat (concurrency=1), so they're partitioned as
            // regular events and published to the bus — making next_hat()
            // find the aggregator on the next iteration.
            if let Ok(reread) = event_loop.process_events_from_jsonl_with_waves()
                && reread.processed.had_events
            {
                info!("Published wave result events to bus for aggregator");
                // Wave results legitimately share the same topic (e.g.
                // 3x review.done). Reset the stale-loop counter so
                // this batch doesn't trigger LoopStale termination.
                event_loop.reset_stale_topic_counter();
            }
        }
        Err(e) => {
            warn!(error = %e, "Wave execution failed");
        }
    }
}

/// Execute a detected wave by spawning parallel backend instances.
///
/// Creates per-worker event files, spawns workers with concurrency-limited
/// semaphore, collects results, and returns a `CompletedWave`.
pub(super) async fn execute_wave(
    wave: &ralph_core::DetectedWave,
    global_backend: &CliBackend,
    main_events_file: &Path,
    show_progress: bool,
    use_colors: bool,
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) -> Result<ralph_core::CompletedWave> {
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    let concurrency = wave.hat_config.concurrency as usize;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));

    let wave_timeout = Duration::from_secs(wave.timeout_secs());

    // Register wave in tracker
    let mut tracker = WaveTracker::new();
    tracker.register_wave(wave.wave_id.clone(), wave.total);

    // Resolve per-worker events directory
    let wave_dir = main_events_file
        .parent()
        .unwrap_or(Path::new(".ralph"))
        .to_path_buf();

    // Build payload previews for display (first ~60 chars of each event payload)
    let payload_previews: Vec<String> = wave
        .events
        .iter()
        .map(|e| e.payload.as_deref().unwrap_or("").replace('\n', " "))
        .collect();

    // Channel for real-time per-worker progress reporting
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();

    // Spawn workers
    let mut handles = Vec::new();
    for (index, event) in wave.events.iter().enumerate() {
        let permit = semaphore.clone().acquire_owned().await?;
        let wave_id = wave.wave_id.clone();
        let index = index as u32;
        let event = event.clone();
        let hat_config = wave.hat_config.clone();

        // Create per-worker events file
        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index));

        // Build worker prompt
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
        };
        let prompt = build_wave_worker_prompt(&hat_config, &event, &ctx);

        // Resolve backend for this worker
        let mut worker_backend = if let Some(ref hat_backend) = hat_config.backend {
            CliBackend::from_hat_backend(hat_backend).unwrap_or_else(|_| global_backend.clone())
        } else {
            global_backend.clone()
        };

        #[cfg(test)]
        {
            // Test-only: allow fake backend PATH injection to flow through the global backend
            // when a wave worker resolves its command from a hat-specific backend.
            for (key, value) in &global_backend.env_vars {
                if !worker_backend
                    .env_vars
                    .iter()
                    .any(|(existing, _)| existing == key)
                {
                    worker_backend.env_vars.push((key.clone(), value.clone()));
                }
            }
        }

        // Inject wave env vars
        worker_backend.env_vars.extend([
            ("RALPH_WAVE_WORKER".into(), "1".into()),
            ("RALPH_WAVE_ID".into(), wave_id.clone()),
            ("RALPH_WAVE_INDEX".into(), index.to_string()),
            (
                "RALPH_EVENTS_FILE".into(),
                worker_events_file.display().to_string(),
            ),
        ]);

        // Apply hat backend args
        if let Some(ref args) = hat_config.backend_args {
            worker_backend.args.extend(args.iter().cloned());
        }

        let worker_events_path = worker_events_file.clone();
        let tx = progress_tx.clone();
        let worker_rpc_tx = rpc_event_tx.clone();
        let worker_tui_state = tui_state.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // Hold permit for concurrency limiting
            run_wave_worker(
                index,
                &worker_backend,
                &prompt,
                &worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
            )
            .await
        });

        handles.push(handle);
    }

    // Drop our sender so the receiver terminates when all workers finish
    drop(progress_tx);

    // Spawn a task to report real-time progress (CLI, RPC, and/or TUI)
    let total = wave.total;
    let previews = payload_previews;
    let progress_handle = tokio::spawn(async move {
        while let Some((index, success, duration)) = progress_rx.recv().await {
            let preview = previews
                .get(index as usize)
                .map(|s| s.as_str())
                .unwrap_or("");
            if show_progress {
                print_wave_worker_done(index, total, duration, success, preview, use_colors);
            }
            if let Some(ref tx) = rpc_event_tx {
                let _ = tx.try_send(RpcEvent::WaveWorkerDone {
                    index,
                    total,
                    duration_ms: duration.as_millis() as u64,
                    success,
                    payload_preview: preview.to_string(),
                });
            }
            if let Some(ref state) = tui_state {
                if let Ok(mut s) = state.lock()
                    && let Some(ref mut wave) = s.wave_active
                {
                    wave.completed += 1;
                }
                let secs = duration.as_secs();
                let (icon, color) = if success {
                    ("\u{2713}", Color::Green)
                } else {
                    ("\u{2717}", Color::Red)
                };
                let status_word = if success { "done" } else { "failed" };
                let truncated_preview = if preview.len() > 60 {
                    &preview[..ralph_core::floor_char_boundary(preview, 60)]
                } else {
                    preview
                };
                let line = Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::raw(format!(
                        "Worker {}/{} {} ({}s) — {}",
                        index + 1,
                        total,
                        status_word,
                        secs,
                        truncated_preview
                    )),
                ]);
                push_to_tui_iteration(state, line);
            }
        }
    });

    // Compute aggregate timeout: enough wall-clock time for all batches + buffer.
    // With semaphore-based concurrency limiting, total time is bounded by
    // ceil(total / concurrency) * per_worker_timeout.
    let batches = u64::from(wave.total).div_ceil(concurrency as u64);
    let aggregate_timeout = Duration::from_secs(wave_timeout.as_secs().saturating_mul(batches))
        + Duration::from_secs(30);

    // Collect results with aggregate timeout to prevent indefinite hangs
    let results =
        match tokio::time::timeout(aggregate_timeout, futures::future::join_all(handles)).await {
            Ok(results) => results,
            Err(_) => {
                warn!(
                    timeout_secs = aggregate_timeout.as_secs(),
                    "Wave aggregate timeout reached, cancelling remaining workers"
                );
                Vec::new()
            }
        };

    // Wait for progress reporter to finish
    let _ = progress_handle.await;

    let mut reported_indices = std::collections::HashSet::new();

    for result in results {
        match result {
            Ok((index, Ok((events, _duration, _success)))) => {
                reported_indices.insert(index);
                let proto_events: Vec<ralph_proto::Event> =
                    events.into_iter().map(ralph_proto::Event::from).collect();
                tracker.record_result(&wave.wave_id, index, proto_events);
            }
            Ok((index, Err((error, duration)))) => {
                reported_indices.insert(index);
                tracker.record_failure(&wave.wave_id, index, error, duration);
            }
            Err(join_err) => {
                // Task panicked or was cancelled — index is lost from JoinError.
                // The missing-index sweep below will record the failure.
                warn!(error = %join_err, "Wave worker task panicked");
            }
        }
    }

    // Record failures for any workers that didn't report back (panicked or timed out).
    for i in 0..wave.total {
        if !reported_indices.contains(&i) {
            warn!(
                worker = i,
                wave_id = %wave.wave_id,
                "Worker did not report — recording synthetic failure"
            );
            tracker.record_failure(
                &wave.wave_id,
                i,
                "Worker panicked or was cancelled by aggregate timeout".into(),
                aggregate_timeout,
            );
        }
    }

    // Take completed wave results
    tracker
        .take_wave_results(&wave.wave_id)
        .ok_or_else(|| anyhow::anyhow!("Wave {} not found in tracker", wave.wave_id))
}

type WaveWorkerOutcome =
    std::result::Result<(Vec<ralph_core::Event>, Duration, bool), (String, Duration)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WaveWorkerExecutionMode {
    Pty,
    Acp,
}

pub(super) fn wave_worker_execution_mode(output_format: BackendOutputFormat) -> WaveWorkerExecutionMode {
    match output_format {
        BackendOutputFormat::Acp => WaveWorkerExecutionMode::Acp,
        _ => WaveWorkerExecutionMode::Pty,
    }
}

struct WaveWorkerStreamHandler {
    worker_index: u32,
    rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

impl WaveWorkerStreamHandler {
    fn new(
        worker_index: u32,
        rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
        tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    ) -> Self {
        Self {
            worker_index,
            rpc_tx,
            tui_state,
        }
    }

    fn emit_delta(&self, delta: impl Into<String>) {
        let delta = delta.into();
        if delta.is_empty() {
            return;
        }

        if let Some(ref rpc_tx) = self.rpc_tx {
            let _ = rpc_tx.try_send(RpcEvent::WaveWorkerTextDelta {
                worker_index: self.worker_index,
                delta: delta.clone(),
            });
        }

        if let Some(ref state) = self.tui_state {
            let tui_lines = ralph_tui::text_to_lines(&delta);
            push_to_wave_worker_buffer(state, self.worker_index as usize, &tui_lines);
        }
    }
}

impl StreamHandler for WaveWorkerStreamHandler {
    fn on_text(&mut self, text: &str) {
        self.emit_delta(text);
    }

    fn on_tool_call(&mut self, name: &str, _id: &str, input: &serde_json::Value) {
        self.emit_delta(format!("⚙ {name}({input})\n"));
    }

    fn on_tool_result(&mut self, _id: &str, output: &str) {
        if output.is_empty() {
            return;
        }
        self.emit_delta(format!("→ {}\n", truncate_wave_worker_preview(output)));
    }

    fn on_error(&mut self, error: &str) {
        if error.is_empty() {
            return;
        }
        self.emit_delta(format!("✗ {}\n", truncate_wave_worker_preview(error)));
    }

    fn on_complete(&mut self, _result: &ralph_adapters::SessionResult) {}
}

async fn run_wave_worker(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) -> (u32, WaveWorkerOutcome) {
    match wave_worker_execution_mode(worker_backend.output_format) {
        WaveWorkerExecutionMode::Pty => {
            run_wave_worker_pty(
                index,
                worker_backend,
                prompt,
                worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
            )
            .await
        }
        WaveWorkerExecutionMode::Acp => {
            run_wave_worker_acp(
                index,
                worker_backend,
                prompt,
                worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
            )
            .await
        }
    }
}

enum AcpWaveExecutionResult {
    Completed(std::result::Result<bool, String>),
    TimedOut,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) enum MockAcpExecution {
    Success {
        success: bool,
        events: Vec<ralph_core::Event>,
    },
    Error {
        error: String,
        events: Vec<ralph_core::Event>,
    },
    Timeout {
        events: Vec<ralph_core::Event>,
    },
}

#[cfg(test)]
impl MockAcpExecution {
    pub(super) fn success(success: bool, events: Vec<ralph_core::Event>) -> Self {
        Self::Success { success, events }
    }

    pub(super) fn error(error: impl Into<String>, events: Vec<ralph_core::Event>) -> Self {
        Self::Error {
            error: error.into(),
            events,
        }
    }

    pub(super) fn timeout(events: Vec<ralph_core::Event>) -> Self {
        Self::Timeout { events }
    }

    fn write_capture(&self, worker_backend: &CliBackend, prompt: &str, worker_events_path: &Path) {
        let capture_path = PathBuf::from(format!("{}.capture", worker_events_path.display()));
        let env = worker_backend
            .env_vars
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        let capture = serde_json::json!({
            "command": worker_backend.command.as_str(),
            "args": &worker_backend.args,
            "env": env,
            "prompt": prompt,
        });
        std::fs::write(
            &capture_path,
            serde_json::to_string(&capture).expect("serialize mock ACP invocation"),
        )
        .expect("write mock ACP invocation");
    }

    fn write_events(&self, worker_events_path: &Path) {
        let events = match self {
            Self::Success { events, .. }
            | Self::Error { events, .. }
            | Self::Timeout { events } => events,
        };

        if events.is_empty() {
            let _ = fs::remove_file(worker_events_path);
            return;
        }

        let content = events
            .iter()
            .map(|event| serde_json::to_string(event).expect("serialize mock ACP event"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(worker_events_path, format!("{content}\n")).expect("write mock ACP events");
    }
}

#[cfg(test)]
pub(super) static MOCK_ACP_EXECUTIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::VecDeque<MockAcpExecution>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

#[cfg(test)]
pub(super) static MOCK_ACP_EXECUTION_SERIAL: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

async fn execute_wave_worker_acp_prompt(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    _worker_events_path: &Path,
    wave_timeout: Duration,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) -> AcpWaveExecutionResult {
    #[cfg(test)]
    {
        if let Some(mock) = {
            let mut queued = MOCK_ACP_EXECUTIONS
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            queued.pop_front()
        } {
            mock.write_capture(worker_backend, prompt, _worker_events_path);
            mock.write_events(_worker_events_path);
            return match mock {
                MockAcpExecution::Success { success, .. } => {
                    AcpWaveExecutionResult::Completed(Ok(success))
                }
                MockAcpExecution::Error { error, .. } => {
                    AcpWaveExecutionResult::Completed(Err(error))
                }
                MockAcpExecution::Timeout { .. } => AcpWaveExecutionResult::TimedOut,
            };
        }
    }

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let executor = AcpExecutor::new(worker_backend.clone(), workspace_root);
    let mut handler = WaveWorkerStreamHandler::new(index, worker_rpc_tx, worker_tui_state);

    match tokio::time::timeout(wave_timeout, executor.execute(prompt, &mut handler)).await {
        Ok(Ok(result)) => AcpWaveExecutionResult::Completed(Ok(result.success)),
        Ok(Err(e)) => AcpWaveExecutionResult::Completed(Err(e.to_string())),
        Err(_) => AcpWaveExecutionResult::TimedOut,
    }
}

pub(super) async fn run_wave_worker_acp(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) -> (u32, WaveWorkerOutcome) {
    let start = std::time::Instant::now();
    let result = execute_wave_worker_acp_prompt(
        index,
        worker_backend,
        prompt,
        worker_events_path,
        wave_timeout,
        worker_rpc_tx,
        worker_tui_state,
    )
    .await;
    let duration = start.elapsed();
    let events = read_worker_events(worker_events_path);
    let _ = fs::remove_file(worker_events_path);

    match result {
        AcpWaveExecutionResult::Completed(Ok(success)) => {
            let _ = tx.send((index, success, duration));
            (index, Ok((events, duration, success)))
        }
        AcpWaveExecutionResult::Completed(Err(error)) => {
            let _ = tx.send((index, false, duration));
            (
                index,
                Err((format!("ACP worker failed: {error}"), duration)),
            )
        }
        AcpWaveExecutionResult::TimedOut if events.is_empty() => {
            let _ = tx.send((index, false, duration));
            (
                index,
                Err((
                    format!(
                        "Worker timed out after {}s without emitting events",
                        wave_timeout.as_secs()
                    ),
                    duration,
                )),
            )
        }
        AcpWaveExecutionResult::TimedOut => {
            let _ = tx.send((index, false, duration));
            (index, Ok((events, duration, false)))
        }
    }
}

#[cfg(test)]
fn forced_test_wave_pty_failure<'a>(worker_backend: &'a CliBackend, key: &str) -> Option<&'a str> {
    worker_backend
        .env_vars
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value.as_str()))
}

pub(super) async fn run_wave_worker_pty(
    index: u32,
    worker_backend: &CliBackend,
    prompt: &str,
    worker_events_path: &Path,
    wave_timeout: Duration,
    tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
) -> (u32, WaveWorkerOutcome) {
    let start = std::time::Instant::now();

    // Build and spawn process in a PTY for real-time stdout streaming.
    // Node.js structured backends buffer stdout when it's a pipe, so NDJSON
    // events only arrive when the process exits. Using a PTY forces the
    // child to see a terminal and flush after each line.
    let (cmd, args, stdin_input, _temp_file_guard) = worker_backend.build_command(prompt, false);
    let mut stdin_prompt_file = None;

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let output_format = worker_backend.output_format;

    #[cfg(test)]
    if let Some(error) =
        forced_test_wave_pty_failure(worker_backend, "RALPH_TEST_FORCE_PTY_OPEN_FAIL")
    {
        let duration = start.elapsed();
        let _ = fs::remove_file(worker_events_path);
        let _ = tx.send((index, false, duration));
        return (index, Err((format!("PTY open failed: {error}"), duration)));
    }

    // Spawn worker in a PTY so stdout is unbuffered
    let pty_system = portable_pty::native_pty_system();
    let pty_pair = match pty_system.openpty(portable_pty::PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY open failed: {e}"), duration)));
        }
    };

    let (spawn_cmd, spawn_args) = if let Some(input) = stdin_input.as_ref() {
        let mut prompt_file = match tempfile::NamedTempFile::new() {
            Ok(file) => file,
            Err(e) => {
                let duration = start.elapsed();
                let _ = fs::remove_file(worker_events_path);
                let _ = tx.send((index, false, duration));
                return (
                    index,
                    Err((
                        format!("PTY stdin temp file creation failed: {e}"),
                        duration,
                    )),
                );
            }
        };
        if let Err(e) = std::io::Write::write_all(&mut prompt_file, input.as_bytes()) {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (
                index,
                Err((format!("PTY stdin temp file write failed: {e}"), duration)),
            );
        }

        let wrapper_args = std::iter::once("-c".to_string())
            .chain(std::iter::once(
                r#"prompt_file="$1"; shift; exec "$@" < "$prompt_file""#.to_string(),
            ))
            .chain(std::iter::once("sh".to_string()))
            .chain(std::iter::once(prompt_file.path().display().to_string()))
            .chain(std::iter::once(cmd.clone()))
            .chain(args.iter().cloned())
            .collect::<Vec<_>>();
        stdin_prompt_file = Some(prompt_file);
        ("sh".to_string(), wrapper_args)
    } else {
        (cmd.clone(), args.clone())
    };

    let mut cmd_builder = portable_pty::CommandBuilder::new(&spawn_cmd);
    cmd_builder.args(&spawn_args);
    cmd_builder.cwd(&cwd);
    for (key, value) in &worker_backend.env_vars {
        cmd_builder.env(key, value);
    }
    cmd_builder.env("TERM", "dumb");
    cmd_builder.env("NO_COLOR", "1");

    let mut child = match pty_pair.slave.spawn_command(cmd_builder) {
        Ok(child) => child,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY spawn failed: {e}"), duration)));
        }
    };
    drop(pty_pair.slave);

    if let Some(input) = stdin_input
        && stdin_prompt_file.is_none()
        && let Ok(mut writer) = pty_pair.master.take_writer()
    {
        let _ = writer.write_all(input.as_bytes());
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }

    #[cfg(test)]
    if let Some(error) =
        forced_test_wave_pty_failure(worker_backend, "RALPH_TEST_FORCE_PTY_READER_FAIL")
    {
        let duration = start.elapsed();
        let _ = fs::remove_file(worker_events_path);
        let _ = tx.send((index, false, duration));
        return (
            index,
            Err((format!("PTY reader failed: {error}"), duration)),
        );
    }

    let pty_reader = match pty_pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(e) => {
            let duration = start.elapsed();
            let _ = fs::remove_file(worker_events_path);
            let _ = tx.send((index, false, duration));
            return (index, Err((format!("PTY reader failed: {e}"), duration)));
        }
    };

    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(256);
    let reader_handle = std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(pty_reader);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if line_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut timed_out = false;
    let stream_result = async {
        let mut line_count: u64 = 0;
        while let Some(line) = line_rx.recv().await {
            line_count += 1;
            if line_count == 1 {
                info!(
                    worker = index,
                    line_len = line.len(),
                    ?output_format,
                    "Wave worker: first stdout line received"
                );
            }
            if let Some(delta) = extract_readable_delta(&line, output_format) {
                if let Some(ref rpc_tx) = worker_rpc_tx {
                    let _ = rpc_tx.try_send(RpcEvent::WaveWorkerTextDelta {
                        worker_index: index,
                        delta: delta.clone(),
                    });
                }
                if let Some(ref state) = worker_tui_state {
                    let tui_lines = ralph_tui::text_to_lines(&delta);
                    push_to_wave_worker_buffer(state, index as usize, &tui_lines);
                }
            }
        }
        Ok::<_, std::io::Error>(())
    };

    match tokio::time::timeout(wave_timeout, stream_result).await {
        Ok(result) => {
            if let Err(e) = result {
                warn!(error = %e, worker = index, "Wave worker I/O error");
            }
        }
        Err(_) => {
            warn!(
                timeout_secs = wave_timeout.as_secs(),
                worker = index,
                "Wave worker timeout, killing process"
            );
            timed_out = true;
            let _ = child.kill();
        }
    }

    let (status, _) = tokio::task::spawn_blocking(move || {
        let status = child.wait();
        let _ = reader_handle.join();
        (status, ())
    })
    .await
    .unwrap_or_else(|_| (Err(std::io::Error::other("join task panicked")), ()));
    let success = status.map(|s| s.success() && !timed_out).unwrap_or(false);
    let duration = start.elapsed();

    let events = read_worker_events(worker_events_path);
    let _ = fs::remove_file(worker_events_path);

    if timed_out && events.is_empty() {
        let _ = tx.send((index, false, duration));
        (
            index,
            Err((
                format!(
                    "Worker timed out after {}s without emitting events",
                    wave_timeout.as_secs()
                ),
                duration,
            )),
        )
    } else {
        let _ = tx.send((index, success, duration));
        (index, Ok((events, duration, success)))
    }
}

fn truncate_wave_worker_preview(text: &str) -> String {
    if text.len() > 200 {
        let end = ralph_core::floor_char_boundary(text, 200);
        format!("{}…", &text[..end])
    } else {
        text.to_string()
    }
}

/// Extract a human-readable text delta from a single stdout line.
pub(super) fn extract_readable_delta(line: &str, output_format: BackendOutputFormat) -> Option<String> {
    match output_format {
        BackendOutputFormat::Text | BackendOutputFormat::Acp => Some(format!("{line}\n")),
        BackendOutputFormat::StreamJson => {
            use ralph_adapters::{ClaudeStreamEvent, ClaudeStreamParser, ContentBlock};
            match ClaudeStreamParser::parse_line(line) {
                Some(ClaudeStreamEvent::Assistant { message, .. }) => {
                    let mut text = String::new();
                    for block in message.content {
                        match block {
                            ContentBlock::Text { text: t } => {
                                text.push_str(&t);
                                text.push('\n');
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                text.push_str(&format!("⚙ {name}({input})\n"));
                            }
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                Some(ClaudeStreamEvent::User { message }) => {
                    let mut text = String::new();
                    for block in message.content {
                        let ralph_adapters::UserContentBlock::ToolResult { content, .. } = block;
                        if !content.is_empty() {
                            text.push_str(&format!(
                                "→ {}\n",
                                truncate_wave_worker_preview(&content)
                            ));
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                _ => None,
            }
        }
        BackendOutputFormat::CopilotStreamJson => {
            CopilotStreamParser::extract_text(line).map(|text| {
                if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                }
            })
        }
        BackendOutputFormat::PiStreamJson => match PiStreamParser::parse_line(line) {
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::TextDelta { delta },
            }) => Some(delta),
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::Error { reason },
            }) => Some(format!("✗ {}\n", truncate_wave_worker_preview(&reason))),
            Some(PiStreamEvent::ToolExecutionStart {
                tool_name, args, ..
            }) => Some(format!("⚙ {tool_name}({args})\n")),
            Some(PiStreamEvent::ToolExecutionEnd {
                result, is_error, ..
            }) => {
                let output = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        PiContentBlock::Text { text } => Some(text.as_str()),
                        PiContentBlock::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if output.is_empty() {
                    None
                } else if is_error {
                    Some(format!("✗ {}\n", truncate_wave_worker_preview(&output)))
                } else {
                    Some(format!("→ {}\n", truncate_wave_worker_preview(&output)))
                }
            }
            _ => None,
        },
    }
}

/// Read events from a per-worker events file.
fn read_worker_events(path: &Path) -> Vec<ralph_core::Event> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<ralph_core::Event>(line).ok())
        .collect()
}

/// Merge wave result events into the main events file.
///
/// Appends all result events to the main JSONL file so the aggregator hat
/// picks them up on the next iteration.
pub(super) fn merge_wave_results_to_events_file(
    completed: &ralph_core::CompletedWave,
    events_file: &Path,
    publish_topics: &[String],
) -> Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    let ts = chrono::Utc::now().to_rfc3339();

    // Build all records into a single buffer, then write_all once for atomic
    // append (consistent with EventLogger::log and write_wave_events).
    let mut buf = String::new();

    for result in &completed.results {
        for event in &result.events {
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload,
                "ts": ts,
                "wave_id": completed.wave_id,
                "wave_index": result.index,
            });
            buf.push_str(&serde_json::to_string(&record)?);
            buf.push('\n');
        }
    }

    // Also write failure events so the aggregator knows about partial results
    for failure in &completed.failures {
        let record = serde_json::json!({
            "topic": "wave.worker.failed",
            "payload": format!("Worker {} failed: {}", failure.index, failure.error),
            "ts": ts,
            "wave_id": completed.wave_id,
            "wave_index": failure.index,
        });
        buf.push_str(&serde_json::to_string(&record)?);
        buf.push('\n');

        // Emit synthetic events on the hat's publish topics so downstream
        // aggregators can still trigger even when workers fail/timeout
        for topic in publish_topics {
            let record = serde_json::json!({
                "topic": topic,
                "payload": format!(
                    "## Worker {} (FAILED)\n\nError: {}",
                    failure.index, failure.error
                ),
                "ts": ts,
                "wave_id": completed.wave_id,
                "wave_index": failure.index,
            });
            buf.push_str(&serde_json::to_string(&record)?);
            buf.push('\n');
        }
    }

    file.write_all(buf.as_bytes())?;

    Ok(())
}
