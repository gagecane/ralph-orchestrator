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
mod cli_parser;
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
mod logging_init;
mod loop_runner;
mod loops;
mod mcp;
mod memory;
mod preflight;
mod preflight_helpers;
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

use anyhow::Result;
use clap::Parser;

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

pub use config_source::{ConfigSource, HatsSource};
pub(crate) use config_source::{
    apply_config_overrides, ensure_scratchpad_directory, load_config_with_overrides,
};

pub(crate) use cli_parser::{Cli, Commands, RunArgs};

// The run/resume/subprocess-TUI machinery lives in `commands/run.rs`.
// We re-export the public entry points here so `main` can dispatch to them.
pub(crate) use commands::run::{
    is_diagnostics_eligible_command, resume_command, run_command, tui_command,
};

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

    logging_init::init(&logging_init::LoggingConfig {
        tui_enabled,
        rpc_enabled,
        mcp_enabled,
        diagnostics_enabled,
        filter,
    });

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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::emit::{EmitArgs, run_with_root as emit_command_with_root};
    use crate::commands::events::EventsArgs;
    use crate::commands::tutorial::tutorial_steps;
    use crate::test_support::CwdGuard;
    use ralph_core::{RalphConfig, UrgentSteerStore, truncate_with_ellipsis};
    use std::path::PathBuf;
    use tempfile::TempDir;

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
