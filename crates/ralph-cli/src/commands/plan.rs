//! `ralph plan` and `ralph code-task` — spawn backend with bundled SOPs.

use crate::display::colors;
use crate::{ColorMode, ConfigSource, HatsSource, preflight, sop_runner};
use anyhow::Result;
use clap::Parser;

/// Arguments for the plan subcommand.
///
/// Starts an interactive PDD (Prompt-Driven Development) session.
/// This is a thin wrapper that spawns the AI backend with the bundled
/// PDD SOP, bypassing Ralph's event loop entirely.
#[derive(Parser, Debug)]
pub struct PlanArgs {
    /// The rough idea to develop (optional - SOP will prompt if not provided)
    #[arg(value_name = "IDEA")]
    pub idea: Option<String>,

    /// Backend to use (overrides config and auto-detection)
    #[arg(short, long, value_name = "BACKEND")]
    pub backend: Option<String>,

    /// Enable Claude Code's experimental Agent Teams feature
    #[arg(long)]
    pub teams: bool,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    pub custom_args: Vec<String>,
}

/// Arguments for the task subcommand.
///
/// Starts an interactive code-task-generator session.
/// This is a thin wrapper that spawns the AI backend with the bundled
/// code-task-generator SOP, bypassing Ralph's event loop entirely.
#[derive(Parser, Debug)]
pub struct CodeTaskArgs {
    /// Input: description text or path to PDD plan file
    #[arg(value_name = "INPUT")]
    pub input: Option<String>,

    /// Backend to use (overrides config and auto-detection)
    #[arg(short, long, value_name = "BACKEND")]
    pub backend: Option<String>,

    /// Enable Claude Code's experimental Agent Teams feature
    #[arg(long)]
    pub teams: bool,

    /// Custom backend command and arguments (use after --)
    #[arg(last = true)]
    pub custom_args: Vec<String>,
}

/// Starts a Prompt-Driven Development planning session.
///
/// This is a thin wrapper that bypasses Ralph's event loop entirely.
/// It spawns the AI backend with the bundled PDD SOP for interactive planning.
pub async fn run_plan(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    color_mode: ColorMode,
    args: PlanArgs,
) -> Result<()> {
    use sop_runner::{Sop, SopRunConfig, SopRunError};

    let use_colors = color_mode.should_use_colors();

    // Show what we're starting
    if use_colors {
        println!(
            "{}🎯{} Starting {} session...",
            colors::CYAN,
            colors::RESET,
            Sop::Pdd.name()
        );
    } else {
        println!("Starting {} session...", Sop::Pdd.name());
    }

    let config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    let config = SopRunConfig {
        sop: Sop::Pdd,
        user_input: args.idea,
        backend_override: args.backend,
        config: Some(config),
        config_path: None,
        custom_args: if args.custom_args.is_empty() {
            None
        } else {
            Some(args.custom_args)
        },
        agent_teams: args.teams,
    };

    sop_runner::run_sop(config).map_err(|e| match e {
        SopRunError::NoBackend(no_backend) => anyhow::Error::new(no_backend),
        SopRunError::UnknownBackend(msg) => anyhow::anyhow!("{}", msg),
        SopRunError::SpawnError(io_err) => anyhow::anyhow!("Failed to spawn backend: {}", io_err),
    })
}

/// Starts a code-task-generator session.
///
/// This is a thin wrapper that bypasses Ralph's event loop entirely.
/// It spawns the AI backend with the bundled code-task-generator SOP.
pub async fn run_code_task(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    color_mode: ColorMode,
    args: CodeTaskArgs,
) -> Result<()> {
    use sop_runner::{Sop, SopRunConfig, SopRunError};

    let use_colors = color_mode.should_use_colors();

    // Show what we're starting
    if use_colors {
        println!(
            "{}📋{} Starting {} session...",
            colors::CYAN,
            colors::RESET,
            Sop::CodeTaskGenerator.name()
        );
    } else {
        println!("Starting {} session...", Sop::CodeTaskGenerator.name());
    }

    let config = preflight::load_config_for_preflight(config_sources, hats_source).await?;

    let config = SopRunConfig {
        sop: Sop::CodeTaskGenerator,
        user_input: args.input,
        backend_override: args.backend,
        config: Some(config),
        config_path: None,
        custom_args: if args.custom_args.is_empty() {
            None
        } else {
            Some(args.custom_args)
        },
        agent_teams: args.teams,
    };

    sop_runner::run_sop(config).map_err(|e| match e {
        SopRunError::NoBackend(no_backend) => anyhow::Error::new(no_backend),
        SopRunError::UnknownBackend(msg) => anyhow::anyhow!("{}", msg),
        SopRunError::SpawnError(io_err) => anyhow::anyhow!("Failed to spawn backend: {}", io_err),
    })
}
