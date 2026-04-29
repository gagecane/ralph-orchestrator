//! CLI-facing enum types: color, verbosity, and output format.
//!
//! These are the small value types used throughout the CLI for formatting
//! and output control. They are small and widely referenced, so they live
//! in their own module to keep `main.rs` focused on dispatch and wiring.

use clap::ValueEnum;
use std::io::{IsTerminal, stdout};

/// Color output mode for terminal display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ColorMode {
    /// Automatically detect if stdout is a TTY
    #[default]
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
}

impl ColorMode {
    /// Returns true if colors should be used based on mode and terminal detection.
    pub fn should_use_colors(self) -> bool {
        // NO_COLOR is a de-facto cross-tooling convention and should disable ANSI
        // colors by default, regardless of output mode.
        if std::env::var("NO_COLOR").is_ok() {
            return false;
        }

        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => stdout().is_terminal(),
        }
    }
}

/// Verbosity level for streaming output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// Suppress all streaming output (for CI/scripting)
    Quiet,
    /// Show assistant text and tool invocations (default)
    #[default]
    Normal,
    /// Show everything including tool results and session summary
    Verbose,
}

impl Verbosity {
    /// Resolves verbosity from CLI args, env vars, and config.
    ///
    /// Precedence (highest to lowest):
    /// 1. CLI flags: `--verbose`/`-v` or `--quiet`/`-q`
    /// 2. Environment variables: `RALPH_VERBOSE=1` or `RALPH_QUIET=1`
    /// 3. Config file: (if supported in future)
    /// 4. Default: Normal
    pub fn resolve(cli_verbose: bool, cli_quiet: bool) -> Self {
        let env_quiet = std::env::var("RALPH_QUIET").is_ok();
        let env_verbose = std::env::var("RALPH_VERBOSE").is_ok();
        Self::resolve_with_env(cli_verbose, cli_quiet, env_quiet, env_verbose)
    }

    #[allow(clippy::fn_params_excessive_bools)]
    pub(crate) fn resolve_with_env(
        cli_verbose: bool,
        cli_quiet: bool,
        env_quiet: bool,
        env_verbose: bool,
    ) -> Self {
        // CLI flags take precedence
        if cli_quiet {
            return Verbosity::Quiet;
        }
        if cli_verbose {
            return Verbosity::Verbose;
        }

        // Environment variables
        if env_quiet {
            return Verbosity::Quiet;
        }
        if env_verbose {
            return Verbosity::Verbose;
        }

        Verbosity::Normal
    }
}

/// Output format for events command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verbosity_cli_quiet() {
        assert_eq!(Verbosity::resolve(false, true), Verbosity::Quiet);
    }

    #[test]
    fn test_verbosity_cli_verbose() {
        assert_eq!(Verbosity::resolve(true, false), Verbosity::Verbose);
    }

    #[test]
    fn test_verbosity_default() {
        assert_eq!(
            Verbosity::resolve_with_env(false, false, false, false),
            Verbosity::Normal
        );
    }

    #[test]
    fn test_verbosity_env_quiet() {
        assert_eq!(
            Verbosity::resolve_with_env(false, false, true, false),
            Verbosity::Quiet
        );
    }

    #[test]
    fn test_verbosity_env_verbose() {
        assert_eq!(
            Verbosity::resolve_with_env(false, false, false, true),
            Verbosity::Verbose
        );
    }

    #[test]
    fn test_color_mode_should_use_colors() {
        // `NO_COLOR` disables ANSI globally, including `--color always`.
        let expected_always = std::env::var("NO_COLOR").is_err();
        assert_eq!(ColorMode::Always.should_use_colors(), expected_always);
        assert!(!ColorMode::Never.should_use_colors());
    }
}
