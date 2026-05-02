//! Double Ctrl+C detection state machine.
//!
//! Tracks the first Ctrl+C press so a second Ctrl+C within a short window
//! terminates the child process instead of being forwarded again.

use std::time::{Duration, Instant};

/// State machine for double Ctrl+C detection.
#[derive(Debug)]
pub struct CtrlCState {
    /// When the first Ctrl+C was pressed (if any).
    first_press: Option<Instant>,
    /// Window duration for double-press detection.
    window: Duration,
}

/// Action to take after handling Ctrl+C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtrlCAction {
    /// Forward the Ctrl+C to Claude and start/restart the window.
    ForwardAndStartWindow,
    /// Terminate Claude (second Ctrl+C within window).
    Terminate,
}

impl CtrlCState {
    /// Creates a new Ctrl+C state tracker.
    pub fn new() -> Self {
        Self {
            first_press: None,
            window: Duration::from_secs(1),
        }
    }

    /// Handles a Ctrl+C keypress and returns the action to take.
    pub fn handle_ctrl_c(&mut self, now: Instant) -> CtrlCAction {
        match self.first_press {
            Some(first) if now.duration_since(first) < self.window => {
                // Second Ctrl+C within window - terminate
                self.first_press = None;
                CtrlCAction::Terminate
            }
            _ => {
                // First Ctrl+C or window expired - forward and start window
                self.first_press = Some(now);
                CtrlCAction::ForwardAndStartWindow
            }
        }
    }
}

impl Default for CtrlCState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_double_ctrl_c_within_window() {
        let mut state = CtrlCState::new();
        let now = Instant::now();

        // First Ctrl+C: should forward and start window
        let action = state.handle_ctrl_c(now);
        assert_eq!(action, CtrlCAction::ForwardAndStartWindow);

        // Second Ctrl+C within 1 second: should terminate
        let later = now + Duration::from_millis(500);
        let action = state.handle_ctrl_c(later);
        assert_eq!(action, CtrlCAction::Terminate);
    }

    #[test]
    fn test_ctrl_c_window_expires() {
        let mut state = CtrlCState::new();
        let now = Instant::now();

        // First Ctrl+C
        state.handle_ctrl_c(now);

        // Wait 2 seconds (window expires)
        let later = now + Duration::from_secs(2);

        // Second Ctrl+C: window expired, should forward and start new window
        let action = state.handle_ctrl_c(later);
        assert_eq!(action, CtrlCAction::ForwardAndStartWindow);
    }
}
