//! `ralph run` and `ralph resume` command handlers.
//!
//! Contains the core orchestration loop entry points:
//! - [`run_command`] — main run command (the big orchestrator dispatch)
//! - [`resume_command`] — deprecated resume command (use `run --continue` instead)
//! - [`run_subprocess_tui`] — spawns an RPC child and attaches the TUI
//! - [`tui_command`] — attach TUI to an existing `ralph-api` server
//!
//! These were previously inlined into `main.rs`; extracted so `main.rs` is a
//! thin dispatch shell.

use anyhow::{Context, Result};
use ralph_adapters::detect_backend;
use ralph_core::{
    LockError, LoopContext, LoopEntry, LoopLock, LoopRegistry, TerminationReason,
    truncate_with_ellipsis,
    worktree::{WorktreeConfig, create_worktree, ensure_gitignore, remove_worktree},
};
use std::io::IsTerminal;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::cli_parser::{Commands, ResumeArgs, RunArgs, TuiArgs};
use crate::cli_types::{ColorMode, Verbosity};
use crate::config_source::{ConfigSource, HatsSource, ensure_scratchpad_directory};
use crate::display::{colors, truncate};
use crate::loop_runner;
use crate::preflight;
use crate::preflight_helpers::{AutoPreflightMode, print_preflight_summary, run_auto_preflight};

/// Attach the TUI to a running `ralph-api` server (separate from `ralph run`).
pub async fn tui_command(args: TuiArgs) -> Result<()> {
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
pub fn is_diagnostics_eligible_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::Run(_) | Commands::Resume(_)) | None)
}

/// The shell restart command used when an orchestration loop requests a restart.
///
/// This is pulled out so it can be unit-tested; the command must exactly match
/// the contract expected by downstream tooling (see `test_required_restart_command_matches_contract`).
pub(crate) fn required_restart_command(pid: u32) -> String {
    format!("kill {pid} && RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- resume -c ralph.test.yml")
}

/// Clear the restart sentinel file so the next loop invocation doesn't re-trigger.
pub(crate) fn clear_restart_request_signal(workspace_root: &std::path::Path) {
    let restart_path = workspace_root.join(".ralph/restart-requested");
    let _ = std::fs::remove_file(&restart_path);
}

/// Main entry point for `ralph run`.
///
/// Loads config, validates CLI overrides, handles worktree/lock coordination,
/// runs preflight, then dispatches to either subprocess TUI mode or the
/// in-process loop runner.
pub async fn run_command(
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
pub async fn resume_command(
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
}
