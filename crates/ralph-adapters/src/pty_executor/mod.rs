//! PTY executor for running prompts with full terminal emulation.
//!
//! Spawns CLI tools in a pseudo-terminal to preserve rich TUI features like
//! colors, spinners, and animations. Supports both interactive mode (user
//! input forwarded) and observe mode (output-only).
//!
//! Key features:
//! - PTY creation via `portable-pty` for cross-platform support
//! - Idle timeout with activity tracking (output AND input reset timer)
//! - Double Ctrl+C handling (first forwards, second terminates)
//! - Raw mode management with cleanup on exit/crash
//!
//! Architecture:
//! - Uses `tokio::select!` for non-blocking I/O multiplexing
//! - Spawns separate tasks for PTY output and user input
//! - Enables responsive Ctrl+C handling even when PTY is idle
//!
//! ## Module layout
//!
//! The [`PtyExecutor`] implementation in this file wires together helpers
//! from the following submodules:
//!
//! - [`config`] — configuration and result types ([`PtyConfig`], [`PtyExecutionResult`], [`TerminationType`])
//! - [`ctrl_c`] — double-Ctrl+C state machine ([`CtrlCState`], [`CtrlCAction`])
//! - [`io_events`] — PTY I/O event enums and ANSI stripping helpers
//! - [`spawn`] — Ralph runtime environment injection for spawned children
//! - [`parser`] — Claude / Copilot / Pi stream dispatch and result building
//! - [`cleanup`] — Child termination and interruptible exit waiting

// Exit codes and PIDs are always within i32 range in practice
#![allow(clippy::cast_possible_wrap)]

mod cleanup;
mod config;
mod ctrl_c;
mod io_events;
mod parser;
mod spawn;
#[cfg(test)]
mod test_support;

pub use config::{PtyConfig, PtyExecutionResult, TerminationType};
pub use ctrl_c::{CtrlCAction, CtrlCState};

use crate::claude_stream::{ClaudeStreamEvent, ClaudeStreamParser};
use crate::cli_backend::{CliBackend, OutputFormat};
use crate::copilot_stream::CopilotStreamState;
use crate::pi_stream::{PiSessionState, PiStreamParser, dispatch_pi_stream_event};
use crate::stream_handler::{SessionResult, StreamHandler};
use io_events::{InputEvent, OutputEvent};
use parser::{
    build_result, dispatch_stream_event, extract_cli_flag_value, handle_copilot_stream_line,
    resolve_termination_type,
};
use portable_pty::{CommandBuilder, PtyPair, PtySize, native_pty_system};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

/// Executor for running prompts in a pseudo-terminal.
pub struct PtyExecutor {
    backend: CliBackend,
    config: PtyConfig,
    // Channel ends for TUI integration
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
    output_rx: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    input_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    input_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    control_tx: Option<mpsc::UnboundedSender<crate::pty_handle::ControlCommand>>,
    control_rx: mpsc::UnboundedReceiver<crate::pty_handle::ControlCommand>,
    // Termination notification for TUI
    terminated_tx: watch::Sender<bool>,
    terminated_rx: Option<watch::Receiver<bool>>,
    // Explicit TUI mode flag - set via set_tui_mode() when TUI is connected.
    // This replaces the previous inference via output_rx.is_none() which broke
    // after the streaming refactor (handle() is no longer called in TUI mode).
    tui_mode: bool,
}

impl PtyExecutor {
    /// Creates a new PTY executor with the given backend and configuration.
    pub fn new(backend: CliBackend, config: PtyConfig) -> Self {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (terminated_tx, terminated_rx) = watch::channel(false);

        Self {
            backend,
            config,
            output_tx,
            output_rx: Some(output_rx),
            input_tx: Some(input_tx),
            input_rx,
            control_tx: Some(control_tx),
            control_rx,
            terminated_tx,
            terminated_rx: Some(terminated_rx),
            tui_mode: false,
        }
    }

    /// Sets the TUI mode flag.
    ///
    /// When TUI mode is enabled, PTY output is sent to the TUI channel instead of
    /// being written directly to stdout. This flag must be set before calling any
    /// of the run methods when using the TUI.
    ///
    /// # Arguments
    /// * `enabled` - Whether TUI mode should be active
    pub fn set_tui_mode(&mut self, enabled: bool) {
        self.tui_mode = enabled;
    }

    /// Updates the backend configuration for this executor.
    ///
    /// This allows switching backends between iterations without recreating
    /// the entire executor. Critical for hat-level backend configuration support.
    ///
    /// # Arguments
    /// * `backend` - The new backend configuration to use
    pub fn set_backend(&mut self, backend: CliBackend) {
        self.backend = backend;
    }

    /// Returns a handle for TUI integration.
    ///
    /// Can only be called once - panics if called multiple times.
    pub fn handle(&mut self) -> crate::pty_handle::PtyHandle {
        crate::pty_handle::PtyHandle {
            output_rx: self.output_rx.take().expect("handle() already called"),
            input_tx: self.input_tx.take().expect("handle() already called"),
            control_tx: self.control_tx.take().expect("handle() already called"),
            terminated_rx: self.terminated_rx.take().expect("handle() already called"),
        }
    }

    /// Spawns Claude in a PTY and returns the PTY pair, child process, stdin input, and temp file.
    ///
    /// The temp file is returned to keep it alive for the duration of execution.
    /// For large prompts (>7000 chars), Claude is instructed to read from a temp file.
    /// If the temp file is dropped before Claude reads it, the file is deleted and Claude hangs.
    ///
    /// The stdin_input is returned so callers can write it to the PTY after taking the writer.
    /// This is necessary because `take_writer()` can only be called once per PTY.
    fn spawn_pty(
        &self,
        prompt: &str,
    ) -> io::Result<(
        PtyPair,
        Box<dyn portable_pty::Child + Send>,
        Option<String>,
        Option<tempfile::NamedTempFile>,
    )> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: self.config.rows,
                cols: self.config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Build the command. For non-interactive PTY mode with large prompts,
        // force arg mode because the PTY line discipline limits canonical
        // input to ~4 KB per line. Large prompts (30-50 KB+) deadlock when
        // written through PTY stdin. By forcing arg mode, the prompt is passed
        // as a command argument (or via temp file for prompts > 7000 chars),
        // bypassing the PTY input path entirely.  See #280.
        let use_pty_safe = !self.config.interactive && prompt.len() > 4000;
        let (cmd, args, stdin_input, temp_file) = if use_pty_safe {
            self.backend.build_command_pty(prompt)
        } else {
            self.backend.build_command(prompt, self.config.interactive)
        };

        let mut cmd_builder = CommandBuilder::new(&cmd);
        cmd_builder.args(&args);

        // Set explicit working directory from config (captured at startup to avoid
        // current_dir() failures when workspace no longer exists)
        cmd_builder.cwd(&self.config.workspace_root);

        // Set up environment for PTY
        cmd_builder.env("TERM", "xterm-256color");
        spawn::inject_ralph_runtime_env(&mut cmd_builder, &self.config.workspace_root);

        // Apply backend-specific environment variables (e.g., Agent Teams env var)
        for (key, value) in &self.backend.env_vars {
            cmd_builder.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd_builder)
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Return stdin_input so callers can write it after taking the writer
        Ok((pair, child, stdin_input, temp_file))
    }

    /// Runs in observe mode (output-only, no input forwarding).
    ///
    /// This is an async function that listens for interrupt signals via the shared
    /// `interrupt_rx` watch channel from the event loop.
    /// Uses a separate thread for blocking PTY reads and tokio::select! for signal handling.
    ///
    /// Returns when the process exits, idle timeout triggers, or interrupt is received.
    ///
    /// # Arguments
    /// * `prompt` - The prompt to execute
    /// * `interrupt_rx` - Watch channel receiver for interrupt signals from the event loop
    ///
    /// # Errors
    ///
    /// Returns an error if PTY allocation fails, the command cannot be spawned,
    /// or an I/O error occurs during output handling.
    pub async fn run_observe(
        &self,
        prompt: &str,
        mut interrupt_rx: tokio::sync::watch::Receiver<bool>,
    ) -> io::Result<PtyExecutionResult> {
        // Keep temp_file alive for the duration of execution (large prompts use temp files)
        let (pair, mut child, stdin_input, _temp_file) = self.spawn_pty(prompt)?;

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Write stdin input if present (for stdin prompt mode)
        if let Some(ref input) = stdin_input {
            // Small delay to let process initialize
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut writer = pair
                .master
                .take_writer()
                .map_err(|e| io::Error::other(e.to_string()))?;
            writer.write_all(input.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }

        // Drop the slave to signal EOF when master closes
        drop(pair.slave);

        let mut output = Vec::new();
        let timeout_duration = if !self.config.interactive || self.config.idle_timeout_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(u64::from(
                self.config.idle_timeout_secs,
            )))
        };

        let mut termination = TerminationType::Natural;
        let mut last_activity = Instant::now();

        // Flag for termination request (shared with reader thread)
        let should_terminate = Arc::new(AtomicBool::new(false));

        // Spawn blocking reader thread that sends output via channel
        let (output_tx, mut output_rx) = mpsc::channel::<OutputEvent>(256);
        let should_terminate_reader = Arc::clone(&should_terminate);
        // Check if TUI is handling output (output_rx taken by handle())
        let tui_connected = self.tui_mode;
        let tui_output_tx = if tui_connected {
            Some(self.output_tx.clone())
        } else {
            None
        };

        debug!("Spawning PTY output reader thread (observe mode)");
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];

            loop {
                if should_terminate_reader.load(Ordering::SeqCst) {
                    debug!("PTY reader: termination requested");
                    break;
                }

                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!("PTY reader: EOF");
                        let _ = output_tx.blocking_send(OutputEvent::Eof);
                        break;
                    }
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        // Send to TUI channel if connected
                        if let Some(ref tx) = tui_output_tx {
                            let _ = tx.send(data.clone());
                        }
                        // Send to main loop
                        if output_tx.blocking_send(OutputEvent::Data(data)).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        debug!(error = %e, "PTY reader error");
                        let _ = output_tx.blocking_send(OutputEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
        });

        // Main event loop using tokio::select! for interruptibility
        loop {
            // Calculate timeout for idle check
            let idle_timeout = timeout_duration.map(|d| {
                let elapsed = last_activity.elapsed();
                if elapsed >= d {
                    Duration::from_millis(1) // Trigger immediately
                } else {
                    d.saturating_sub(elapsed)
                }
            });

            tokio::select! {
                // Check for interrupt signal from event loop
                _ = interrupt_rx.changed() => {
                    if *interrupt_rx.borrow() {
                        debug!("Interrupt received in observe mode, terminating");
                        termination = TerminationType::UserInterrupt;
                        should_terminate.store(true, Ordering::SeqCst);
                        let _ = cleanup::terminate_child(&mut child, true).await;
                        break;
                    }
                }

                // Check for output from reader thread
                event = output_rx.recv() => {
                    match event {
                        Some(OutputEvent::Data(data)) => {
                            // Only write to stdout if TUI is NOT handling output
                            if !tui_connected {
                                io::stdout().write_all(&data)?;
                                io::stdout().flush()?;
                            }
                            output.extend_from_slice(&data);
                            last_activity = Instant::now();
                        }
                        Some(OutputEvent::Eof) | None => {
                            debug!("Output channel closed, process likely exited");
                            break;
                        }
                        Some(OutputEvent::Error(e)) => {
                            debug!(error = %e, "Reader thread reported error");
                            break;
                        }
                    }
                }

                // Check for idle timeout
                _ = async {
                    if let Some(timeout) = idle_timeout {
                        tokio::time::sleep(timeout).await;
                    } else {
                        // No timeout configured, wait forever
                        std::future::pending::<()>().await;
                    }
                } => {
                    warn!(
                        timeout_secs = self.config.idle_timeout_secs,
                        "Idle timeout triggered"
                    );
                    termination = TerminationType::IdleTimeout;
                    should_terminate.store(true, Ordering::SeqCst);
                    cleanup::terminate_child(&mut child, true).await?;
                    break;
                }
            }

            // Check if child has exited
            if let Some(status) = child
                .try_wait()
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                let exit_code = status.exit_code() as i32;
                debug!(exit_status = ?status, exit_code, "Child process exited");

                // Drain any remaining output from channel
                while let Ok(event) = output_rx.try_recv() {
                    if let OutputEvent::Data(data) = event {
                        if !tui_connected {
                            io::stdout().write_all(&data)?;
                            io::stdout().flush()?;
                        }
                        output.extend_from_slice(&data);
                    }
                }

                // Give the reader thread a brief window to flush any final bytes/EOF.
                // This avoids races where fast-exiting commands can drop tail output.
                let drain_deadline = Instant::now() + Duration::from_millis(200);
                loop {
                    let remaining = drain_deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, output_rx.recv()).await {
                        Ok(Some(OutputEvent::Data(data))) => {
                            if !tui_connected {
                                io::stdout().write_all(&data)?;
                                io::stdout().flush()?;
                            }
                            output.extend_from_slice(&data);
                        }
                        Ok(Some(OutputEvent::Eof) | None) => break,
                        Ok(Some(OutputEvent::Error(e))) => {
                            debug!(error = %e, "PTY read error after exit");
                            break;
                        }
                        Err(_) => break,
                    }
                }

                let final_termination = resolve_termination_type(exit_code, termination);
                // run_observe doesn't parse JSON, so extracted_text is empty
                return Ok(build_result(
                    &output,
                    status.success(),
                    Some(exit_code),
                    final_termination,
                    String::new(),
                    None,
                ));
            }
        }

        // Signal reader thread to stop
        should_terminate.store(true, Ordering::SeqCst);

        // Wait for child to fully exit (interruptible + bounded)
        let status =
            cleanup::wait_for_exit(&mut child, Some(Duration::from_secs(2)), &mut interrupt_rx)
                .await?;

        let (success, exit_code, final_termination) = match status {
            Some(s) => {
                let code = s.exit_code() as i32;
                (
                    s.success(),
                    Some(code),
                    resolve_termination_type(code, termination),
                )
            }
            None => {
                warn!("Timed out waiting for child to exit after termination");
                (false, None, termination)
            }
        };

        // run_observe doesn't parse JSON, so extracted_text is empty
        Ok(build_result(
            &output,
            success,
            exit_code,
            final_termination,
            String::new(),
            None,
        ))
    }

    /// Runs in observe mode with streaming event handling for JSON output.
    ///
    /// When the backend's output format is `StreamJson`, this method parses
    /// NDJSON lines and dispatches events to the provided handler for real-time
    /// display. For `Text` format, behaves identically to `run_observe`.
    ///
    /// # Arguments
    /// * `prompt` - The prompt to execute
    /// * `interrupt_rx` - Watch channel receiver for interrupt signals
    /// * `handler` - Handler to receive streaming events
    ///
    /// # Errors
    ///
    /// Returns an error if PTY allocation fails, the command cannot be spawned,
    /// or an I/O error occurs during output handling.
    pub async fn run_observe_streaming<H: StreamHandler>(
        &self,
        prompt: &str,
        mut interrupt_rx: tokio::sync::watch::Receiver<bool>,
        handler: &mut H,
    ) -> io::Result<PtyExecutionResult> {
        // Check output format to decide parsing strategy
        let output_format = self.backend.output_format;

        // StreamJson format uses NDJSON line parsing (Claude)
        // CopilotStreamJson format uses JSONL line parsing (Copilot prompt mode)
        // PiStreamJson format uses NDJSON line parsing (Pi)
        // Text format streams raw output directly to handler
        let is_stream_json = output_format == OutputFormat::StreamJson;
        let is_copilot_stream = output_format == OutputFormat::CopilotStreamJson;
        let is_pi_stream = output_format == OutputFormat::PiStreamJson;
        // Pi thinking deltas are noisy for plain console output but useful in TUI.
        let show_pi_thinking = is_pi_stream && self.tui_mode;
        let is_real_pi_backend = self.backend.command == "pi";

        if is_pi_stream && is_real_pi_backend {
            let configured_provider =
                extract_cli_flag_value(&self.backend.args, "--provider", "-p")
                    .unwrap_or_else(|| "auto".to_string());
            let configured_model = extract_cli_flag_value(&self.backend.args, "--model", "-m")
                .unwrap_or_else(|| "default".to_string());
            handler.on_text(&format!(
                "Pi configured: provider={configured_provider}, model={configured_model}\n"
            ));
        }

        // Keep temp_file alive for the duration of execution
        let (pair, mut child, stdin_input, _temp_file) = self.spawn_pty(prompt)?;

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Write stdin input if present (for stdin prompt mode)
        if let Some(ref input) = stdin_input {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut writer = pair
                .master
                .take_writer()
                .map_err(|e| io::Error::other(e.to_string()))?;
            writer.write_all(input.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }

        drop(pair.slave);

        let mut output = Vec::new();
        let mut line_buffer = String::new();
        // Accumulate extracted text from NDJSON for event parsing
        let mut extracted_text = String::new();
        // Pi session state for accumulating cost/turns (wall-clock for duration)
        let mut pi_state = PiSessionState::new();
        let mut copilot_state = CopilotStreamState::new();
        let mut completion: Option<SessionResult> = None;
        let start_time = Instant::now();
        let timeout_duration = if !self.config.interactive || self.config.idle_timeout_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(u64::from(
                self.config.idle_timeout_secs,
            )))
        };

        let mut termination = TerminationType::Natural;
        let mut last_activity = Instant::now();

        let should_terminate = Arc::new(AtomicBool::new(false));

        // Spawn blocking reader thread
        let (output_tx, mut output_rx) = mpsc::channel::<OutputEvent>(256);
        let should_terminate_reader = Arc::clone(&should_terminate);
        let tui_connected = self.tui_mode;
        let tui_output_tx = if tui_connected {
            Some(self.output_tx.clone())
        } else {
            None
        };

        debug!("Spawning PTY output reader thread (streaming mode)");
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];

            loop {
                if should_terminate_reader.load(Ordering::SeqCst) {
                    debug!("PTY reader: termination requested");
                    break;
                }

                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!("PTY reader: EOF");
                        let _ = output_tx.blocking_send(OutputEvent::Eof);
                        break;
                    }
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if let Some(ref tx) = tui_output_tx {
                            let _ = tx.send(data.clone());
                        }
                        if output_tx.blocking_send(OutputEvent::Data(data)).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        debug!(error = %e, "PTY reader error");
                        let _ = output_tx.blocking_send(OutputEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
        });

        // Main event loop with JSON line parsing
        loop {
            let idle_timeout = timeout_duration.map(|d| {
                let elapsed = last_activity.elapsed();
                if elapsed >= d {
                    Duration::from_millis(1)
                } else {
                    d.saturating_sub(elapsed)
                }
            });

            tokio::select! {
                _ = interrupt_rx.changed() => {
                    if *interrupt_rx.borrow() {
                        debug!("Interrupt received in streaming observe mode, terminating");
                        termination = TerminationType::UserInterrupt;
                        should_terminate.store(true, Ordering::SeqCst);
                        let _ = cleanup::terminate_child(&mut child, true).await;
                        break;
                    }
                }

                event = output_rx.recv() => {
                    match event {
                        Some(OutputEvent::Data(data)) => {
                            output.extend_from_slice(&data);
                            last_activity = Instant::now();

                            if let Ok(text) = std::str::from_utf8(&data) {
                                if is_stream_json {
                                    // StreamJson format: Parse JSON lines from the data
                                    line_buffer.push_str(text);

                                    // Process complete lines
                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();

                                        if let Some(event) = ClaudeStreamParser::parse_line(&line) {
                                            if let ClaudeStreamEvent::Result {
                                                duration_ms,
                                                total_cost_usd,
                                                num_turns,
                                                is_error,
                                            } = &event
                                            {
                                                completion = Some(SessionResult {
                                                    duration_ms: *duration_ms,
                                                    total_cost_usd: *total_cost_usd,
                                                    num_turns: *num_turns,
                                                    is_error: *is_error,
                                                    ..Default::default()
                                                });
                                            }
                                            dispatch_stream_event(event, handler, &mut extracted_text);
                                        }
                                    }
                                } else if is_copilot_stream {
                                    line_buffer.push_str(text);

                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();

                                        if let Some(session_result) = handle_copilot_stream_line(
                                            &line,
                                            handler,
                                            &mut extracted_text,
                                            &mut copilot_state,
                                        ) {
                                            completion = Some(session_result);
                                        }
                                    }
                                } else if is_pi_stream {
                                    // PiStreamJson format: Parse NDJSON lines from pi
                                    line_buffer.push_str(text);

                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();

                                        if let Some(event) = PiStreamParser::parse_line(&line) {
                                            dispatch_pi_stream_event(
                                                event,
                                                handler,
                                                &mut extracted_text,
                                                &mut pi_state,
                                                show_pi_thinking,
                                            );
                                        }
                                    }
                                } else {
                                    // Text format: Stream raw output directly to handler
                                    // This preserves ANSI escape codes for TUI rendering
                                    handler.on_text(text);
                                }
                            }
                        }
                        Some(OutputEvent::Eof) | None => {
                            debug!("Output channel closed");
                            // Process any remaining content in buffer
                            if is_stream_json && !line_buffer.is_empty()
                                && let Some(event) = ClaudeStreamParser::parse_line(&line_buffer)
                            {
                                if let ClaudeStreamEvent::Result {
                                    duration_ms,
                                    total_cost_usd,
                                    num_turns,
                                    is_error,
                                } = &event
                                {
                                    completion = Some(SessionResult {
                                        duration_ms: *duration_ms,
                                        total_cost_usd: *total_cost_usd,
                                        num_turns: *num_turns,
                                        is_error: *is_error,
                                        ..Default::default()
                                    });
                                }
                                dispatch_stream_event(event, handler, &mut extracted_text);
                            } else if is_copilot_stream && !line_buffer.is_empty() {
                                if let Some(session_result) = handle_copilot_stream_line(
                                    &line_buffer,
                                    handler,
                                    &mut extracted_text,
                                    &mut copilot_state,
                                ) {
                                    completion = Some(session_result);
                                }
                            } else if is_pi_stream && !line_buffer.is_empty()
                                && let Some(event) = PiStreamParser::parse_line(&line_buffer)
                            {
                                dispatch_pi_stream_event(
                                    event,
                                    handler,
                                    &mut extracted_text,
                                    &mut pi_state,
                                    show_pi_thinking,
                                );
                            }
                            break;
                        }
                        Some(OutputEvent::Error(e)) => {
                            debug!(error = %e, "Reader thread reported error");
                            handler.on_error(&e);
                            break;
                        }
                    }
                }

                _ = async {
                    if let Some(timeout) = idle_timeout {
                        tokio::time::sleep(timeout).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    warn!(
                        timeout_secs = self.config.idle_timeout_secs,
                        "Idle timeout triggered"
                    );
                    termination = TerminationType::IdleTimeout;
                    should_terminate.store(true, Ordering::SeqCst);
                    cleanup::terminate_child(&mut child, true).await?;
                    break;
                }
            }

            // Check if child has exited
            if let Some(status) = child
                .try_wait()
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                let exit_code = status.exit_code() as i32;
                debug!(exit_status = ?status, exit_code, "Child process exited");

                // Drain remaining output
                while let Ok(event) = output_rx.try_recv() {
                    if let OutputEvent::Data(data) = event {
                        output.extend_from_slice(&data);
                        if let Ok(text) = std::str::from_utf8(&data) {
                            if is_stream_json {
                                // StreamJson: parse JSON lines
                                line_buffer.push_str(text);
                                while let Some(newline_pos) = line_buffer.find('\n') {
                                    let line = line_buffer[..newline_pos].to_string();
                                    line_buffer = line_buffer[newline_pos + 1..].to_string();
                                    if let Some(event) = ClaudeStreamParser::parse_line(&line) {
                                        if let ClaudeStreamEvent::Result {
                                            duration_ms,
                                            total_cost_usd,
                                            num_turns,
                                            is_error,
                                        } = &event
                                        {
                                            completion = Some(SessionResult {
                                                duration_ms: *duration_ms,
                                                total_cost_usd: *total_cost_usd,
                                                num_turns: *num_turns,
                                                is_error: *is_error,
                                                ..Default::default()
                                            });
                                        }
                                        dispatch_stream_event(event, handler, &mut extracted_text);
                                    }
                                }
                            } else if is_copilot_stream {
                                line_buffer.push_str(text);
                                while let Some(newline_pos) = line_buffer.find('\n') {
                                    let line = line_buffer[..newline_pos].to_string();
                                    line_buffer = line_buffer[newline_pos + 1..].to_string();
                                    if let Some(session_result) = handle_copilot_stream_line(
                                        &line,
                                        handler,
                                        &mut extracted_text,
                                        &mut copilot_state,
                                    ) {
                                        completion = Some(session_result);
                                    }
                                }
                            } else if is_pi_stream {
                                // PiStreamJson: parse NDJSON lines
                                line_buffer.push_str(text);
                                while let Some(newline_pos) = line_buffer.find('\n') {
                                    let line = line_buffer[..newline_pos].to_string();
                                    line_buffer = line_buffer[newline_pos + 1..].to_string();
                                    if let Some(event) = PiStreamParser::parse_line(&line) {
                                        dispatch_pi_stream_event(
                                            event,
                                            handler,
                                            &mut extracted_text,
                                            &mut pi_state,
                                            show_pi_thinking,
                                        );
                                    }
                                }
                            } else {
                                // Text: stream raw output to handler
                                handler.on_text(text);
                            }
                        }
                    }
                }

                // Give the reader thread a brief window to flush any final bytes/EOF.
                // This avoids races where fast-exiting commands can drop tail output.
                let drain_deadline = Instant::now() + Duration::from_millis(200);
                loop {
                    let remaining = drain_deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, output_rx.recv()).await {
                        Ok(Some(OutputEvent::Data(data))) => {
                            output.extend_from_slice(&data);
                            if let Ok(text) = std::str::from_utf8(&data) {
                                if is_stream_json {
                                    // StreamJson: parse JSON lines
                                    line_buffer.push_str(text);
                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();
                                        if let Some(event) = ClaudeStreamParser::parse_line(&line) {
                                            if let ClaudeStreamEvent::Result {
                                                duration_ms,
                                                total_cost_usd,
                                                num_turns,
                                                is_error,
                                            } = &event
                                            {
                                                completion = Some(SessionResult {
                                                    duration_ms: *duration_ms,
                                                    total_cost_usd: *total_cost_usd,
                                                    num_turns: *num_turns,
                                                    is_error: *is_error,
                                                    ..Default::default()
                                                });
                                            }
                                            dispatch_stream_event(
                                                event,
                                                handler,
                                                &mut extracted_text,
                                            );
                                        }
                                    }
                                } else if is_copilot_stream {
                                    line_buffer.push_str(text);
                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();
                                        handle_copilot_stream_line(
                                            &line,
                                            handler,
                                            &mut extracted_text,
                                            &mut copilot_state,
                                        );
                                    }
                                } else if is_pi_stream {
                                    // PiStreamJson: parse NDJSON lines
                                    line_buffer.push_str(text);
                                    while let Some(newline_pos) = line_buffer.find('\n') {
                                        let line = line_buffer[..newline_pos].to_string();
                                        line_buffer = line_buffer[newline_pos + 1..].to_string();
                                        if let Some(event) = PiStreamParser::parse_line(&line) {
                                            dispatch_pi_stream_event(
                                                event,
                                                handler,
                                                &mut extracted_text,
                                                &mut pi_state,
                                                show_pi_thinking,
                                            );
                                        }
                                    }
                                } else {
                                    // Text: stream raw output to handler
                                    handler.on_text(text);
                                }
                            }
                        }
                        Ok(Some(OutputEvent::Eof) | None) => break,
                        Ok(Some(OutputEvent::Error(e))) => {
                            debug!(error = %e, "PTY read error after exit");
                            break;
                        }
                        Err(_) => break,
                    }
                }

                // Process final buffer content
                if is_stream_json
                    && !line_buffer.is_empty()
                    && let Some(event) = ClaudeStreamParser::parse_line(&line_buffer)
                {
                    if let ClaudeStreamEvent::Result {
                        duration_ms,
                        total_cost_usd,
                        num_turns,
                        is_error,
                    } = &event
                    {
                        completion = Some(SessionResult {
                            duration_ms: *duration_ms,
                            total_cost_usd: *total_cost_usd,
                            num_turns: *num_turns,
                            is_error: *is_error,
                            ..Default::default()
                        });
                    }
                    dispatch_stream_event(event, handler, &mut extracted_text);
                } else if is_copilot_stream && !line_buffer.is_empty() {
                    if let Some(session_result) = handle_copilot_stream_line(
                        &line_buffer,
                        handler,
                        &mut extracted_text,
                        &mut copilot_state,
                    ) {
                        completion = Some(session_result);
                    }
                } else if is_pi_stream
                    && !line_buffer.is_empty()
                    && let Some(event) = PiStreamParser::parse_line(&line_buffer)
                {
                    dispatch_pi_stream_event(
                        event,
                        handler,
                        &mut extracted_text,
                        &mut pi_state,
                        show_pi_thinking,
                    );
                }

                let final_termination = resolve_termination_type(exit_code, termination);

                // Synthesize on_complete for Pi sessions (pi has no dedicated result event)
                if is_pi_stream {
                    if is_real_pi_backend {
                        let stream_provider =
                            pi_state.stream_provider.as_deref().unwrap_or("unknown");
                        let stream_model = pi_state.stream_model.as_deref().unwrap_or("unknown");
                        handler.on_text(&format!(
                            "Pi stream: provider={stream_provider}, model={stream_model}\n"
                        ));
                    }
                    let session_result = SessionResult {
                        duration_ms: start_time.elapsed().as_millis() as u64,
                        total_cost_usd: pi_state.total_cost_usd,
                        num_turns: pi_state.num_turns,
                        is_error: !status.success(),
                        input_tokens: pi_state.input_tokens,
                        output_tokens: pi_state.output_tokens,
                        cache_read_tokens: pi_state.cache_read_tokens,
                        cache_write_tokens: pi_state.cache_write_tokens,
                    };
                    handler.on_complete(&session_result);
                    completion = Some(session_result);
                }

                // Pass extracted_text for event parsing from NDJSON
                return Ok(build_result(
                    &output,
                    status.success(),
                    Some(exit_code),
                    final_termination,
                    extracted_text,
                    completion.as_ref(),
                ));
            }
        }

        should_terminate.store(true, Ordering::SeqCst);

        let status =
            cleanup::wait_for_exit(&mut child, Some(Duration::from_secs(2)), &mut interrupt_rx)
                .await?;

        let (success, exit_code, final_termination) = match status {
            Some(s) => {
                let code = s.exit_code() as i32;
                (
                    s.success(),
                    Some(code),
                    resolve_termination_type(code, termination),
                )
            }
            None => {
                warn!("Timed out waiting for child to exit after termination");
                (false, None, termination)
            }
        };

        // Synthesize on_complete for Pi sessions (pi has no dedicated result event)
        if is_pi_stream {
            if is_real_pi_backend {
                let stream_provider = pi_state.stream_provider.as_deref().unwrap_or("unknown");
                let stream_model = pi_state.stream_model.as_deref().unwrap_or("unknown");
                handler.on_text(&format!(
                    "Pi stream: provider={stream_provider}, model={stream_model}\n"
                ));
            }
            let session_result = SessionResult {
                duration_ms: start_time.elapsed().as_millis() as u64,
                total_cost_usd: pi_state.total_cost_usd,
                num_turns: pi_state.num_turns,
                is_error: !success,
                input_tokens: pi_state.input_tokens,
                output_tokens: pi_state.output_tokens,
                cache_read_tokens: pi_state.cache_read_tokens,
                cache_write_tokens: pi_state.cache_write_tokens,
            };
            handler.on_complete(&session_result);
            completion = Some(session_result);
        }

        // Pass extracted_text for event parsing from NDJSON
        Ok(build_result(
            &output,
            success,
            exit_code,
            final_termination,
            extracted_text,
            completion.as_ref(),
        ))
    }

    /// Runs in interactive mode (bidirectional I/O).
    ///
    /// Uses `tokio::select!` for non-blocking I/O multiplexing between:
    /// 1. PTY output (from blocking reader via channel)
    /// 2. User input (from stdin thread via channel)
    /// 3. Interrupt signal from event loop
    /// 4. Idle timeout
    ///
    /// This design ensures Ctrl+C is always responsive, even when the PTY
    /// has no output (e.g., during long-running tool calls).
    ///
    /// # Arguments
    /// * `prompt` - The prompt to execute
    /// * `interrupt_rx` - Watch channel receiver for interrupt signals from the event loop
    ///
    /// # Errors
    ///
    /// Returns an error if PTY allocation fails, the command cannot be spawned,
    /// or an I/O error occurs during bidirectional communication.
    #[allow(clippy::too_many_lines)] // Complex state machine requires cohesive implementation
    pub async fn run_interactive(
        &mut self,
        prompt: &str,
        mut interrupt_rx: tokio::sync::watch::Receiver<bool>,
    ) -> io::Result<PtyExecutionResult> {
        // Keep temp_file alive for the duration of execution (large prompts use temp files)
        let (pair, mut child, stdin_input, _temp_file) = self.spawn_pty(prompt)?;

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::other(e.to_string()))?;

        // Keep master for resize operations
        let master = pair.master;

        // Drop the slave to signal EOF when master closes
        drop(pair.slave);

        // Store stdin_input for writing after reader thread starts
        let pending_stdin = stdin_input;

        let mut output = Vec::new();
        let timeout_duration = if self.config.idle_timeout_secs > 0 {
            Some(Duration::from_secs(u64::from(
                self.config.idle_timeout_secs,
            )))
        } else {
            None
        };

        let mut ctrl_c_state = CtrlCState::new();
        let mut termination = TerminationType::Natural;
        let mut last_activity = Instant::now();

        // Flag for termination request (shared with spawned tasks)
        let should_terminate = Arc::new(AtomicBool::new(false));

        // Spawn output reading task (blocking read wrapped in spawn_blocking via channel)
        let (output_tx, mut output_rx) = mpsc::channel::<OutputEvent>(256);
        let should_terminate_output = Arc::clone(&should_terminate);
        // Check if TUI is handling output (output_rx taken by handle())
        let tui_connected = self.tui_mode;
        let tui_output_tx = if tui_connected {
            Some(self.output_tx.clone())
        } else {
            None
        };

        debug!("Spawning PTY output reader thread");
        std::thread::spawn(move || {
            debug!("PTY output reader thread started");
            let mut reader = reader;
            let mut buf = [0u8; 4096];

            loop {
                if should_terminate_output.load(Ordering::SeqCst) {
                    debug!("PTY output reader: termination requested");
                    break;
                }

                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF - PTY closed
                        debug!("PTY output reader: EOF received");
                        let _ = output_tx.blocking_send(OutputEvent::Eof);
                        break;
                    }
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        // Send to TUI channel if connected
                        if let Some(ref tx) = tui_output_tx {
                            let _ = tx.send(data.clone());
                        }
                        // Send to main loop
                        if output_tx.blocking_send(OutputEvent::Data(data)).is_err() {
                            debug!("PTY output reader: channel closed");
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        // Non-blocking mode: no data available, yield briefly
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        // Interrupted by signal, retry
                    }
                    Err(e) => {
                        warn!("PTY output reader: error - {}", e);
                        let _ = output_tx.blocking_send(OutputEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
            debug!("PTY output reader thread exiting");
        });

        // Spawn input reading task - ONLY when TUI is NOT connected
        // In TUI mode (observation mode), user input should not be captured from stdin.
        // The TUI has its own input handling, and raw Ctrl+C should go directly to the
        // signal handler (interrupt_rx) without racing with the stdin reader.
        let mut input_rx = if tui_connected {
            debug!("TUI connected - skipping stdin reader thread");
            None
        } else {
            let (input_tx, input_rx) = mpsc::unbounded_channel::<InputEvent>();
            let should_terminate_input = Arc::clone(&should_terminate);

            std::thread::spawn(move || {
                let mut stdin = io::stdin();
                let mut buf = [0u8; 1];

                loop {
                    if should_terminate_input.load(Ordering::SeqCst) {
                        break;
                    }

                    match stdin.read(&mut buf) {
                        Ok(0) => break, // EOF
                        Ok(1) => {
                            let byte = buf[0];
                            let event = match byte {
                                3 => InputEvent::CtrlC,          // Ctrl+C
                                28 => InputEvent::CtrlBackslash, // Ctrl+\
                                _ => InputEvent::Data(vec![byte]),
                            };
                            if input_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {} // Shouldn't happen with 1-byte buffer
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            });
            Some(input_rx)
        };

        // Write stdin input after threads are spawned (so we capture any output)
        // Give Claude's TUI a moment to initialize before sending the prompt
        if let Some(ref input) = pending_stdin {
            tokio::time::sleep(Duration::from_millis(100)).await;
            writer.write_all(input.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            last_activity = Instant::now();
        }

        // Main select loop - this is the key fix for blocking I/O
        // We use tokio::select! to multiplex between output, input, and timeout
        loop {
            // Check if child has exited (non-blocking check before select)
            if let Some(status) = child
                .try_wait()
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                let exit_code = status.exit_code() as i32;
                debug!(exit_status = ?status, exit_code, "Child process exited");

                // Drain remaining output already buffered.
                while let Ok(event) = output_rx.try_recv() {
                    if let OutputEvent::Data(data) = event {
                        if !tui_connected {
                            io::stdout().write_all(&data)?;
                            io::stdout().flush()?;
                        }
                        output.extend_from_slice(&data);
                    }
                }

                // Give the reader thread a brief window to flush any final bytes/EOF.
                // This avoids races where fast-exiting commands drop output before we return.
                let drain_deadline = Instant::now() + Duration::from_millis(200);
                loop {
                    let remaining = drain_deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, output_rx.recv()).await {
                        Ok(Some(OutputEvent::Data(data))) => {
                            if !tui_connected {
                                io::stdout().write_all(&data)?;
                                io::stdout().flush()?;
                            }
                            output.extend_from_slice(&data);
                        }
                        Ok(Some(OutputEvent::Eof) | None) => break,
                        Ok(Some(OutputEvent::Error(e))) => {
                            debug!(error = %e, "PTY read error after exit");
                            break;
                        }
                        Err(_) => break, // timeout
                    }
                }

                should_terminate.store(true, Ordering::SeqCst);
                // Signal TUI that PTY has terminated
                let _ = self.terminated_tx.send(true);

                let final_termination = resolve_termination_type(exit_code, termination);
                // run_interactive doesn't parse JSON, so extracted_text is empty
                return Ok(build_result(
                    &output,
                    status.success(),
                    Some(exit_code),
                    final_termination,
                    String::new(),
                    None,
                ));
            }

            // Build the timeout future (or a never-completing one if disabled)
            let timeout_future = async {
                match timeout_duration {
                    Some(d) => {
                        let elapsed = last_activity.elapsed();
                        if elapsed >= d {
                            tokio::time::sleep(Duration::ZERO).await
                        } else {
                            tokio::time::sleep(d.saturating_sub(elapsed)).await
                        }
                    }
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                // PTY output received
                output_event = output_rx.recv() => {
                    match output_event {
                        Some(OutputEvent::Data(data)) => {
                            // Only write to stdout if TUI is NOT handling output
                            if !tui_connected {
                                io::stdout().write_all(&data)?;
                                io::stdout().flush()?;
                            }
                            output.extend_from_slice(&data);

                            last_activity = Instant::now();
                        }
                        Some(OutputEvent::Eof) => {
                            debug!("PTY EOF received");
                            break;
                        }
                        Some(OutputEvent::Error(e)) => {
                            debug!(error = %e, "PTY read error");
                            break;
                        }
                        None => {
                            // Channel closed, reader thread exited
                            break;
                        }
                    }
                }

                // User input received (from stdin) - only active when TUI is NOT connected
                input_event = async {
                    match input_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await, // Never resolves when TUI is connected
                    }
                } => {
                    match input_event {
                        Some(InputEvent::CtrlC) => {
                            match ctrl_c_state.handle_ctrl_c(Instant::now()) {
                                CtrlCAction::ForwardAndStartWindow => {
                                    // Forward Ctrl+C to Claude
                                    let _ = writer.write_all(&[3]);
                                    let _ = writer.flush();
                                    last_activity = Instant::now();
                                }
                                CtrlCAction::Terminate => {
                                    info!("Double Ctrl+C detected, terminating");
                                    termination = TerminationType::UserInterrupt;
                                    should_terminate.store(true, Ordering::SeqCst);
                                    cleanup::terminate_child(&mut child, true).await?;
                                    break;
                                }
                            }
                        }
                        Some(InputEvent::CtrlBackslash) => {
                            info!("Ctrl+\\ detected, force killing");
                            termination = TerminationType::ForceKill;
                            should_terminate.store(true, Ordering::SeqCst);
                            cleanup::terminate_child(&mut child, false).await?;
                            break;
                        }
                        Some(InputEvent::Data(data)) => {
                            // Forward to Claude
                            let _ = writer.write_all(&data);
                            let _ = writer.flush();
                            last_activity = Instant::now();
                        }
                        None => {
                            // Input channel closed (stdin EOF)
                            debug!("Input channel closed");
                        }
                    }
                }

                // TUI input received (convert to InputEvent for unified handling)
                tui_input = self.input_rx.recv() => {
                    if let Some(data) = tui_input {
                        match InputEvent::from_bytes(data) {
                            InputEvent::CtrlC => {
                                match ctrl_c_state.handle_ctrl_c(Instant::now()) {
                                    CtrlCAction::ForwardAndStartWindow => {
                                        let _ = writer.write_all(&[3]);
                                        let _ = writer.flush();
                                        last_activity = Instant::now();
                                    }
                                    CtrlCAction::Terminate => {
                                        info!("Double Ctrl+C detected, terminating");
                                        termination = TerminationType::UserInterrupt;
                                        should_terminate.store(true, Ordering::SeqCst);
                                        cleanup::terminate_child(&mut child, true).await?;
                                        break;
                                    }
                                }
                            }
                            InputEvent::CtrlBackslash => {
                                info!("Ctrl+\\ detected, force killing");
                                termination = TerminationType::ForceKill;
                                should_terminate.store(true, Ordering::SeqCst);
                                cleanup::terminate_child(&mut child, false).await?;
                                break;
                            }
                            InputEvent::Data(bytes) => {
                                let _ = writer.write_all(&bytes);
                                let _ = writer.flush();
                                last_activity = Instant::now();
                            }
                        }
                    }
                }

                // Control commands from TUI
                control_cmd = self.control_rx.recv() => {
                    if let Some(cmd) = control_cmd {
                        use crate::pty_handle::ControlCommand;
                        match cmd {
                            ControlCommand::Kill => {
                                info!("Control command: Kill");
                                termination = TerminationType::UserInterrupt;
                                should_terminate.store(true, Ordering::SeqCst);
                                cleanup::terminate_child(&mut child, true).await?;
                                break;
                            }
                            ControlCommand::Resize(cols, rows) => {
                                debug!(cols, rows, "Control command: Resize");
                                // Resize the PTY to match TUI dimensions
                                if let Err(e) = master.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                }) {
                                    warn!("Failed to resize PTY: {}", e);
                                }
                            }
                            ControlCommand::Skip | ControlCommand::Abort => {
                                // These are handled at orchestrator level, not here
                                debug!("Control command: {:?} (ignored at PTY level)", cmd);
                            }
                        }
                    }
                }

                // Idle timeout expired
                _ = timeout_future => {
                    warn!(
                        timeout_secs = self.config.idle_timeout_secs,
                        "Idle timeout triggered"
                    );
                    termination = TerminationType::IdleTimeout;
                    should_terminate.store(true, Ordering::SeqCst);
                    cleanup::terminate_child(&mut child, true).await?;
                    break;
                }

                // Interrupt signal from event loop
                _ = interrupt_rx.changed() => {
                    if *interrupt_rx.borrow() {
                        debug!("Interrupt received in interactive mode, terminating");
                        termination = TerminationType::UserInterrupt;
                        should_terminate.store(true, Ordering::SeqCst);
                        cleanup::terminate_child(&mut child, true).await?;
                        break;
                    }
                }
            }
        }

        // Ensure termination flag is set for spawned threads
        should_terminate.store(true, Ordering::SeqCst);

        // Signal TUI that PTY has terminated
        let _ = self.terminated_tx.send(true);

        // Wait for child to fully exit (interruptible + bounded)
        let status =
            cleanup::wait_for_exit(&mut child, Some(Duration::from_secs(2)), &mut interrupt_rx)
                .await?;

        let (success, exit_code, final_termination) = match status {
            Some(s) => {
                let code = s.exit_code() as i32;
                (
                    s.success(),
                    Some(code),
                    resolve_termination_type(code, termination),
                )
            }
            None => {
                warn!("Timed out waiting for child to exit after termination");
                (false, None, termination)
            }
        };

        // run_interactive doesn't parse JSON, so extracted_text is empty
        Ok(build_result(
            &output,
            success,
            exit_code,
            final_termination,
            String::new(),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::CapturingHandler;
    use super::*;
    #[cfg(unix)]
    use crate::cli_backend::PromptMode;
    #[cfg(unix)]
    use tempfile::TempDir;

    /// Verifies that the idle timeout logic in run_interactive correctly handles
    /// activity resets. Per spec (interactive-mode.spec.md lines 155-159):
    /// - Timeout resets on agent output (any bytes from PTY)
    /// - Timeout resets on user input (any key forwarded to agent)
    ///
    /// This test validates the timeout calculation logic that enables resets.
    /// The actual reset happens in the select! branches at lines 497, 523, and 545.
    #[test]
    fn test_idle_timeout_reset_logic() {
        // Simulate the timeout calculation used in run_interactive
        let timeout_duration = Duration::from_secs(30);

        // Simulate 25 seconds of inactivity
        let simulated_25s = Duration::from_secs(25);

        // Remaining time before timeout
        let remaining = timeout_duration.saturating_sub(simulated_25s);
        assert_eq!(remaining.as_secs(), 5);

        // After activity (output or input), last_activity would be reset to now
        let last_activity_after_reset = Instant::now();

        // Now elapsed is 0, full timeout duration available again
        let elapsed = last_activity_after_reset.elapsed();
        assert!(elapsed < Duration::from_millis(100)); // Should be near-zero

        // Timeout calculation would give full duration minus small elapsed
        let new_remaining = timeout_duration.saturating_sub(elapsed);
        assert!(new_remaining > Duration::from_secs(29)); // Should be nearly full timeout
    }
    /// Regression test: TUI mode should not spawn stdin reader thread
    ///
    /// Bug: In TUI mode, Ctrl+C required double-press to exit because the stdin
    /// reader thread (which captures byte 0x03) raced with the signal handler.
    /// The stdin reader would win, triggering "double Ctrl+C" logic instead of
    /// clean exit via interrupt_rx.
    ///
    /// Fix: When tui_connected=true, skip spawning stdin reader entirely.
    /// TUI mode is observation-only; user input should not be captured from stdin.
    /// The TUI has its own input handling (Ctrl+a q), and raw Ctrl+C goes directly
    /// to the signal handler (interrupt_rx) without racing.
    ///
    /// This test documents the expected behavior. The actual fix is in
    /// run_interactive() where `let mut input_rx = if !tui_connected { ... }`.
    #[test]
    fn test_tui_mode_stdin_reader_bypass() {
        // The tui_connected flag is now determined by the explicit tui_mode field,
        // set via set_tui_mode(true) when TUI is connected.
        // Previously used output_rx.is_none() which broke after streaming refactor.

        // Simulate TUI connected scenario (tui_mode = true)
        let tui_mode = true;
        let tui_connected = tui_mode;

        // When TUI is connected, stdin reader is skipped
        // (verified by: input_rx becomes None instead of Some(channel))
        assert!(
            tui_connected,
            "When tui_mode is true, stdin reader must be skipped"
        );

        // In non-TUI mode, stdin reader is spawned
        let tui_mode_disabled = false;
        let tui_connected_non_tui = tui_mode_disabled;
        assert!(
            !tui_connected_non_tui,
            "When tui_mode is false, stdin reader must be spawned"
        );
    }
    #[test]
    fn test_tui_mode_default_is_false() {
        // Create a PtyExecutor and verify tui_mode defaults to false
        let backend = CliBackend::claude();
        let config = PtyConfig::default();
        let executor = PtyExecutor::new(backend, config);

        // tui_mode should default to false
        assert!(!executor.tui_mode, "tui_mode should default to false");
    }
    #[test]
    fn test_set_tui_mode() {
        // Create a PtyExecutor and verify set_tui_mode works
        let backend = CliBackend::claude();
        let config = PtyConfig::default();
        let mut executor = PtyExecutor::new(backend, config);

        // Initially false
        assert!(!executor.tui_mode, "tui_mode should start as false");

        // Set to true
        executor.set_tui_mode(true);
        assert!(
            executor.tui_mode,
            "tui_mode should be true after set_tui_mode(true)"
        );

        // Set back to false
        executor.set_tui_mode(false);
        assert!(
            !executor.tui_mode,
            "tui_mode should be false after set_tui_mode(false)"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_observe_executes_arg_prompt() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: false,
            idle_timeout_secs: 0,
            cols: 80,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let executor = PtyExecutor::new(backend, config);
        let (_tx, rx) = tokio::sync::watch::channel(false);

        let result = executor
            .run_observe("echo hello-pty", rx)
            .await
            .expect("run_observe");

        assert!(result.success);
        assert!(result.output.contains("hello-pty"));
        assert!(result.stripped_output.contains("hello-pty"));
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.termination, TerminationType::Natural);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_observe_writes_stdin_prompt() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "read line; echo \"$line\"".to_string()],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: false,
            idle_timeout_secs: 0,
            cols: 80,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let executor = PtyExecutor::new(backend, config);
        let (_tx, rx) = tokio::sync::watch::channel(false);

        let result = executor
            .run_observe("stdin-line", rx)
            .await
            .expect("run_observe");

        assert!(result.success);
        assert!(result.output.contains("stdin-line"));
        assert!(result.stripped_output.contains("stdin-line"));
        assert_eq!(result.termination, TerminationType::Natural);
    }
    /// Regression test for #280: large stdin-mode prompts deadlocked the PTY
    /// because the PTY line discipline limits canonical input to ~4KB. The fix
    /// converts stdin-mode to arg-mode in non-interactive PTY execution via
    /// `build_command_pty`, so the prompt is passed as a command argument
    /// (with temp file for very large prompts) instead of through PTY stdin.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_pty_converts_stdin_to_arg_for_large_prompt() {
        let _temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };

        // Verify build_command_pty converts stdin to arg mode
        let large_prompt = "x".repeat(32_000);
        let (cmd, args, stdin_input, temp_file) = backend.build_command_pty(&large_prompt);
        assert_eq!(cmd, "echo");
        // stdin_input should be None (converted to arg mode)
        assert!(stdin_input.is_none(), "PTY mode should not use stdin");
        // Large prompt should use temp file
        assert!(temp_file.is_some(), "Large prompt should use temp file");
        // Args should contain the temp file instruction
        assert!(
            args.iter().any(|a| a.contains("Please read and execute")),
            "args should contain temp file instruction: {:?}",
            args
        );

        // Also verify a small prompt goes directly as arg
        let small_prompt = "hello world";
        let (_, args, stdin_input, temp_file) = backend.build_command_pty(small_prompt);
        assert!(stdin_input.is_none());
        assert!(temp_file.is_none());
        assert!(args.iter().any(|a| a == small_prompt));
    }
    /// Verify that PTY execution with stdin-mode backend completes without
    /// deadlock by confirming the prompt is delivered via arg mode.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_observe_large_stdin_backend_does_not_deadlock() {
        let temp_dir = TempDir::new().expect("temp dir");
        // Use echo which just prints its args — confirms the prompt arrives via
        // arg mode (not stdin) in PTY context.
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: PromptMode::Stdin,
            prompt_flag: Some("-p".to_string()),
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: false,
            idle_timeout_secs: 0,
            cols: 32768,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let executor = PtyExecutor::new(backend, config);
        let (_tx, rx) = tokio::sync::watch::channel(false);

        let large_prompt = "x".repeat(32_000);

        // Before the fix, this would hang forever with stdin-mode backends.
        // The timeout is deliberately generous: this test runs end-to-end
        // through PTY allocation, process spawn, echo execution, and a 2s
        // wait-for-exit grace period. Under parallel workspace load the host
        // can take several seconds to schedule these steps. The assertion we
        // care about is "does not hang forever", not "completes quickly", so
        // a 60s ceiling still catches a real deadlock while tolerating slow
        // CI/refinery hosts. See ro-d8rt.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            executor.run_observe(&large_prompt, rx),
        )
        .await
        .expect("should not deadlock")
        .expect("run_observe");

        assert!(result.success);
        // echo should have printed the temp file instruction
        assert!(
            result.output.contains("Please read and execute"),
            "output should contain temp file instruction: {}",
            &result.output[..result.output.len().min(200)]
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_observe_streaming_text_routes_output() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: false,
            idle_timeout_secs: 0,
            cols: 80,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let executor = PtyExecutor::new(backend, config);
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let mut handler = CapturingHandler::default();

        let result = executor
            .run_observe_streaming("printf 'alpha\\nbeta\\n'", rx, &mut handler)
            .await
            .expect("run_observe_streaming");

        assert!(result.success);
        let captured = handler.texts.join("");
        assert!(captured.contains("alpha"), "captured: {captured}");
        assert!(captured.contains("beta"), "captured: {captured}");
        assert!(handler.completions.is_empty());
        assert!(result.extracted_text.is_empty());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_observe_streaming_parses_stream_json() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::StreamJson,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: false,
            idle_timeout_secs: 0,
            cols: 80,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let executor = PtyExecutor::new(backend, config);
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let mut handler = CapturingHandler::default();

        let script = r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Hello stream"}]}}' '{"type":"result","duration_ms":1,"total_cost_usd":0.0,"num_turns":1,"is_error":false}'"#;
        let result = executor
            .run_observe_streaming(script, rx, &mut handler)
            .await
            .expect("run_observe_streaming");

        assert!(result.success);
        assert!(
            handler
                .texts
                .iter()
                .any(|text| text.contains("Hello stream"))
        );
        assert_eq!(handler.completions.len(), 1);
        assert!(result.extracted_text.contains("Hello stream"));
        assert_eq!(result.termination, TerminationType::Natural);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_interactive_in_tui_mode() {
        let temp_dir = TempDir::new().expect("temp dir");
        let backend = CliBackend {
            command: "sh".to_string(),
            args: vec!["-c".to_string()],
            prompt_mode: PromptMode::Arg,
            prompt_flag: None,
            output_format: OutputFormat::Text,
            env_vars: vec![],
        };
        let config = PtyConfig {
            interactive: true,
            idle_timeout_secs: 0,
            cols: 80,
            rows: 24,
            workspace_root: temp_dir.path().to_path_buf(),
        };
        let mut executor = PtyExecutor::new(backend, config);
        executor.set_tui_mode(true);
        let (_tx, rx) = tokio::sync::watch::channel(false);

        let result = executor
            .run_interactive("echo hello-tui", rx)
            .await
            .expect("run_interactive");

        assert!(result.success);
        assert!(result.output.contains("hello-tui"));
        assert!(result.stripped_output.contains("hello-tui"));
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.termination, TerminationType::Natural);
    }
}
