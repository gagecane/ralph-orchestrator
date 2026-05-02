//! PTY executor configuration and result types.

/// Result of a PTY execution.
#[derive(Debug)]
pub struct PtyExecutionResult {
    /// The accumulated output (ANSI sequences preserved).
    pub output: String,
    /// The ANSI-stripped output for event parsing.
    pub stripped_output: String,
    /// Extracted text content from NDJSON stream (for Claude's stream-json output).
    /// When Claude outputs `--output-format stream-json`, event tags like
    /// `<event topic="...">` are inside JSON string values. This field contains
    /// the extracted text content for proper event parsing.
    /// Empty for non-JSON backends (use `stripped_output` instead).
    pub extracted_text: String,
    /// Whether the process exited successfully.
    pub success: bool,
    /// The exit code if available.
    pub exit_code: Option<i32>,
    /// How the process was terminated.
    pub termination: TerminationType,
    /// Total session cost in USD, if available from stream metadata.
    pub total_cost_usd: f64,
    /// Total input tokens in the session.
    pub input_tokens: u64,
    /// Total output tokens in the session.
    pub output_tokens: u64,
    /// Total cache-read tokens in the session.
    pub cache_read_tokens: u64,
    /// Total cache-write tokens in the session.
    pub cache_write_tokens: u64,
}

/// How the PTY process was terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationType {
    /// Process exited naturally.
    Natural,
    /// Terminated due to idle timeout.
    IdleTimeout,
    /// Terminated by user (double Ctrl+C).
    UserInterrupt,
    /// Force killed by user (Ctrl+\).
    ForceKill,
}

/// Configuration for PTY execution.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Enable interactive mode (forward user input).
    pub interactive: bool,
    /// Idle timeout in seconds (0 = disabled).
    pub idle_timeout_secs: u32,
    /// Terminal width.
    pub cols: u16,
    /// Terminal height.
    pub rows: u16,
    /// Workspace root directory for command execution.
    /// This is captured at startup to avoid `current_dir()` failures when the
    /// working directory no longer exists (e.g., in E2E test workspaces).
    pub workspace_root: std::path::PathBuf,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            interactive: true,
            idle_timeout_secs: 30,
            cols: 80,
            rows: 24,
            workspace_root: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
        }
    }
}

impl PtyConfig {
    /// Creates config from environment, falling back to defaults.
    pub fn from_env() -> Self {
        let cols = std::env::var("COLUMNS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(80);
        let rows = std::env::var("LINES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);

        Self {
            cols,
            rows,
            ..Default::default()
        }
    }

    /// Sets the workspace root directory.
    pub fn with_workspace_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.workspace_root = root.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pty_config_defaults() {
        let config = PtyConfig::default();
        assert!(config.interactive);
        assert_eq!(config.idle_timeout_secs, 30);
        assert_eq!(config.cols, 80);
        assert_eq!(config.rows, 24);
    }

    #[test]
    fn test_pty_config_from_env_matches_env_or_defaults() {
        let cols = std::env::var("COLUMNS")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(80);
        let rows = std::env::var("LINES")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(24);

        let config = PtyConfig::from_env();
        assert_eq!(config.cols, cols);
        assert_eq!(config.rows, rows);
    }

    #[test]
    fn test_extracted_text_field_exists() {
        // Test that PtyExecutionResult has extracted_text field
        // This is for NDJSON output where event tags are inside JSON strings
        let result = PtyExecutionResult {
            output: String::new(),
            stripped_output: String::new(),
            extracted_text: String::from("<event topic=\"build.done\">Test</event>"),
            success: true,
            exit_code: Some(0),
            termination: TerminationType::Natural,
            total_cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        assert!(
            result
                .extracted_text
                .contains("<event topic=\"build.done\">")
        );
    }
}
