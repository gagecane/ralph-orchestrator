//! Top-level CLI argument parsing (clap definitions).
//!
//! Contains:
//! - [`Cli`] — the root parser with global options (`-c`, `-H`, `--verbose`, `--color`)
//! - [`Commands`] — the enum of all top-level subcommands
//! - [`RunArgs`], [`ResumeArgs`], [`TuiArgs`] — arguments for `run`, `resume`, `tui`
//!
//! Subcommand-specific args for other subcommands live alongside their
//! handlers (for example `commands::plan::PlanArgs`, `preflight::PreflightArgs`).

use clap::{ArgAction, Parser, Subcommand};
use std::path::PathBuf;

use crate::cli_types::ColorMode;
use crate::{bot, commands, doctor, hats, hooks, loops, mcp, preflight, tools, wave, web};

/// Ralph Orchestrator - Multi-agent orchestration framework
#[derive(Parser, Debug)]
#[command(name = "ralph", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    // ─────────────────────────────────────────────────────────────────────────
    // Global options (available for all subcommands)
    // ─────────────────────────────────────────────────────────────────────────
    /// Core configuration source: file path, URL, or core.field=value override.
    /// Can be specified multiple times. Overrides are applied after core config loading.
    /// If not set, defaults to `ralph.yml` or `$RALPH_CONFIG`.
    #[arg(short, long, global = true, action = ArgAction::Append)]
    pub(crate) config: Vec<String>,

    /// Hat collection source: file path, builtin:name, or URL.
    ///
    /// Example: `-H builtin:code-assist` or `-H .ralph/hats/my-workflow.yml`
    #[arg(short = 'H', long, global = true)]
    pub(crate) hats: Option<String>,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    /// Color output mode (auto, always, never)
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    pub(crate) color: ColorMode,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
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
pub(crate) struct RunArgs {
    /// Inline prompt text (mutually exclusive with -P/--prompt-file)
    #[arg(short = 'p', long = "prompt", conflicts_with = "prompt_file")]
    pub(crate) prompt_text: Option<String>,

    /// Override backend from config (cli > config > auto-detect)
    #[arg(short = 'b', long = "backend", value_name = "BACKEND")]
    pub(crate) backend: Option<String>,

    /// Prompt file path (mutually exclusive with -p/--prompt)
    #[arg(short = 'P', long = "prompt-file", conflicts_with = "prompt_text")]
    pub(crate) prompt_file: Option<PathBuf>,

    /// Override max iterations
    #[arg(long)]
    pub(crate) max_iterations: Option<u32>,

    /// Override completion promise
    #[arg(long)]
    pub(crate) completion_promise: Option<String>,

    /// Dry run - show what would be executed without running
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Continue from existing scratchpad (resume interrupted loop).
    /// Use this when a previous run was interrupted and you want to
    /// continue from where it left off.
    #[arg(long = "continue")]
    pub(crate) continue_mode: bool,

    /// Explicit loop ID to use with --continue.
    /// Reuses tasks from the specified loop instead of generating a new ID.
    /// If omitted with --continue, reuses the existing current-loop-id marker.
    #[arg(long, requires = "continue_mode")]
    pub(crate) loop_id: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Execution Mode Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    pub(crate) no_tui: bool,

    /// Force autonomous mode (headless, non-interactive).
    /// Overrides default_mode from config.
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    pub(crate) autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    /// All output is valid JSON; input accepts RpcCommand messages.
    /// Use this for IDE integrations and machine-readable interfaces.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    pub(crate) rpc: bool,

    /// Use legacy in-process TUI mode instead of subprocess RPC mode.
    /// This is an escape hatch during the migration to subprocess TUI.
    #[arg(long, hide = true, conflicts_with = "rpc", conflicts_with = "no_tui")]
    pub(crate) legacy_tui: bool,

    /// Idle timeout in seconds for interactive mode (default: 30).
    /// Process is terminated after this many seconds of inactivity.
    /// Set to 0 to disable idle timeout.
    #[arg(long)]
    pub(crate) idle_timeout: Option<u32>,

    // ─────────────────────────────────────────────────────────────────────────
    // Multi-Loop Concurrency Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Wait for the primary loop slot instead of spawning into a worktree.
    /// Use this when you want to ensure only one loop runs at a time.
    #[arg(long)]
    pub(crate) exclusive: bool,

    /// Skip automatic merge after loop completes (keep worktree for manual handling).
    /// Only relevant for parallel loops running in worktrees.
    #[arg(long)]
    pub(crate) no_auto_merge: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Preflight Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Skip preflight checks before loop start.
    /// Overrides features.preflight.enabled from config.
    #[arg(long)]
    pub(crate) skip_preflight: bool,

    // ─────────────────────────────────────────────────────────────────────────
    // Verbosity Options
    // ─────────────────────────────────────────────────────────────────────────
    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    pub(crate) verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub(crate) quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    pub(crate) record_session: Option<PathBuf>,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    pub(crate) custom_args: Vec<String>,
}

/// Arguments for the resume subcommand.
///
/// Per spec: "When loop terminates due to safeguard (not completion promise),
/// user can run `ralph resume` to restart reading existing scratchpad."
#[derive(Parser, Debug)]
pub(crate) struct ResumeArgs {
    /// Override max iterations (from current position)
    #[arg(long)]
    pub(crate) max_iterations: Option<u32>,

    /// Disable TUI observation mode (TUI is enabled by default)
    #[arg(long, conflicts_with = "autonomous")]
    pub(crate) no_tui: bool,

    /// Force autonomous mode
    #[arg(short, long, conflicts_with = "no_tui", conflicts_with = "rpc")]
    pub(crate) autonomous: bool,

    /// Run in RPC mode with JSON-lines protocol on stdin/stdout.
    #[arg(long, conflicts_with = "no_tui", conflicts_with = "autonomous")]
    pub(crate) rpc: bool,

    /// Idle timeout in seconds for TUI mode
    #[arg(long)]
    pub(crate) idle_timeout: Option<u32>,

    /// Enable verbose output (show tool results and session summary)
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    pub(crate) verbose: bool,

    /// Suppress streaming output (for CI/scripting)
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub(crate) quiet: bool,

    /// Record session to JSONL file for replay testing
    #[arg(long, value_name = "FILE")]
    pub(crate) record_session: Option<PathBuf>,
}

/// Arguments for the `ralph tui` subcommand.
#[derive(Parser, Debug)]
pub(crate) struct TuiArgs {
    /// ralph-api server URL to connect to.
    /// Defaults to RALPH_API_URL env var, or http://127.0.0.1:3000.
    #[arg(short = 'u', long = "url")]
    pub(crate) url: Option<String>,
}
