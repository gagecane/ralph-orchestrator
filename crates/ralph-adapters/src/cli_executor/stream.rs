//! Stream event types and line-reader helpers for the CLI executor.
//!
//! These helpers decouple the orchestration logic in `executor.rs` from the
//! mechanics of reading stdout/stderr line-by-line and forwarding them as
//! events on an mpsc channel.

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Events emitted as the child process writes to its stdout/stderr pipes.
///
/// A separate EOF variant is needed because the executor needs to know when
/// each stream is finished independently so it can stop polling once both
/// are drained.
pub(super) enum StreamEvent {
    StdoutLine(String),
    StderrLine(String),
    StdoutEof,
    StderrEof,
}

/// Identifies which of the two child streams a reader task is attached to.
pub(super) enum StreamKind {
    Stdout,
    Stderr,
}

/// Returns true when a line indicates the backend emitted a Ralph event.
///
/// When this happens the executor switches from the inactivity timeout to a
/// short fixed post-event grace period so orphaned backends don't linger.
pub(super) fn line_signals_event_emitted(line: &str) -> bool {
    line.contains("Event emitted:")
}

/// Reads `stream` line-by-line and forwards each line (plus a final EOF event)
/// on `tx`.
///
/// Returns early without error if the receiver has been dropped — this is the
/// normal path when the executor has already broken out of its poll loop (for
/// example after an inactivity timeout).
pub(super) async fn read_stream<R>(
    stream: R,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
    stream_kind: StreamKind,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let event = match stream_kind {
            StreamKind::Stdout => StreamEvent::StdoutLine(line),
            StreamKind::Stderr => StreamEvent::StderrLine(line),
        };
        if tx.send(event).await.is_err() {
            return Ok(());
        }
    }

    let eof_event = match stream_kind {
        StreamKind::Stdout => StreamEvent::StdoutEof,
        StreamKind::Stderr => StreamEvent::StderrEof,
    };
    let _ = tx.send(eof_event).await;
    Ok(())
}

/// Converts a tokio `JoinError` into an `io::Error` so stream-reader task
/// failures can be surfaced through the executor's `io::Result` return type.
pub(super) fn join_error_to_io(error: tokio::task::JoinError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}
