//! Core `CliExecutor` orchestration.
//!
//! Handles spawning backend processes, streaming their output, applying
//! inactivity and post-event timeouts, and collecting the final result.

use crate::cli_backend::{CliBackend, OutputFormat};
use crate::copilot_stream::CopilotStreamParser;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tracing::{debug, warn};

use super::env::inject_ralph_runtime_env;
use super::stream::{
    StreamEvent, StreamKind, join_error_to_io, line_signals_event_emitted, read_stream,
};

/// Once a backend emits an event marker, we give it this long to exit
/// before force-terminating. Keeps orphaned agents from lingering.
const POST_EVENT_GRACE_TIMEOUT: Duration = Duration::from_secs(5);

/// After sending SIGTERM, wait this long for the child to exit before
/// escalating to SIGKILL.
const TERMINATION_GRACE_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of a CLI execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// The full output from the CLI.
    pub output: String,
    /// Whether the execution succeeded (exit code 0).
    pub success: bool,
    /// The exit code.
    pub exit_code: Option<i32>,
    /// Whether the execution was terminated due to timeout.
    pub timed_out: bool,
}

/// Executor for running prompts through CLI backends.
#[derive(Debug)]
pub struct CliExecutor {
    backend: CliBackend,
}

impl CliExecutor {
    /// Creates a new executor with the given backend.
    pub fn new(backend: CliBackend) -> Self {
        Self { backend }
    }

    /// Executes a prompt and streams output to the provided writer.
    ///
    /// Output is streamed line-by-line to the writer while being accumulated
    /// for the return value. If `timeout` is provided and the execution produces
    /// no stdout/stderr activity for longer than that duration, the process
    /// receives SIGTERM and the result indicates timeout.
    ///
    /// When `verbose` is true, stderr output is also written to the output writer
    /// with a `[stderr]` prefix. When false, stderr is captured but not displayed.
    pub async fn execute<W: Write + Send>(
        &self,
        prompt: &str,
        mut output_writer: W,
        timeout: Option<Duration>,
        verbose: bool,
    ) -> std::io::Result<ExecutionResult> {
        // Note: _temp_file is kept alive for the duration of this function scope.
        // Some Arg-mode backends use temp-file indirection for very large prompts.
        let (cmd, args, stdin_input, _temp_file) = self.backend.build_command(prompt, false);

        let mut command = Command::new(&cmd);
        command.args(&args);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);

        // Set working directory to current directory (mirrors PTY executor behavior)
        // Use fallback to "." if current_dir fails (e.g., E2E test workspaces)
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        command.current_dir(&cwd);
        inject_ralph_runtime_env(&mut command, &cwd);

        // Apply backend-specific environment variables (e.g., Agent Teams env var)
        command.envs(self.backend.env_vars.iter().map(|(k, v)| (k, v)));

        debug!(
            command = %cmd,
            args = ?args,
            cwd = ?cwd,
            "Spawning CLI command"
        );

        if stdin_input.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command.spawn()?;

        // Write to stdin if needed. Some short-lived commands can exit before
        // consuming stdin, which surfaces as BrokenPipe. Treat that as benign
        // and continue collecting output/exit status from the child.
        if let Some(input) = stdin_input
            && let Some(mut stdin) = child.stdin.take()
        {
            if let Err(err) = stdin.write_all(input.as_bytes()).await
                && err.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(err);
            }
            drop(stdin); // Close stdin to signal EOF
        }

        let mut timed_out = false;
        let mut post_event_deadline: Option<tokio::time::Instant> = None;
        let mut terminated_status = None;

        // Take both stdout and stderr handles upfront to read concurrently.
        // Each emitted line resets the inactivity timeout.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);

        let stdout_task = stdout_handle.map(|stdout| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stdout, tx, StreamKind::Stdout).await })
        });
        let stderr_task = stderr_handle.map(|stderr| {
            let tx = event_tx.clone();
            tokio::spawn(async move { read_stream(stderr, tx, StreamKind::Stderr).await })
        });
        drop(event_tx);

        let mut stdout_done = stdout_task.is_none();
        let mut stderr_done = stderr_task.is_none();
        let mut accumulated_output = String::new();

        if let Some(duration) = timeout {
            debug!(
                timeout_secs = duration.as_secs(),
                "Executing with inactivity timeout"
            );
        }

        while !stdout_done || !stderr_done {
            let now = tokio::time::Instant::now();
            let effective_timeout = match (timeout, post_event_deadline) {
                (_, Some(deadline)) if deadline <= now => Some(Duration::ZERO),
                (Some(duration), Some(deadline)) => {
                    Some(duration.min(deadline.saturating_duration_since(now)))
                }
                (None, Some(deadline)) => Some(deadline.saturating_duration_since(now)),
                (Some(duration), None) => Some(duration),
                (None, None) => None,
            };

            let next_event = match effective_timeout {
                Some(duration) => match tokio::time::timeout(duration, event_rx.recv()).await {
                    Ok(event) => event,
                    Err(_) => {
                        warn!(
                            timeout_secs = duration.as_secs(),
                            "Execution inactivity timeout reached, sending SIGTERM"
                        );
                        timed_out = true;
                        terminated_status = Some(Self::terminate_child_and_wait(&mut child).await?);
                        break;
                    }
                },
                None => event_rx.recv().await,
            };

            match next_event {
                Some(StreamEvent::StdoutLine(line)) => {
                    if line_signals_event_emitted(&line) {
                        post_event_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + POST_EVENT_GRACE_TIMEOUT
                        });
                    }
                    if self.backend.output_format == OutputFormat::CopilotStreamJson {
                        if let Some(text) = CopilotStreamParser::extract_text(&line) {
                            write!(output_writer, "{text}")?;
                            if !text.ends_with('\n') {
                                writeln!(output_writer)?;
                            }
                        }
                    } else {
                        writeln!(output_writer, "{line}")?;
                    }
                    output_writer.flush()?;
                    accumulated_output.push_str(&line);
                    accumulated_output.push('\n');
                }
                Some(StreamEvent::StderrLine(line)) => {
                    if line_signals_event_emitted(&line) {
                        post_event_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + POST_EVENT_GRACE_TIMEOUT
                        });
                    }
                    if verbose {
                        writeln!(output_writer, "[stderr] {line}")?;
                        output_writer.flush()?;
                    }
                    accumulated_output.push_str("[stderr] ");
                    accumulated_output.push_str(&line);
                    accumulated_output.push('\n');
                }
                Some(StreamEvent::StdoutEof) => stdout_done = true,
                Some(StreamEvent::StderrEof) => stderr_done = true,
                None => {
                    stdout_done = true;
                    stderr_done = true;
                }
            }
        }

        let status = if let Some(status) = terminated_status {
            status
        } else {
            child.wait().await?
        };

        if let Some(handle) = stdout_task {
            handle.await.map_err(join_error_to_io)??;
        }
        if let Some(handle) = stderr_task {
            handle.await.map_err(join_error_to_io)??;
        }

        Ok(ExecutionResult {
            output: accumulated_output,
            success: status.success() && !timed_out,
            exit_code: status.code(),
            timed_out,
        })
    }

    /// Terminates the child process with SIGTERM, then SIGKILL if it ignores graceful shutdown.
    async fn terminate_child_and_wait(
        child: &mut Child,
    ) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(not(unix))]
        {
            child.start_kill()?;
            return child.wait().await;
        }

        #[cfg(unix)]
        if let Some(pid) = child.id() {
            #[allow(clippy::cast_possible_wrap)]
            let pid = Pid::from_raw(pid as i32);
            let pgid = Pid::from_raw(-pid.as_raw());
            debug!(%pid, "Sending SIGTERM to child process group");
            let _ = kill(pgid, Signal::SIGTERM);
            match tokio::time::timeout(TERMINATION_GRACE_TIMEOUT, child.wait()).await {
                Ok(status) => status,
                Err(_) => {
                    warn!(%pid, "Child process ignored SIGTERM, sending SIGKILL");
                    let _ = kill(pgid, Signal::SIGKILL);
                    child.wait().await
                }
            }
        } else {
            child.wait().await
        }
    }

    /// Executes a prompt without streaming (captures all output).
    ///
    /// Uses no timeout by default. For timed execution, use `execute_capture_with_timeout`.
    pub async fn execute_capture(&self, prompt: &str) -> std::io::Result<ExecutionResult> {
        self.execute_capture_with_timeout(prompt, None).await
    }

    /// Executes a prompt without streaming, with optional timeout.
    pub async fn execute_capture_with_timeout(
        &self,
        prompt: &str,
        timeout: Option<Duration>,
    ) -> std::io::Result<ExecutionResult> {
        // Use a sink that discards output for non-streaming execution
        // verbose=false since output is being discarded anyway
        let sink = std::io::sink();
        self.execute(prompt, sink, timeout, false).await
    }
}
