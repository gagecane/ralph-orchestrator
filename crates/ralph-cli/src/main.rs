//! # ralph-cli
//!
//! Binary entry point for the Ralph Orchestrator.
//!
//! This crate provides:
//! - CLI argument parsing using `clap`
//! - Application initialization and configuration
//! - Entry point to the headless orchestration loop
//! - Event history viewing via `ralph events`
//! - Project initialization via `ralph init`
//! - SOP-based planning via `ralph plan`
//! - Code task generation via `ralph code-task`
//! - Hook config validation via `ralph hooks validate`
//! - Work item tracking via `ralph task`

mod backend_support;
mod bot;
mod cli_types;
mod commands;
mod config_resolution;
mod config_source;
mod display;
mod doctor;
mod hats;
mod hooks;
mod init;
mod interact;
mod loop_runner;
mod loops;
mod mcp;
mod memory;
mod preflight;
mod presets;
mod rpc_stdin;
mod skill_cli;
mod sop_runner;
mod task_cli;
#[cfg(test)]
mod test_support;
mod tools;
mod wave;
mod web;
mod workspace;

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use ralph_adapters::detect_backend;
use ralph_core::{
    CheckStatus, LockError, LoopContext, LoopEntry, LoopLock, LoopRegistry,
    PreflightReport, PreflightRunner, RalphConfig, TerminationReason,
    truncate_with_ellipsis,
    worktree::{WorktreeConfig, create_worktree, ensure_gitignore, remove_worktree},
};
use std::io::IsTerminal;
use std::path::PathBuf;
use tracing::{debug, info, warn};

// Unix-specific process management for process group leadership
#[cfg(unix)]
mod process_management {
    use nix::unistd::{Pid, getpgrp, setpgid, tcgetpgrp};
    use std::io::{IsTerminal, stdin, stdout};
    use tracing::debug;

    /// Sets up process group leadership.
    ///
    /// Per spec: "The orchestrator must run as a process group leader. All spawned
    /// CLI processes (Claude, Kiro, etc.) belong to this group. On termination,
    /// the entire process group receives the signal, preventing orphans."
    pub fn setup_process_group() {
        // Make ourselves the process group leader when safe.
        // If we're launched by a wrapper (e.g., `npx`), moving to a new process
        // group can drop us out of the foreground TTY group and break TUI input.
        let pid = Pid::this();
        let pgrp = getpgrp();
        if pgrp == pid {
            debug!("Already process group leader: PID {}", pid);
            return;
        }

        if is_foreground_tty_group(pgrp) {
            debug!(
                "Skipping setpgid: keeping foreground process group {}",
                pgrp
            );
            return;
        }

        if let Err(e) = setpgid(pid, pid) {
            // EPERM is OK - we're already a process group leader (e.g., started from shell)
            if e != nix::errno::Errno::EPERM {
                debug!(
                    "Note: Could not set process group ({}), continuing anyway",
                    e
                );
            }
        }
        debug!("Process group initialized: PID {}", pid);
    }

    fn is_foreground_tty_group(current_pgrp: Pid) -> bool {
        // Prefer stdin for foreground checks, fall back to stdout.
        if stdin().is_terminal()
            && let Ok(fg) = tcgetpgrp(stdin())
        {
            return fg == current_pgrp;
        }

        if stdout().is_terminal()
            && let Ok(fg) = tcgetpgrp(stdout())
        {
            return fg == current_pgrp;
        }

        false
    }
}

#[cfg(not(unix))]
mod process_management {
    /// No-op on non-Unix platforms.
    pub fn setup_process_group() {}
}

/// Installs a panic hook that restores terminal state before printing panic info.
///
/// When a TUI application panics, the terminal can be left in a broken state:
/// - Raw mode enabled (input not line-buffered)
/// - Alternate screen buffer active (no scrollback)
/// - Cursor hidden
///
/// This hook ensures the terminal is restored so the panic message is visible
/// and the user can scroll/interact normally.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Restore terminal state before printing panic info
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        // Call the default panic hook to print the panic message
        default_hook(panic_info);
    }));
}

/// Color output mode for terminal display.
pub use cli_types::ColorMode;

pub(crate) use workspace::{
    default_config_path, resolve_marker_target, resolve_path_from_workspace,
    resolve_workspace_root, urgent_steer_path_from_workspace,
};

/// Verbosity level for streaming output.
pub use cli_types::Verbosity;

/// Output format for events command.
pub use cli_types::OutputFormat;

// Re-export colors and truncate from display module for use in this file
use display::colors;
use display::truncate;

pub use config_source::{ConfigSource, HatsSource};
pub(crate) use config_source::{
    apply_config_overrides, ensure_scratchpad_directory, load_config_with_overrides,
};

/// Ralph Orchestrator - Multi-agent orchestration framework
#[derive(Parser, Debug)]
#[command(name = "ralph", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // ─────────────────────────────────────────────────────────────────────────
    // Global options (available for all subcommands)
    // ─────────────────────────────────────────────────────────────────────────
    /// Core configuration source: file path, URL, or core.field=value override.
    /// Can be specified multiple times. Overrides are applied after core config loading.
    /// If not set, defaults to `ralph.yml` or `$RALPH_CONFIG`.
    #[arg(short, long, global = true, action = ArgAction::Append)]
    config: Vec<String>,

    /// Hat collection source: file path, builtin:name, or URL.
    ///
    /// Example: `-H builtin:code-assist` or `-H .ralph/hats/my-workflow.yml`
    #[arg(short = 'H', long, global = true)]
    hats: Option<String>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Color output mode (auto, always, never)
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    color: ColorMode,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the orchestration loop (default if no subcommand given)
    Run(RunArgs),

    /// Run preflight checks to validate configuration and environment
    Preflight(preflight::PreflightArgs),

    /// Validate hooks configuration and command wiring
    Hooks(hooks::HooksArgs),

    /// Run first-run diagnostics and environment checks
    Doctor(doctor::DoctorArgs),

    /// Interactive walkthrough of hats, hat collections, and workflow
    Tutorial(commands::tutorial::TutorialArgs),

    /// DEPRECATED: Use `ralph run --continue` instead.
    /// Resume a previously interrupted loop from existing scratchpad.
    #[command(hide = true)]
    Resume(ResumeArgs),

    /// View event history for debugging
    Events(commands::events::EventsArgs),

    /// Initialize a new ralph.yml configuration file
    Init(commands::init::InitArgs),

    /// Clean up Ralph artifacts from `.ralph/agent`.
    Clean(commands::clean::CleanArgs),

    /// Emit an event to the current run's events file with proper JSON formatting
    Emit(commands::emit::EmitArgs),

    /// Start a Prompt-Driven Development planning session
    Plan(commands::plan::PlanArgs),

    /// Generate code task files from descriptions or plans
    CodeTask(commands::plan::CodeTaskArgs),

    /// Legacy alias for `code-task` (runtime tasks are `ralph tools task`).
    #[command(hide = true)]
    Task(commands::plan::CodeTaskArgs),

    /// Ralph's runtime tools (agent-facing)
    Tools(tools::ToolsArgs),

    /// Dispatch wave events for parallel hat execution
    Wave(wave::WaveArgs),

    /// Manage parallel loops
    Loops(loops::LoopsArgs),

    /// Manage configured hats
    Hats(hats::HatsArgs),

    /// Attach a TUI to a running ralph-api server
    Tui(TuiArgs),

    /// Run the web dashboard
    Web(web::WebArgs),

    /// Run Ralph as an MCP server over stdio
    Mcp(mcp::McpArgs),

    /// Manage Telegram bot setup and testing
    Bot(bot::BotArgs),

    /// Generate shell completions
    Completions(commands::completions::CompletionsArgs),
}

/// Arguments for the run subcommand.
#[derive(Parser, Debug)]
struct RunArgs {
    /// Inline prompt text (mutually exclusive with -P/--prompt-file)
    #[arg(short = 'p', long = "prompt", conflicts_with = "prompt_file")]
    prompt_text: Option<String>,

    /// Override backend from config (cli > config > auto-detect)
    #[arg(short = 'b', long = "backend", value_name = "BACKEND")]
    backend: Option<String>,

    /// Prompt file path (mutually exclusive with -p/--prompt)
    #[arg(short = 'P', long = "prompt-file", conflicts_with = "prompt_text")]
    prompt_file: Option<PathBuf>,

    /// Override max iterations
    #[arg(long)]
    max_iterations: Option<u32>,

    /// Override completion promise
    #[arg(long)]
    completion_promise: Option<String>,

    /// Dry run - show what would be executed without running
    #[arg(long)]
    dry_run: bool,

    /// Continue from existing scratchpad (resume interrupted loop).
    /// Use this when a previous run was interrupted and you want to
    /// continue from where it left off.
    #[arg(long = "continue")]
    continue_mode: bool,

    /// Explicit loop ID to use with --continue.
    /// Reuses tasks from the specified loop instead of generating a new ID.
    /// If omitted with --continue, reuses the existing current-loop-id marker.
    #[arg(long, requires = "continue_mode")]
    loop_id: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Execution Mode Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    no_tui: bool,

    /// Force autonomous mode (headless, non-interactive).
    /// Overrides default_mode from config.
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    /// All output is valid JSON; input accepts RpcCommand messages.
    /// Use this for IDE integrations and machine-readable interfaces.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    rpc: bool,

    /// Use legacy in-process TUI mode instead of subprocess RPC mode.
    /// This is an escape hatch during the migration to subprocess TUI.
    #[arg(long, hide = true, conflicts_with = "rpc", conflicts_with = "no_tui")]
    legacy_tui: bool,

    /// Idle timeout in seconds for interactive mode (default: 30).
    /// Process is terminated after this many seconds of inactivity.
    /// Set to 0 to disable idle timeout.
    #[arg(long)]
    idle_timeout: Option<u32>,

    // ─────────────────────────────────────────────────────────────────────────
    // Multi-Loop Concurrency Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Wait for the primary loop slot instead of spawning into a worktree.
    /// Use this when you want to ensure only one loop runs at a time.
    #[arg(long)]
    exclusive: bool,

    /// Skip automatic merge after loop completes (keep worktree for manual handling).
    /// Only relevant for parallel loops running in worktrees.
    #[arg(long)]
    no_auto_merge: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Preflight Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Skip preflight checks before loop start.
    /// Overrides features.preflight.enabled from config.
    #[arg(long)]
    skip_preflight: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Verbosity Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    record_session: Option<PathBuf>,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    custom_args: Vec<String>,
}

/// Arguments for the resume subcommand.
///
/// Per spec: "When loop terminates due to safeguard (not completion promise),
/// user can run `ralph resume` to restart reading existing scratchpad."
#[derive(Parser, Debug)]
struct ResumeArgs {
    /// Override max iterations (from current position)
    #[arg(long)]
    max_iterations: Option<u32>,

    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    no_tui: bool,

    /// Force autonomous mode
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    rpc: bool,

    /// Idle timeout in seconds for TUI mode
    #[arg(long)]
    idle_timeout: Option<u32>,

    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    record_session: Option<PathBuf>,
}

/// Arguments for the `ralph tui` subcommand.
#[derive(Parser, Debug)]
struct TuiArgs {
    /// ralph-api server URL to connect to.
    /// Defaults to RALPH_API_URL env var, or http://127.0.0.1:3000.
    #[arg(short = 'u', long = "url")]
    url: Option<String>,
}

async fn tui_command(args: TuiArgs) -> Result<()> {
    use ralph_tui::Tui;

    let url = args
        .url
        .or_else(|| std::env::var("RALPH_API_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

    info!(url = %url, "Attaching TUI to ralph-api server");

    let tui =
        Tui::connect(&url).with_context(|| format!("Failed to create TUI client for {url}"))?;

    tui.run().await.context("TUI exited with error")
}

/// Returns true if the given command is eligible for diagnostics session creation.
/// Only `run` and `resume` commands (and the default no-subcommand case) should
/// create diagnostics session directories. Other subcommands like `emit` or `tools`
/// would otherwise create empty session dirs.
fn is_diagnostics_eligible_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::Run(_) | Commands::Resume(_)) | None)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install panic hook to restore terminal state on crash
    // This prevents the terminal from being left in raw mode or alternate screen
    install_panic_hook();

    let cli = Cli::parse();

    // Detect if TUI mode is requested - TUI owns the terminal, so logs must not go to stdout
    // TUI is enabled by default unless --no-tui, --autonomous, or --rpc is specified
    // RPC mode also suppresses stdout logging (JSON-only output)
    let tui_enabled = match &cli.command {
        Some(Commands::Run(args)) => !args.no_tui && !args.autonomous && !args.rpc,
        Some(Commands::Resume(args)) => !args.no_tui && !args.autonomous && !args.rpc,
        None => true,
        _ => false,
    };
    let rpc_enabled = match &cli.command {
        Some(Commands::Run(args)) => args.rpc,
        Some(Commands::Resume(args)) => args.rpc,
        _ => false,
    };
    let mcp_enabled = matches!(&cli.command, Some(Commands::Mcp(_)));

    // Initialize logging - suppress in TUI mode to avoid corrupting the display
    let filter = if cli.verbose { "debug" } else { "info" };

    // Check if diagnostics are enabled
    let diagnostics_enabled = is_diagnostics_eligible_command(cli.command.as_ref())
        && std::env::var("RALPH_DIAGNOSTICS")
            .map(|v| v == "1")
            .unwrap_or(false);

    if tui_enabled {
        // TUI mode: logs would corrupt the display, so write to a rotating log file
        if let Ok((file, _log_path)) =
            ralph_core::diagnostics::create_log_file(std::path::Path::new("."))
        {
            if diagnostics_enabled {
                use ralph_core::diagnostics::DiagnosticTraceLayer;
                use tracing_subscriber::prelude::*;

                if let Ok(collector) =
                    ralph_core::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
                    && let Some(session_dir) = collector.session_dir()
                {
                    if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                        tracing_subscriber::registry()
                            .with(
                                tracing_subscriber::fmt::layer()
                                    .with_writer(std::sync::Mutex::new(file))
                                    .with_ansi(false),
                            )
                            .with(tracing_subscriber::EnvFilter::new(filter))
                            .with(trace_layer)
                            .init();
                    } else {
                        tracing_subscriber::fmt()
                            .with_env_filter(filter)
                            .with_writer(std::sync::Mutex::new(file))
                            .with_ansi(false)
                            .init();
                    }
                }
            } else {
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
            }
        }
        // If log file creation fails, silently continue without logging
    } else if rpc_enabled || mcp_enabled {
        // RPC/MCP mode: logs must go to stderr to keep stdout clean for protocol messages
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        // Normal mode: logs go to stdout
        if diagnostics_enabled {
            // Normal mode + diagnostics: stdout + trace layer
            use ralph_core::diagnostics::DiagnosticTraceLayer;
            use tracing_subscriber::prelude::*;

            if let Ok(collector) =
                ralph_core::diagnostics::DiagnosticsCollector::new(std::path::Path::new("."))
                && let Some(session_dir) = collector.session_dir()
            {
                if let Ok(trace_layer) = DiagnosticTraceLayer::new(session_dir) {
                    tracing_subscriber::registry()
                        .with(tracing_subscriber::fmt::layer())
                        .with(tracing_subscriber::EnvFilter::new(filter))
                        .with(trace_layer)
                        .init();
                } else {
                    // Fallback: just stdout
                    tracing_subscriber::fmt().with_env_filter(filter).init();
                }
            } else {
                // Fallback: just stdout
                tracing_subscriber::fmt().with_env_filter(filter).init();
            }
        } else {
            // Normal mode without diagnostics: just stdout
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    // Parse all config sources from CLI
    let config_values: Vec<String> = if cli.config.is_empty() {
        vec![default_config_path().to_string_lossy().to_string()]
    } else {
        cli.config.clone()
    };

    let config_sources: Vec<ConfigSource> = config_values
        .iter()
        .map(|s| ConfigSource::parse(s))
        .collect();
    let hats_source = cli.hats.as_deref().map(HatsSource::parse);

    match cli.command {
        Some(Commands::Run(args)) => {
            run_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
            )
            .await
        }
        Some(Commands::Preflight(args)) => {
            preflight::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Hooks(args)) => {
            hooks::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Doctor(args)) => {
            doctor::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Tutorial(args)) => commands::tutorial::run(cli.color, args),
        Some(Commands::Resume(args)) => {
            resume_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
            )
            .await
        }
        Some(Commands::Events(args)) => commands::events::run(cli.color, args),
        Some(Commands::Init(args)) => commands::init::run(cli.color, args),
        Some(Commands::Clean(args)) => commands::clean::run(&config_sources, cli.color, args),
        Some(Commands::Emit(args)) => commands::emit::run(cli.color, args),
        Some(Commands::Plan(args)) => {
            commands::plan::run_plan(&config_sources, hats_source.as_ref(), cli.color, args).await
        }
        Some(Commands::CodeTask(args)) => {
            commands::plan::run_code_task(&config_sources, hats_source.as_ref(), cli.color, args)
                .await
        }
        Some(Commands::Task(args)) => {
            commands::plan::run_code_task(&config_sources, hats_source.as_ref(), cli.color, args)
                .await
        }
        Some(Commands::Tools(args)) => tools::execute(args, cli.color.should_use_colors()).await,
        Some(Commands::Wave(args)) => wave::execute(args, cli.color.should_use_colors()),
        Some(Commands::Loops(args)) => loops::execute(args, cli.color.should_use_colors()),
        Some(Commands::Hats(args)) => {
            hats::execute(
                &config_sources,
                hats_source.as_ref(),
                args,
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Tui(args)) => tui_command(args).await,
        Some(Commands::Web(args)) => web::execute(args).await,
        Some(Commands::Mcp(args)) => mcp::execute(args).await,
        Some(Commands::Bot(args)) => {
            bot::execute(
                args,
                &config_sources,
                hats_source.as_ref(),
                cli.color.should_use_colors(),
            )
            .await
        }
        Some(Commands::Completions(args)) => commands::completions::run(args),
        None => {
            // Default to run with TUI enabled (new default behavior)
            let args = RunArgs {
                prompt_text: None,
                prompt_file: None,
                backend: None,
                max_iterations: None,
                completion_promise: None,
                dry_run: false,
                continue_mode: false,
                loop_id: None,
                no_tui: false, // TUI enabled by default
                autonomous: false,
                rpc: false,
                legacy_tui: false,
                idle_timeout: None,
                exclusive: false,
                no_auto_merge: false,
                skip_preflight: false,
                verbose: false,
                quiet: false,
                record_session: None,
                custom_args: Vec::new(),
            };
            run_command(
                &config_sources,
                hats_source.as_ref(),
                cli.verbose,
                cli.color,
                args,
            )
            .await
        }
    }
}

fn format_preflight_summary(report: &PreflightReport) -> String {
    let icons: Vec<String> = report
        .checks
        .iter()
        .map(|check| {
            let icon = match check.status {
                CheckStatus::Pass => "✓",
                CheckStatus::Warn => "⚠",
                CheckStatus::Fail => "✗",
            };
            format!("{icon} {}", check.name)
        })
        .collect();

    let summary = if icons.is_empty() {
        "no checks".to_string()
    } else {
        icons.join(" ")
    };

    let suffix = if report.failures > 0 {
        format!(
            " ({} failure{})",
            report.failures,
            if report.failures == 1 { "" } else { "s" }
        )
    } else if report.warnings > 0 {
        format!(
            " ({} warning{})",
            report.warnings,
            if report.warnings == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };

    format!("{summary}{suffix}")
}

enum AutoPreflightMode {
    DryRun,
    Run,
}

fn preflight_failure_detail(report: &PreflightReport, strict: bool) -> String {
    if strict && report.warnings > 0 {
        format!(
            "{} failure{}, {} warning{}",
            report.failures,
            if report.failures == 1 { "" } else { "s" },
            report.warnings,
            if report.warnings == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} failure{}",
            report.failures,
            if report.failures == 1 { "" } else { "s" }
        )
    }
}

async fn run_auto_preflight(
    config: &RalphConfig,
    skip_preflight: bool,
    verbose: bool,
    mode: AutoPreflightMode,
) -> Result<Option<PreflightReport>> {
    if skip_preflight || !config.features.preflight.enabled {
        return Ok(None);
    }

    let runner = PreflightRunner::default_checks();
    let mut report = if config.features.preflight.skip.is_empty() {
        runner.run_all(config).await
    } else {
        let skip_lower: std::collections::HashSet<String> = config
            .features
            .preflight
            .skip
            .iter()
            .map(|name| name.to_lowercase())
            .collect();
        let selected: Vec<String> = runner
            .check_names()
            .into_iter()
            .filter(|name| !skip_lower.contains(&name.to_lowercase()))
            .map(|name| name.to_string())
            .collect();
        runner.run_selected(config, &selected).await
    };

    let effective_passed = if config.features.preflight.strict {
        report.failures == 0 && report.warnings == 0
    } else {
        report.failures == 0
    };
    report.passed = effective_passed;

    match mode {
        AutoPreflightMode::DryRun => Ok(Some(report)),
        AutoPreflightMode::Run => {
            print_preflight_summary(&report, verbose, "Preflight: ", false);
            if !effective_passed {
                let detail = preflight_failure_detail(&report, config.features.preflight.strict);
                anyhow::bail!(
                    "Preflight checks failed ({}). Fix the issues above or use --skip-preflight to bypass.",
                    detail
                );
            }
            Ok(None)
        }
    }
}

fn print_preflight_summary(
    report: &PreflightReport,
    verbose: bool,
    prefix: &str,
    use_stdout: bool,
) {
    let summary = format_preflight_summary(report);
    if use_stdout {
        println!("{prefix}{summary}");
    } else {
        eprintln!("{prefix}{summary}");
    }

    let emit = |line: String| {
        if use_stdout {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };

    for check in &report.checks {
        if check.status == CheckStatus::Fail
            && let Some(message) = &check.message
        {
            emit(format!("  ✗ {}: {}", check.name, message));
        }
    }

    if verbose {
        for check in &report.checks {
            if check.status == CheckStatus::Warn
                && let Some(message) = &check.message
            {
                emit(format!("  ⚠ {}: {}", check.name, message));
            }
        }
    }
}

async fn run_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    verbose: bool,
    color_mode: ColorMode,
    args: RunArgs,
) -> Result<()> {
    let mut config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Handle --continue mode: check scratchpad exists before proceeding
    let resume = args.continue_mode;
    if resume {
        let scratchpad_path = std::path::Path::new(&config.core.scratchpad.path);
        if !scratchpad_path.exists() {
            anyhow::bail!(
                "Cannot continue: scratchpad not found at '{}'. \
                 Start a fresh run with `ralph run`.",
                config.core.scratchpad.path
            );
        }
        info!(
            "Found existing scratchpad at '{}', continuing from previous state",
            config.core.scratchpad.path
        );
    }

    // Capture args for subprocess TUI mode BEFORE fields are consumed below
    let subprocess_tui_args = SubprocessTuiArgs::new(&args, config_sources, hats_source);

    // Apply CLI overrides (after normalization so they take final precedence)
    // Per spec: CLI -p and -P are mutually exclusive (enforced by clap)
    if let Some(text) = args.prompt_text {
        config.event_loop.prompt = Some(text);
        config.event_loop.prompt_file = String::new(); // Clear file path
    } else if let Some(path) = args.prompt_file {
        config.event_loop.prompt_file = path.to_string_lossy().to_string();
        config.event_loop.prompt = None; // Clear inline
    }
    if let Some(max_iter) = args.max_iterations {
        config.event_loop.max_iterations = max_iter;
    }
    if let Some(promise) = args.completion_promise {
        config.event_loop.completion_promise = promise;
    }
    if verbose {
        config.verbose = true;
    }

    // Apply execution mode overrides per spec
    // TUI is enabled by default (unless --no-tui is specified)
    if args.autonomous {
        config.cli.default_mode = "autonomous".to_string();
    } else if !args.no_tui {
        config.cli.default_mode = "interactive".to_string();
    }

    // Override idle timeout if specified
    if let Some(timeout) = args.idle_timeout {
        config.cli.idle_timeout_secs = timeout;
    }

    // Apply backend override from CLI (takes precedence over config)
    if let Some(backend) = args.backend {
        config.cli.backend = backend;
    }

    // Validate configuration and emit warnings
    let warnings = config
        .validate()
        .context("Configuration validation failed")?;
    for warning in &warnings {
        eprintln!("{warning}");
    }

    // Handle auto-detection if backend is "auto"
    if config.cli.backend == "auto" {
        let priority = config.get_agent_priority();
        let detected = detect_backend(&priority, |backend| {
            config.adapter_settings(backend).enabled
        });

        match detected {
            Ok(backend) => {
                info!("Auto-detected backend: {}", backend);
                config.cli.backend = backend;
            }
            Err(e) => {
                eprintln!("{e}");
                return Err(anyhow::Error::new(e));
            }
        }
    }

    let preflight_verbose = verbose || args.verbose;

    if args.dry_run {
        let preflight_report = run_auto_preflight(
            &config,
            args.skip_preflight,
            preflight_verbose,
            AutoPreflightMode::DryRun,
        )
        .await?;
        println!("Dry run mode - configuration:");
        println!(
            "  Hats: {}",
            if config.hats.is_empty() {
                "planner, builder (default)".to_string()
            } else {
                config.hats.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        );

        // Show prompt source
        if let Some(ref inline) = config.event_loop.prompt {
            let preview = truncate_with_ellipsis(&inline.replace('\n', " "), 60);
            println!("  Prompt: inline text ({})", preview);
        } else {
            println!("  Prompt file: {}", config.event_loop.prompt_file);
        }

        println!(
            "  Completion promise: {}",
            config.event_loop.completion_promise
        );
        println!("  Max iterations: {}", config.event_loop.max_iterations);
        println!("  Max runtime: {}s", config.event_loop.max_runtime_seconds);
        println!(
            "  Scratchpad: {} (enabled: {})",
            config.core.scratchpad.path, config.core.scratchpad.enabled
        );
        println!("  Specs dir: {}", config.core.specs_dir);
        println!("  Backend: {}", config.cli.backend);
        println!("  Verbose: {}", config.verbose);
        // Execution mode info
        println!("  Default mode: {}", config.cli.default_mode);
        if config.cli.default_mode == "interactive" {
            println!("  Idle timeout: {}s", config.cli.idle_timeout_secs);
        }
        if !warnings.is_empty() {
            println!("  Warnings: {}", warnings.len());
        }
        if let Some(report) = preflight_report.as_ref() {
            print_preflight_summary(report, preflight_verbose, "  Preflight: ", true);
        }
        return Ok(());
    }

    // Ensure scratchpad directory exists (auto-create with depth limit)
    // This is done after dry-run check to avoid creating directories during dry-run
    ensure_scratchpad_directory(&config)?;

    // Get the prompt for lock metadata (short version for display)
    // When prompt_file is used, read its content for the summary instead of showing the file path
    let prompt_summary = config
        .event_loop
        .prompt
        .clone()
        .or_else(|| {
            let prompt_file = &config.event_loop.prompt_file;
            if prompt_file.is_empty() {
                None
            } else {
                let path = std::path::Path::new(prompt_file);
                if path.exists() {
                    std::fs::read_to_string(path).ok()
                } else {
                    None
                }
            }
        })
        .map(|p| truncate(&p, 100))
        .unwrap_or_else(|| "[no prompt]".to_string());

    let mut pending_worktree_registration: Option<LoopEntry> = None;

    // Determine TUI mode early (before lock acquisition) to avoid self-lock contention
    // in subprocess TUI mode. The child RPC process will acquire the lock itself.
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let use_subprocess_tui =
        !args.no_tui && !args.autonomous && !args.rpc && !args.legacy_tui && is_tty;

    // Try to acquire the loop lock for multi-loop concurrency support
    // This implements the lock detection flow from the multi-loop spec
    // Skip lock acquisition in subprocess TUI mode - let the child acquire it
    let workspace_root = &config.core.workspace_root;
    let (loop_context, _lock_guard) = if use_subprocess_tui {
        // In subprocess TUI mode, don't acquire lock here - the child RPC process will do it
        // This avoids the self-lock contention where parent holds lock and child sees it,
        // then incorrectly spawns a worktree thinking there's another concurrent loop
        debug!("Skipping lock acquisition in subprocess TUI mode (child will acquire)");
        let context = LoopContext::primary(workspace_root.clone());
        (context, None)
    } else {
        match LoopLock::try_acquire(workspace_root, &prompt_summary) {
            Ok(guard) => {
                // We're the primary loop - run in place
                debug!("Acquired loop lock, running as primary loop");
                let context = LoopContext::primary(workspace_root.clone());
                (context, Some(guard))
            }
            Err(LockError::AlreadyLocked(existing)) => {
                // Another loop is running
                if args.exclusive {
                    // --exclusive: wait for the lock instead of spawning worktree
                    info!(
                        "Loop lock held by PID {} (started {}), waiting for lock (--exclusive mode)...",
                        existing.pid, existing.started
                    );
                    let guard = LoopLock::acquire_blocking(workspace_root, &prompt_summary)
                        .context("Failed to acquire loop lock in exclusive mode")?;
                    debug!("Acquired loop lock after waiting");
                    let context = LoopContext::primary(workspace_root.clone());
                    (context, Some(guard))
                } else if !config.features.parallel {
                    // Parallel loops disabled via config - error out
                    anyhow::bail!(
                        "Another loop is already running (PID {}, prompt: \"{}\"). \
                    Parallel loops are disabled in config (features.parallel: false). \
                    Use --exclusive to wait for the lock, or enable parallel loops.",
                        existing.pid,
                        existing.prompt.chars().take(50).collect::<String>()
                    );
                } else {
                    // Auto-spawn into worktree
                    info!(
                        "Loop lock held by PID {} ({}), spawning parallel loop in worktree",
                        existing.pid,
                        existing.prompt.chars().take(50).collect::<String>()
                    );

                    let worktree_config = WorktreeConfig::default();

                    // Generate memorable loop ID (adjective-noun only, no prompt keywords)
                    // This ID will be used consistently for: registry ID, worktree path, and branch name
                    let name_generator =
                        ralph_core::LoopNameGenerator::from_config(&config.features.loop_naming);
                    let loop_id = name_generator.generate_memorable_unique(|name| {
                        ralph_core::worktree_exists(workspace_root, name, &worktree_config)
                    });

                    // Ensure worktree directory is in .gitignore
                    ensure_gitignore(workspace_root, ".worktrees")
                        .context("Failed to update .gitignore for worktrees")?;

                    // Create the worktree
                    let worktree = create_worktree(workspace_root, &loop_id, &worktree_config)
                        .context("Failed to create worktree for parallel loop")?;

                    info!(
                        "Created worktree at {} on branch {}",
                        worktree.path.display(),
                        worktree.branch
                    );

                    // Create loop context for the worktree
                    let context = LoopContext::worktree(
                        loop_id.clone(),
                        worktree.path.clone(),
                        workspace_root.clone(),
                    );

                    // Set up all worktree symlinks (memories, specs, code tasks)
                    context
                        .setup_worktree_symlinks()
                        .context("Failed to create symlinks in worktree")?;

                    // Generate context file with worktree metadata
                    context
                        .generate_context_file(&worktree.branch, &prompt_summary)
                        .context("Failed to generate context file in worktree")?;

                    // Register this loop after preflight succeeds so failed runs
                    // don't leave stale registry entries behind.
                    let entry = LoopEntry::with_id(
                        &loop_id,
                        &prompt_summary,
                        Some(worktree.path.to_string_lossy().to_string()),
                        worktree.path.to_string_lossy().to_string(),
                    );
                    pending_worktree_registration = Some(entry);

                    // Update config to use worktree paths
                    // The scratchpad and other paths should resolve to the worktree
                    // Note: We keep the lock guard as None since worktree loops don't hold the primary lock

                    (context, None)
                }
            }
            Err(LockError::UnsupportedPlatform) => {
                // Non-Unix: just run without locking (single-loop fallback)
                warn!("Loop locking not supported on this platform, running without lock");
                let context = LoopContext::primary(workspace_root.clone());
                (context, None)
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context("Failed to acquire loop lock"));
            }
        }
    };

    // Update workspace_root in config if running in worktree
    if !loop_context.is_primary() {
        config.core.workspace_root = loop_context.workspace().to_path_buf();
        // Also update scratchpad path to use worktree location
        config.core.scratchpad.path = loop_context.scratchpad_path().to_string_lossy().to_string();
        debug!(
            "Running in worktree: workspace={}, scratchpad={}",
            config.core.workspace_root.display(),
            config.core.scratchpad.path
        );
    }

    // Ensure directories exist in the loop context
    loop_context
        .ensure_directories()
        .context("Failed to create loop directories")?;

    if let Err(err) = run_auto_preflight(
        &config,
        args.skip_preflight,
        preflight_verbose,
        AutoPreflightMode::Run,
    )
    .await
    {
        if !loop_context.is_primary()
            && let Err(clean_err) =
                remove_worktree(loop_context.repo_root(), loop_context.workspace())
        {
            warn!(
                "Preflight failed; unable to remove worktree {}: {}",
                loop_context.workspace().display(),
                clean_err
            );
        }
        return Err(err);
    }

    if let Some(entry) = pending_worktree_registration {
        let registry = LoopRegistry::new(loop_context.repo_root());
        registry
            .register(entry)
            .context("Failed to register loop in registry")?;
    }

    // Run the orchestration loop and exit with proper exit code
    // TUI is enabled by default (unless --no-tui, --autonomous, or --rpc is specified)
    let wants_tui = !args.no_tui && !args.autonomous && !args.rpc;
    let use_legacy_tui = args.legacy_tui;
    let enable_rpc = args.rpc;
    let verbosity = Verbosity::resolve(verbose || args.verbose, args.quiet);
    let custom_args = args.custom_args.clone();
    // --no-auto-merge CLI flag overrides config.features.auto_merge
    let auto_merge_override = if args.no_auto_merge {
        Some(false)
    } else {
        None
    };
    let workspace_root = config.core.workspace_root.clone();

    // Determine TUI mode:
    // 1. Subprocess TUI (default): TUI spawns `ralph run --rpc` as child, reads JSON events
    // 2. Legacy TUI: In-process TUI (--legacy-tui escape hatch)
    // 3. RPC mode: Headless JSON-lines output (--rpc)
    // 4. CLI mode: No TUI (--no-tui or --autonomous)
    // Note: use_subprocess_tui is now determined earlier (before lock acquisition)
    let reason = if use_subprocess_tui {
        // Subprocess TUI mode: spawn child with --rpc and attach TUI
        run_subprocess_tui(subprocess_tui_args, resume, custom_args).await?
    } else {
        // In-process mode: run_loop_impl handles everything
        let enable_tui = wants_tui && use_legacy_tui;
        loop_runner::run_loop_impl(
            config,
            color_mode,
            resume,
            enable_tui,
            enable_rpc,
            verbosity,
            args.record_session,
            Some(loop_context),
            custom_args,
            auto_merge_override,
            args.loop_id,
        )
        .await?
    };

    // Handle restart: run required single-command restart sequence.
    if matches!(reason, TerminationReason::RestartRequested) {
        clear_restart_request_signal(&workspace_root);

        #[cfg(unix)]
        {
            let restart_cmd = required_restart_command(std::process::id());
            info!(
                "Restart requested — launching single-command restart: {}",
                restart_cmd
            );

            std::process::Command::new("sh")
                .arg("-lc")
                .arg(&restart_cmd)
                .spawn()
                .with_context(|| format!("Failed to spawn restart command: {}", restart_cmd))?;

            // Shell command takes over restarting this loop after kill.
            return Ok(());
        }

        #[cfg(not(unix))]
        {
            anyhow::bail!("Restart via single-command shell restart is only supported on Unix");
        }
    }

    let exit_code = reason.exit_code();

    // Use explicit exit for non-zero codes to ensure proper exit status
    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn required_restart_command(pid: u32) -> String {
    format!("kill {pid} && RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- resume -c ralph.test.yml")
}

fn clear_restart_request_signal(workspace_root: &std::path::Path) {
    let restart_path = workspace_root.join(".ralph/restart-requested");
    let _ = std::fs::remove_file(&restart_path);
}

/// Arguments needed for subprocess TUI mode.
/// We clone these early before RunArgs fields are consumed.
#[derive(Clone)]
struct SubprocessTuiArgs {
    prompt_text: Option<String>,
    prompt_file: Option<PathBuf>,
    backend: Option<String>,
    max_iterations: Option<u32>,
    completion_promise: Option<String>,
    continue_mode: bool,
    loop_id: Option<String>,
    idle_timeout: Option<u32>,
    verbose: bool,
    quiet: bool,
    record_session: Option<PathBuf>,
    exclusive: bool,
    no_auto_merge: bool,
    skip_preflight: bool,
    /// Config sources to forward to child process (-c args)
    config_sources: Vec<String>,
    /// Hats source to forward to child process (-H arg)
    hats_source: Option<String>,
}

impl SubprocessTuiArgs {
    /// Create from RunArgs with config/hats sources from Cli.
    fn new(
        args: &RunArgs,
        config_sources: &[ConfigSource],
        hats_source: Option<&HatsSource>,
    ) -> Self {
        Self {
            prompt_text: args.prompt_text.clone(),
            prompt_file: args.prompt_file.clone(),
            backend: args.backend.clone(),
            max_iterations: args.max_iterations,
            completion_promise: args.completion_promise.clone(),
            continue_mode: args.continue_mode,
            loop_id: args.loop_id.clone(),
            idle_timeout: args.idle_timeout,
            verbose: args.verbose,
            quiet: args.quiet,
            record_session: args.record_session.clone(),
            exclusive: args.exclusive,
            no_auto_merge: args.no_auto_merge,
            skip_preflight: args.skip_preflight,
            config_sources: config_sources.iter().map(|s| s.to_cli_string()).collect(),
            hats_source: hats_source.map(|h| h.label()),
        }
    }
}

/// Run the orchestration loop as a subprocess with TUI attached.
///
/// This spawns `ralph run --rpc` as a child process and attaches the TUI
/// as a client that reads JSON events from stdout and sends commands to stdin.
/// This two-process model allows the TUI to be decoupled from the orchestration loop.
async fn run_subprocess_tui(
    args: SubprocessTuiArgs,
    resume: bool,
    custom_args: Vec<String>,
) -> Result<TerminationReason> {
    use std::process::Stdio;
    use tokio::process::Command;

    // Build child command: ralph [-c ...] [-H ...] run --rpc <forwarded args>
    // Note: -c and -H are global options that must come BEFORE the subcommand
    let mut child_args = Vec::new();

    // Forward config sources (global option, before subcommand)
    for config_source in &args.config_sources {
        child_args.push("-c".to_string());
        child_args.push(config_source.clone());
    }

    // Forward hats source (global option, before subcommand)
    if let Some(ref hats) = args.hats_source {
        child_args.push("-H".to_string());
        child_args.push(hats.clone());
    }

    // Add subcommand and mode
    child_args.push("run".to_string());
    child_args.push("--rpc".to_string());

    // Forward prompt
    if let Some(ref prompt) = args.prompt_text {
        child_args.push("-p".to_string());
        child_args.push(prompt.clone());
    }
    if let Some(ref prompt_file) = args.prompt_file {
        child_args.push("-P".to_string());
        child_args.push(prompt_file.to_string_lossy().to_string());
    }

    // Forward backend
    if let Some(ref backend) = args.backend {
        child_args.push("-b".to_string());
        child_args.push(backend.clone());
    }

    // Forward max iterations
    if let Some(max_iters) = args.max_iterations {
        child_args.push("--max-iterations".to_string());
        child_args.push(max_iters.to_string());
    }

    // Forward completion promise
    if let Some(ref promise) = args.completion_promise {
        child_args.push("--completion-promise".to_string());
        child_args.push(promise.clone());
    }

    // Forward continue mode and loop ID
    if resume || args.continue_mode {
        child_args.push("--continue".to_string());
    }
    if let Some(ref loop_id) = args.loop_id {
        child_args.push("--loop-id".to_string());
        child_args.push(loop_id.clone());
    }

    // Forward idle timeout
    if let Some(timeout) = args.idle_timeout {
        child_args.push("--idle-timeout".to_string());
        child_args.push(timeout.to_string());
    }

    // Forward verbosity
    if args.verbose {
        child_args.push("-v".to_string());
    }
    if args.quiet {
        child_args.push("-q".to_string());
    }

    // Forward record session
    if let Some(ref path) = args.record_session {
        child_args.push("--record-session".to_string());
        child_args.push(path.to_string_lossy().to_string());
    }

    // Forward multi-loop options
    if args.exclusive {
        child_args.push("--exclusive".to_string());
    }
    if args.no_auto_merge {
        child_args.push("--no-auto-merge".to_string());
    }

    // Forward preflight options
    if args.skip_preflight {
        child_args.push("--skip-preflight".to_string());
    }

    // Forward custom args (after --)
    if !custom_args.is_empty() {
        child_args.push("--".to_string());
        child_args.extend(custom_args);
    }

    info!(child_args = ?child_args, "Spawning subprocess for TUI mode");

    // Spawn child process.
    // Redirect stderr to a log file to prevent child tracing output from
    // corrupting the TUI display (ratatui runs in raw terminal mode).
    let stderr_stdio = match ralph_core::diagnostics::create_log_file(
        &std::env::current_dir().unwrap_or_default(),
    ) {
        Ok((file, path)) => {
            info!(log_file = %path.display(), "TUI subprocess stderr redirected to log file");
            Stdio::from(file)
        }
        Err(_) => Stdio::null(),
    };

    let mut child = Command::new(std::env::current_exe()?)
        .args(&child_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_stdio)
        .spawn()
        .context("Failed to spawn ralph subprocess for TUI")?;

    let stdin = child
        .stdin
        .take()
        .context("Failed to capture subprocess stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to capture subprocess stdout")?;

    // Create TUI state and start event reader
    let state = std::sync::Arc::new(std::sync::Mutex::new(ralph_tui::TuiState::new()));
    let (terminated_tx, terminated_rx) = tokio::sync::watch::channel(false);

    // Create RPC writer for sending commands
    let rpc_writer = ralph_tui::RpcWriter::new(stdin);

    // Spawn the event reader as a background task
    let reader_state = std::sync::Arc::clone(&state);
    let cancel_rx = terminated_rx.clone();
    let reader_handle = tokio::spawn(async move {
        ralph_tui::run_rpc_event_reader(stdout, reader_state, cancel_rx).await;
    });

    info!("TUI running in subprocess RPC mode");

    // Run the TUI render/input loop with subprocess support
    let app = ralph_tui::App::new_subprocess(
        std::sync::Arc::clone(&state),
        terminated_rx,
        rpc_writer.clone(),
    );
    let tui_result = app.run().await;

    // Signal cancellation
    let _ = terminated_tx.send(true);

    // Send abort to subprocess and close stdin
    let _ = rpc_writer.send_abort().await;
    let _ = rpc_writer.close().await;

    // Wait for reader to finish
    let _ = reader_handle.await;

    // Wait for subprocess to exit and get exit status
    let exit_status = child.wait().await?;

    // Map exit status to termination reason
    // Exit codes: 0=success, 1=max_iterations, 130=interrupted (SIGINT)
    let reason = if exit_status.success() {
        TerminationReason::CompletionPromise
    } else {
        match exit_status.code() {
            Some(1) => TerminationReason::MaxIterations,
            Some(130) => TerminationReason::Interrupted,
            _ => TerminationReason::Stopped,
        }
    };

    // Return TUI result if it failed, otherwise the termination reason
    tui_result.map(|_| reason)
}

/// Resume a previously interrupted loop from existing scratchpad.
///
/// DEPRECATED: Use `ralph run --continue` instead.
///
/// Per spec: "When loop terminates due to safeguard (not completion promise),
/// user can run `ralph run --continue` to restart reading existing scratchpad,
/// continuing from where it left off."
async fn resume_command(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    verbose: bool,
    color_mode: ColorMode,
    args: ResumeArgs,
) -> Result<()> {
    // Show deprecation warning
    eprintln!(
        "{}warning:{} `ralph resume` is deprecated. Use `ralph run --continue` instead.",
        colors::YELLOW,
        colors::RESET
    );

    // Load split core + hats config
    let mut config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    // Check that scratchpad exists (required for resume)
    let scratchpad_path = std::path::Path::new(&config.core.scratchpad.path);
    if !scratchpad_path.exists() {
        anyhow::bail!(
            "Cannot continue: scratchpad not found at '{}'. \
             Start a fresh run with `ralph run`.",
            config.core.scratchpad.path
        );
    }

    info!(
        "Found existing scratchpad at '{}', continuing from previous state",
        config.core.scratchpad.path
    );

    // Apply CLI overrides
    if let Some(max_iter) = args.max_iterations {
        config.event_loop.max_iterations = max_iter;
    }
    if verbose {
        config.verbose = true;
    }

    // Apply execution mode overrides
    // TUI is enabled by default (unless --no-tui is specified)
    if args.autonomous {
        config.cli.default_mode = "autonomous".to_string();
    } else if !args.no_tui {
        config.cli.default_mode = "interactive".to_string();
    }

    // Override idle timeout if specified
    if let Some(timeout) = args.idle_timeout {
        config.cli.idle_timeout_secs = timeout;
    }

    // Validate configuration
    let warnings = config
        .validate()
        .context("Configuration validation failed")?;
    for warning in &warnings {
        eprintln!("{warning}");
    }

    // Handle auto-detection if backend is "auto"
    if config.cli.backend == "auto" {
        let priority = config.get_agent_priority();
        let detected = detect_backend(&priority, |backend| {
            config.adapter_settings(backend).enabled
        });

        match detected {
            Ok(backend) => {
                info!("Auto-detected backend: {}", backend);
                config.cli.backend = backend;
            }
            Err(e) => {
                eprintln!("{e}");
                return Err(anyhow::Error::new(e));
            }
        }
    }

    // Run the orchestration loop in resume mode
    // The key difference: we publish task.resume instead of task.start,
    // signaling the planner to read the existing scratchpad
    // TUI is enabled by default (unless --no-tui, --autonomous, or --rpc is specified)
    let enable_tui = !args.no_tui && !args.autonomous && !args.rpc;
    let enable_rpc = args.rpc;
    let verbosity = Verbosity::resolve(verbose || args.verbose, args.quiet);
    let reason = loop_runner::run_loop_impl(
        config,
        color_mode,
        true,
        enable_tui,
        enable_rpc,
        verbosity,
        args.record_session,
        None,       // Deprecated resume command doesn't have loop_context
        Vec::new(), // Resume command doesn't support custom args
        None,       // Use config.features.auto_merge (deprecated command)
        None,       // Deprecated resume command doesn't support --loop-id
    )
    .await?;
    let exit_code = reason.exit_code();

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::emit::{EmitArgs, run_with_root as emit_command_with_root};
    use crate::commands::events::EventsArgs;
    use crate::commands::tutorial::tutorial_steps;
    use crate::test_support::CwdGuard;
    use ralph_core::{HookMutationConfig, HookOnError, HookPhaseEvent, HookSpec, UrgentSteerStore};
    use std::path::PathBuf;
    use tempfile::TempDir;
    #[test]
    fn test_required_restart_command_matches_contract() {
        let command = required_restart_command(4242);
        assert_eq!(
            command,
            "kill 4242 && RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- resume -c ralph.test.yml"
        );
    }

    #[test]
    fn test_clear_restart_request_signal_removes_sentinel_file() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let restart_dir = temp_dir.path().join(".ralph");
        std::fs::create_dir_all(&restart_dir).expect("create .ralph dir");
        let restart_path = restart_dir.join("restart-requested");
        std::fs::write(&restart_path, "requested").expect("write sentinel");

        clear_restart_request_signal(temp_dir.path());

        assert!(
            !restart_path.exists(),
            "restart sentinel should be removed before restart command dispatch"
        );
    }

    #[test]
    fn test_cli_parses_global_hats_flag() {
        let cli = Cli::try_parse_from(["ralph", "run", "-H", "builtin:code-assist"])
            .expect("CLI parse failed");
        assert_eq!(cli.hats.as_deref(), Some("builtin:code-assist"));
    }

    #[test]
    fn test_bot_daemon_parses_global_config_flag() {
        let cli = Cli::try_parse_from(["ralph", "bot", "daemon", "-c", "ralph.bot.yml"])
            .expect("CLI parse failed");

        assert!(cli.config.iter().any(|value| value == "ralph.bot.yml"));
        assert!(matches!(
            cli.command,
            Some(Commands::Bot(crate::bot::BotArgs {
                command: crate::bot::BotCommands::Daemon(_),
            }))
        ));
    }

    #[test]
    fn test_doctor_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "doctor"]).expect("CLI parse failed");

        assert!(matches!(cli.command, Some(Commands::Doctor(_))));
    }

    #[test]
    fn test_tutorial_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "tutorial"]).expect("CLI parse failed");

        assert!(matches!(cli.command, Some(Commands::Tutorial(_))));
    }

    #[test]
    fn test_mcp_serve_parses_command() {
        let cli = Cli::try_parse_from(["ralph", "mcp", "serve"]).expect("CLI parse failed");
        assert!(matches!(cli.command, Some(Commands::Mcp(_))));
    }

    #[test]
    fn test_mcp_serve_parses_workspace_root_flag() {
        let cli = Cli::try_parse_from([
            "ralph",
            "mcp",
            "serve",
            "--workspace-root",
            "/tmp/ralph-workspace",
        ])
        .expect("CLI parse failed");

        match cli.command {
            Some(Commands::Mcp(crate::mcp::McpArgs {
                command: crate::mcp::McpCommands::Serve(crate::mcp::ServeArgs { workspace_root }),
            })) => {
                assert_eq!(
                    workspace_root,
                    Some(std::path::PathBuf::from("/tmp/ralph-workspace"))
                );
            }
            other => panic!("unexpected CLI parse result: {other:?}"),
        }
    }

    #[test]
    fn test_emit_command_resolves_marker_relative_to_workspace_root_from_nested_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260309-test.jsonl\n",
        )
        .expect("write marker");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: "debug.step".to_string(),
                payload: "task_id=demo".to_string(),
                json: false,
                ts: Some("2026-03-09T00:00:00Z".to_string()),
                file: PathBuf::from(".ralph/events.jsonl"),
            },
            Some(&workspace),
        )
        .expect("emit command");

        let events = std::fs::read_to_string(workspace.join(".ralph/events-20260309-test.jsonl"))
            .expect("read events");
        assert!(events.contains("\"topic\":\"debug.step\""));
        assert!(events.contains("task_id=demo"));
    }

    #[test]
    fn test_emit_command_blocks_once_when_urgent_steer_pending() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
            .append_message("stop and fix the failing tests")
            .expect("write urgent steer");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: "debug.step".to_string(),
                payload: "task_id=demo".to_string(),
                json: false,
                ts: Some("2026-03-09T00:00:00Z".to_string()),
                file: PathBuf::from(".ralph/events.jsonl"),
            },
            Some(&workspace),
        )
        .expect_err("urgent steer should block first emit");

        let message = format!("{err:#}");
        assert!(message.contains("Urgent steer is pending"));
        assert!(message.contains("stop and fix the failing tests"));

        assert!(
            UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
                .load()
                .expect("load marker")
                .is_none(),
            "first blocked emit should clear urgent steer marker"
        );
    }

    #[test]
    fn test_tutorial_steps_cover_core_topics() {
        let steps = tutorial_steps();
        assert_eq!(steps.len(), 3);
        assert!(steps.iter().any(|step| step.title.contains("Hats")));
        assert!(
            steps
                .iter()
                .any(|step| step.title.contains("Hat collections"))
        );
        assert!(steps.iter().any(|step| step.title.contains("Workflow")));
    }

    #[tokio::test]
    async fn test_auto_preflight_dry_run_returns_report() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        config.features.preflight.skip = vec!["git".to_string(), "tools".to_string()];
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("definitely-missing-12345".to_string());

        let report = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap();

        let report = report.expect("expected preflight report in dry-run mode");
        assert!(!report.passed);
        assert!(report.failures >= 1);
    }

    #[tokio::test]
    async fn test_auto_preflight_skip_list_can_omit_hooks_check_failures() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        config.cli.backend = "custom".to_string();

        let backend_cmd = temp_dir.path().join("backend-ok");
        std::fs::write(&backend_cmd, "ok").unwrap();
        config.cli.command = Some(backend_cmd.to_string_lossy().to_string());

        config.hooks.enabled = true;
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![HookSpec {
                name: "broken-hook".to_string(),
                command: vec!["./scripts/hooks/missing.sh".to_string()],
                cwd: None,
                env: std::collections::HashMap::new(),
                timeout_seconds: None,
                max_output_bytes: None,
                on_error: Some(HookOnError::Block),
                suspend_mode: None,
                mutate: HookMutationConfig::default(),
                extra: std::collections::HashMap::new(),
            }],
        );

        let unskipped = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap()
            .expect("dry-run preflight report");

        assert!(!unskipped.passed);
        let hooks_check = unskipped
            .checks
            .iter()
            .find(|check| check.name == "hooks")
            .expect("hooks check should be present without skip");
        assert_eq!(hooks_check.status, CheckStatus::Fail);

        config.features.preflight.skip = vec!["hooks".to_string()];
        let skipped = run_auto_preflight(&config, false, false, AutoPreflightMode::DryRun)
            .await
            .unwrap()
            .expect("dry-run preflight report");

        assert!(skipped.passed);
        assert!(skipped.checks.iter().all(|check| check.name != "hooks"));
    }

    #[tokio::test]
    async fn test_auto_preflight_run_fails_on_check_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();
        config.features.preflight.enabled = true;
        config.features.preflight.skip = vec!["git".to_string(), "tools".to_string()];
        config.cli.backend = "custom".to_string();
        config.cli.command = Some("definitely-missing-12345".to_string());

        let err = run_auto_preflight(&config, false, false, AutoPreflightMode::Run)
            .await
            .expect_err("expected preflight failure in run mode");

        assert!(err.to_string().contains("Preflight checks failed"));
    }

    /// Regression test for prompt_summary reading file content instead of path.
    ///
    /// Previously, when prompt_file was used, the prompt_summary would just
    /// return the file path string. This caused confusing error messages like
    /// "Configuration file not found at con..." when the path was displayed.
    ///
    /// The fix ensures prompt_summary reads the actual file content.
    #[test]
    fn test_prompt_summary_reads_file_content_not_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt_path = temp_dir.path().join("PROMPT.md");
        let prompt_content = "Build a feature that does amazing things";

        // Write the prompt file
        std::fs::write(&prompt_path, prompt_content).unwrap();

        // Create config with prompt_file set
        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic from run_command
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: summary contains file content, NOT the file path
        assert_eq!(prompt_summary, prompt_content);
        assert!(!prompt_summary.contains("PROMPT.md"));
        assert!(!prompt_summary.contains(&temp_dir.path().to_string_lossy().to_string()));
    }

    #[test]
    fn test_prompt_summary_truncates_long_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prompt_path = temp_dir.path().join("LONG_PROMPT.md");
        let long_content = "X".repeat(150); // 150 chars, exceeds 100 limit

        std::fs::write(&prompt_path, &long_content).unwrap();

        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = prompt_path.to_string_lossy().to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: truncated to 100 chars total
        assert_eq!(prompt_summary.len(), 100);
        assert!(prompt_summary.ends_with("..."));
    }

    #[test]
    fn test_prompt_summary_returns_no_prompt_for_missing_file() {
        let mut config = RalphConfig::default();
        config.event_loop.prompt_file = "/nonexistent/path/PROMPT.md".to_string();
        config.event_loop.prompt = None;

        // Simulate the prompt_summary logic
        let prompt_summary = config
            .event_loop
            .prompt
            .clone()
            .or_else(|| {
                let prompt_file = &config.event_loop.prompt_file;
                if prompt_file.is_empty() {
                    None
                } else {
                    let path = std::path::Path::new(prompt_file);
                    if path.exists() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                }
            })
            .map(|p| truncate_with_ellipsis(&p, 100))
            .unwrap_or_else(|| "[no prompt]".to_string());

        // Assert: returns "[no prompt]" for missing file
        assert_eq!(prompt_summary, "[no prompt]");
    }

    #[test]
    fn test_format_preflight_summary_with_failures() {
        let report = PreflightReport {
            passed: false,
            warnings: 1,
            failures: 1,
            checks: vec![
                ralph_core::CheckResult::pass("config", "Config"),
                ralph_core::CheckResult::warn("backend", "Backend", "Missing"),
                ralph_core::CheckResult::fail("paths", "Paths", "Missing path"),
            ],
        };

        let summary = format_preflight_summary(&report);

        assert!(summary.contains("✓"));
        assert!(summary.contains("⚠"));
        assert!(summary.contains("✗"));
        assert!(summary.contains("(1 failure)"));
    }

    #[test]
    fn test_format_preflight_summary_no_checks() {
        let report = PreflightReport {
            passed: true,
            warnings: 0,
            failures: 0,
            checks: Vec::new(),
        };

        let summary = format_preflight_summary(&report);

        assert_eq!(summary, "no checks");
    }

    #[test]
    fn test_preflight_failure_detail_strict_includes_warnings() {
        let report = PreflightReport {
            passed: false,
            warnings: 2,
            failures: 1,
            checks: Vec::new(),
        };

        assert_eq!(preflight_failure_detail(&report, false), "1 failure");
        assert_eq!(
            preflight_failure_detail(&report, true),
            "1 failure, 2 warnings"
        );
    }

    #[test]
    fn test_print_preflight_summary_handles_failures_and_warnings() {
        let report = PreflightReport {
            passed: false,
            warnings: 1,
            failures: 1,
            checks: vec![
                ralph_core::CheckResult::pass("config", "Config"),
                ralph_core::CheckResult::warn("backend", "Backend", "Missing"),
                ralph_core::CheckResult::fail("paths", "Paths", "Missing path"),
            ],
        };

        print_preflight_summary(&report, true, "Preflight: ", true);
        print_preflight_summary(&report, false, "Preflight: ", false);
    }

    fn default_run_args() -> RunArgs {
        RunArgs {
            prompt_text: None,
            backend: Some("claude".to_string()),
            prompt_file: None,
            max_iterations: None,
            completion_promise: None,
            dry_run: false,
            continue_mode: false,
            loop_id: None,
            no_tui: true,
            autonomous: false,
            rpc: false,
            legacy_tui: false,
            idle_timeout: None,
            exclusive: false,
            no_auto_merge: false,
            skip_preflight: true,
            verbose: false,
            quiet: false,
            record_session: None,
            custom_args: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_run_command_continue_missing_scratchpad_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let mut args = default_run_args();
        args.continue_mode = true;

        let err = run_command(&[], None, false, ColorMode::Never, args)
            .await
            .expect_err("expected missing scratchpad error");
        assert!(err.to_string().contains("scratchpad not found"));
    }

    #[tokio::test]
    async fn test_run_command_dry_run_inline_prompt_skips_execution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let mut args = default_run_args();
        args.dry_run = true;
        args.prompt_text = Some("Test inline prompt".to_string());

        run_command(&[], None, false, ColorMode::Never, args)
            .await
            .expect("dry run should succeed");
    }

    #[tokio::test]
    async fn test_run_command_allows_single_file_combined_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        std::fs::write(
            temp_dir.path().join("ralph.yml"),
            r#"
cli:
  backend: claude
hats:
  builder:
    name: Builder
    description: Test builder
    triggers: ["build.task"]
    publishes: ["build.done"]
"#,
        )
        .unwrap();

        let mut args = default_run_args();
        args.dry_run = true;
        args.prompt_text = Some("Test inline prompt".to_string());

        run_command(
            &[ConfigSource::File(std::path::PathBuf::from("ralph.yml"))],
            None,
            false,
            ColorMode::Never,
            args,
        )
        .await
        .expect("combined config should be accepted");
    }

    #[test]
    fn test_diagnostics_eligible_for_run_command() {
        let command = Some(Commands::Run(default_run_args()));
        assert!(is_diagnostics_eligible_command(command.as_ref()));
    }

    #[test]
    fn test_diagnostics_eligible_for_no_subcommand() {
        assert!(is_diagnostics_eligible_command(None));
    }

    #[test]
    fn test_diagnostics_not_eligible_for_emit_command() {
        let command = Some(Commands::Emit(EmitArgs {
            topic: "test.event".to_string(),
            payload: String::new(),
            json: false,
            ts: None,
            file: PathBuf::from(".ralph/events.jsonl"),
        }));
        assert!(!is_diagnostics_eligible_command(command.as_ref()));
    }

    #[test]
    fn test_diagnostics_not_eligible_for_events_command() {
        let command = Some(Commands::Events(EventsArgs {
            last: None,
            topic: None,
            iteration: None,
            format: OutputFormat::Table,
            file: None,
            clear: false,
        }));
        assert!(!is_diagnostics_eligible_command(command.as_ref()));
    }
}
