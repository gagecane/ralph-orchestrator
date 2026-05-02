//! PTY I/O event types and helpers.
//!
//! Defines the event enums used by the input/output channels that bridge
//! blocking PTY I/O threads to the async runtime.

/// Input events from the user.
#[derive(Debug)]
pub(super) enum InputEvent {
    /// Ctrl+C pressed.
    CtrlC,
    /// Ctrl+\ pressed.
    CtrlBackslash,
    /// Regular data to forward.
    Data(Vec<u8>),
}

impl InputEvent {
    /// Creates an `InputEvent` from raw bytes.
    pub(super) fn from_bytes(data: Vec<u8>) -> Self {
        if data.len() == 1 {
            match data[0] {
                3 => return InputEvent::CtrlC,
                28 => return InputEvent::CtrlBackslash,
                _ => {}
            }
        }
        InputEvent::Data(data)
    }
}

/// Output events from the PTY.
#[derive(Debug)]
pub(super) enum OutputEvent {
    /// Data received from PTY.
    Data(Vec<u8>),
    /// PTY reached EOF (process exited).
    Eof,
    /// Error reading from PTY.
    Error(String),
}

/// Strips ANSI escape sequences from raw bytes.
///
/// Uses `strip-ansi-escapes` for direct byte-level ANSI removal without terminal
/// emulation. This ensures ALL content is preserved regardless of output size,
/// unlike vt100's terminal simulation which can lose content that scrolls off.
pub(super) fn strip_ansi(bytes: &[u8]) -> String {
    let stripped = strip_ansi_escapes::strip(bytes);
    String::from_utf8_lossy(&stripped).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_event_from_bytes_ctrl_c() {
        let event = InputEvent::from_bytes(vec![3]);
        assert!(matches!(event, InputEvent::CtrlC));
    }

    #[test]
    fn test_input_event_from_bytes_ctrl_backslash() {
        let event = InputEvent::from_bytes(vec![28]);
        assert!(matches!(event, InputEvent::CtrlBackslash));
    }

    #[test]
    fn test_input_event_from_bytes_data() {
        let event = InputEvent::from_bytes(vec![b'a']);
        assert!(matches!(event, InputEvent::Data(_)));

        let event = InputEvent::from_bytes(vec![1, 2, 3]);
        assert!(matches!(event, InputEvent::Data(_)));
    }

    #[test]
    fn test_strip_ansi_basic() {
        let input = b"\x1b[1;36m  Thinking...\x1b[0m\r\n";
        let stripped = strip_ansi(input);
        assert!(stripped.contains("Thinking..."));
        assert!(!stripped.contains("\x1b["));
    }

    #[test]
    fn test_completion_promise_extraction() {
        // Simulate Claude output with heavy ANSI formatting
        let input = b"\x1b[1;36m  Thinking...\x1b[0m\r\n\
                      \x1b[2K\x1b[1;32m  Done!\x1b[0m\r\n\
                      \x1b[33mLOOP_COMPLETE\x1b[0m\r\n";

        let stripped = strip_ansi(input);

        // Event parser sees clean text
        assert!(stripped.contains("LOOP_COMPLETE"));
        assert!(!stripped.contains("\x1b["));
    }

    #[test]
    fn test_event_tag_extraction() {
        // Event tags may be wrapped in ANSI codes
        let input = b"\x1b[90m<event topic=\"build.done\">\x1b[0m\r\n\
                      Task completed successfully\r\n\
                      \x1b[90m</event>\x1b[0m\r\n";

        let stripped = strip_ansi(input);

        assert!(stripped.contains("<event topic=\"build.done\">"));
        assert!(stripped.contains("</event>"));
    }

    #[test]
    fn test_large_output_preserves_early_events() {
        // Regression test: ensure event tags aren't lost when output is large
        let mut input = Vec::new();

        // Event tag at the beginning
        input.extend_from_slice(b"<event topic=\"build.task\">Implement feature X</event>\r\n");

        // Simulate 500 lines of verbose output (would overflow any terminal)
        for i in 0..500 {
            input.extend_from_slice(format!("Line {}: Processing step {}...\r\n", i, i).as_bytes());
        }

        let stripped = strip_ansi(&input);

        // Event tag should still be present - no scrollback loss with strip-ansi-escapes
        assert!(
            stripped.contains("<event topic=\"build.task\">"),
            "Event tag was lost - strip_ansi is not preserving all content"
        );
        assert!(stripped.contains("Implement feature X"));
        assert!(stripped.contains("Line 499")); // Last line should be present too
    }
}
