//! Child-process cleanup and graceful termination helpers.
//!
//! These functions are platform-aware wrappers around the
//! `portable_pty::Child` trait for sending SIGTERM/SIGKILL (Unix) or killing
//! directly (non-Unix), and for waiting on the child with an interruptible
//! timeout.

use std::io;
use std::time::{Duration, Instant};
use tracing::debug;

#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;

/// Terminates the child process, optionally gracefully.
///
/// On non-Unix platforms this always issues a hard kill because we have no
/// portable signal primitive. On Unix, a `graceful` termination sends SIGTERM
/// first and waits up to 2 seconds for the child to exit, falling back to
/// SIGKILL if the grace period expires.
#[allow(clippy::unused_async)] // Kept async to preserve signature parity with Unix implementation
#[cfg(not(unix))]
pub(super) async fn terminate_child(
    child: &mut Box<dyn portable_pty::Child + Send>,
    _graceful: bool,
) -> io::Result<()> {
    child.kill()
}

/// Terminates the child process, optionally gracefully (Unix).
///
/// See the non-Unix variant for the overall contract.
#[cfg(unix)]
pub(super) async fn terminate_child(
    child: &mut Box<dyn portable_pty::Child + Send>,
    graceful: bool,
) -> io::Result<()> {
    let pid = match child.process_id() {
        Some(id) => Pid::from_raw(id as i32),
        None => return Ok(()), // Already exited
    };

    if graceful {
        debug!(pid = %pid, "Sending SIGTERM");
        let _ = kill(pid, Signal::SIGTERM);

        // Wait up to 2 seconds for graceful exit.
        let grace_period = Duration::from_secs(2);
        let start = Instant::now();

        while start.elapsed() < grace_period {
            if child
                .try_wait()
                .map_err(|e| io::Error::other(e.to_string()))?
                .is_some()
            {
                return Ok(());
            }
            // Use async sleep to avoid blocking the tokio runtime
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Still running after grace period - force kill
        debug!(pid = %pid, "Grace period expired, sending SIGKILL");
    }

    debug!(pid = %pid, "Sending SIGKILL");
    let _ = kill(pid, Signal::SIGKILL);
    Ok(())
}

/// Waits for the child process to exit, optionally with a timeout.
///
/// This is interruptible by the shared interrupt channel from the event loop.
/// When interrupted, returns `Ok(None)` to let the caller handle termination.
pub(super) async fn wait_for_exit(
    child: &mut Box<dyn portable_pty::Child + Send>,
    max_wait: Option<Duration>,
    interrupt_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> io::Result<Option<portable_pty::ExitStatus>> {
    let start = Instant::now();

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| io::Error::other(e.to_string()))?
        {
            return Ok(Some(status));
        }

        if let Some(max) = max_wait
            && start.elapsed() >= max
        {
            return Ok(None);
        }

        tokio::select! {
            _ = interrupt_rx.changed() => {
                if *interrupt_rx.borrow() {
                    debug!("Interrupt received while waiting for child exit");
                    return Ok(None);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}
