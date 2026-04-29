//! Tracing/logging initialization for the `ralph` binary.
//!
//! Different execution modes need different sinks and layers:
//! - TUI mode writes to a rotating log file (stdout is owned by ratatui).
//! - RPC / MCP modes write to stderr so stdout stays clean for JSON frames.
//! - Normal mode writes to stdout.
//!
//! All modes optionally add the diagnostics trace layer when `RALPH_DIAGNOSTICS=1`
//! is set and the current subcommand is eligible for diagnostics.

use ralph_core::diagnostics::{DiagnosticTraceLayer, DiagnosticsCollector, create_log_file};
use tracing_subscriber::prelude::*;

/// Selects which sink tracing output is routed to.
pub struct LoggingConfig {
    /// True when the TUI owns the terminal (logs must go to a file).
    pub tui_enabled: bool,
    /// True when stdout is reserved for RPC JSON frames.
    pub rpc_enabled: bool,
    /// True when stdout is reserved for MCP protocol traffic.
    pub mcp_enabled: bool,
    /// True when diagnostics capture is active for this invocation.
    pub diagnostics_enabled: bool,
    /// Tracing filter directive (for example `"info"` or `"debug"`).
    pub filter: &'static str,
}

/// Initialize the tracing subscriber for the selected mode.
///
/// Note: this installs a global subscriber and must be called at most once
/// per process. Subsequent calls will silently no-op via tracing's `try_init`.
pub fn init(cfg: &LoggingConfig) {
    if cfg.tui_enabled {
        init_tui(cfg);
    } else if cfg.rpc_enabled || cfg.mcp_enabled {
        init_stderr(cfg);
    } else {
        init_stdout(cfg);
    }
}

/// TUI mode: logs go to a rotating file (stdout belongs to ratatui).
fn init_tui(cfg: &LoggingConfig) {
    let Ok((file, _log_path)) = create_log_file(std::path::Path::new(".")) else {
        // If log file creation fails, silently continue without logging.
        return;
    };

    if cfg.diagnostics_enabled {
        if let Ok(collector) = DiagnosticsCollector::new(std::path::Path::new("."))
            && let Some(session_dir) = collector.session_dir()
        {
            if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                tracing_subscriber::registry()
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(std::sync::Mutex::new(file))
                            .with_ansi(false),
                    )
                    .with(tracing_subscriber::EnvFilter::new(cfg.filter))
                    .with(trace_layer)
                    .init();
            } else {
                tracing_subscriber::fmt()
                    .with_env_filter(cfg.filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
            }
        }
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(cfg.filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    }
}

/// RPC / MCP modes: stdout is reserved for protocol, so logs go to stderr.
fn init_stderr(cfg: &LoggingConfig) {
    tracing_subscriber::fmt()
        .with_env_filter(cfg.filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Normal CLI mode: logs go to stdout.
fn init_stdout(cfg: &LoggingConfig) {
    if cfg.diagnostics_enabled {
        if let Ok(collector) = DiagnosticsCollector::new(std::path::Path::new("."))
            && let Some(session_dir) = collector.session_dir()
        {
            if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                tracing_subscriber::registry()
                    .with(tracing_subscriber::fmt::layer())
                    .with(tracing_subscriber::EnvFilter::new(cfg.filter))
                    .with(trace_layer)
                    .init();
            } else {
                // Fallback: just stdout
                tracing_subscriber::fmt().with_env_filter(cfg.filter).init();
            }
        } else {
            // Fallback: just stdout
            tracing_subscriber::fmt().with_env_filter(cfg.filter).init();
        }
    } else {
        // Normal mode without diagnostics: just stdout
        tracing_subscriber::fmt().with_env_filter(cfg.filter).init();
    }
}
