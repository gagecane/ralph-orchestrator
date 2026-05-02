//! Shared test helpers for pty_executor submodules.
//!
//! Compiled only in `#[cfg(test)]`. `CapturingHandler` is exercised by both
//! `parser::tests` (unit-level event dispatch) and `pty_executor::tests`
//! (integration-level streaming runs), so keeping one definition here avoids
//! duplication and keeps the two test surfaces in sync.

use crate::stream_handler::{SessionResult, StreamHandler};

#[derive(Default)]
pub(super) struct CapturingHandler {
    pub(super) texts: Vec<String>,
    pub(super) tool_calls: Vec<(String, String, serde_json::Value)>,
    pub(super) tool_results: Vec<(String, String)>,
    pub(super) errors: Vec<String>,
    pub(super) completions: Vec<SessionResult>,
}

impl StreamHandler for CapturingHandler {
    fn on_text(&mut self, text: &str) {
        self.texts.push(text.to_string());
    }

    fn on_tool_call(&mut self, name: &str, id: &str, input: &serde_json::Value) {
        self.tool_calls
            .push((name.to_string(), id.to_string(), input.clone()));
    }

    fn on_tool_result(&mut self, id: &str, output: &str) {
        self.tool_results.push((id.to_string(), output.to_string()));
    }

    fn on_error(&mut self, error: &str) {
        self.errors.push(error.to_string());
    }

    fn on_complete(&mut self, result: &SessionResult) {
        self.completions.push(result.clone());
    }
}
