//! `ralph init` — initialize a new ralph.yml configuration file.
//!
//! This module is the thin CLI wrapper; the actual init logic lives in
//! `crate::init` (backend/preset handling).

use crate::backend_support;
use crate::display::colors;
use crate::{ColorMode, init};
use anyhow::Result;
use clap::Parser;

/// Arguments for the init subcommand.
#[derive(Parser, Debug)]
pub struct InitArgs {
    /// Backend to use (claude, kiro, gemini, codex, amp, copilot, opencode, pi, custom).
    /// Generates core config only.
    #[arg(long, conflicts_with = "list_presets")]
    pub backend: Option<String>,

    /// REMOVED: monolithic presets are no longer supported.
    ///
    /// Use split config instead:
    ///   ralph init --backend <backend>
    ///   ralph run -c ralph.yml -H builtin:<collection>
    #[arg(long, conflicts_with = "list_presets", conflicts_with = "backend")]
    pub preset: Option<String>,

    /// List all available builtin hat collections
    #[arg(long, conflicts_with = "backend", conflicts_with = "preset")]
    pub list_presets: bool,

    /// Overwrite existing ralph.yml if present
    #[arg(long)]
    pub force: bool,
}

pub fn run(color_mode: ColorMode, args: InitArgs) -> Result<()> {
    let use_colors = color_mode.should_use_colors();

    // Handle --list-presets (lists builtin hat collections)
    if args.list_presets {
        println!("{}", init::format_preset_list());
        return Ok(());
    }

    // Hard cutover: --preset no longer writes monolithic config.
    if let Some(preset) = args.preset {
        anyhow::bail!(
            "`ralph init --preset {preset}` was removed.\n\nUse split config:\n  1) Create core config: ralph init --backend <backend>\n  2) Run with hats:     ralph run -c ralph.yml -H builtin:{preset}"
        );
    }

    // Handle --backend alone (minimal config)
    if let Some(backend) = args.backend {
        match init::init_from_backend(&backend, args.force) {
            Ok(()) => {
                if use_colors {
                    println!(
                        "{}✓{} Created ralph.yml with {} backend",
                        colors::GREEN,
                        colors::RESET,
                        backend
                    );
                    println!(
                        "\n{}Next steps:{}\n  1. Create PROMPT.md with your task\n  2. Run core-only: ralph run -c ralph.yml\n  3. Or with hats:  ralph run -c ralph.yml -H builtin:code-assist",
                        colors::DIM,
                        colors::RESET
                    );
                } else {
                    println!("Created ralph.yml with {} backend", backend);
                    println!(
                        "\nNext steps:\n  1. Create PROMPT.md with your task\n  2. Run core-only: ralph run -c ralph.yml\n  3. Or with hats:  ralph run -c ralph.yml -H builtin:code-assist"
                    );
                }
                return Ok(());
            }
            Err(e) => {
                anyhow::bail!("{}", e);
            }
        }
    }

    // No flag specified - show help
    println!("Initialize a new ralph.yml configuration file.\n");
    println!("Usage:");
    println!("  ralph init --backend <backend>   Generate core config (ralph.yml)");
    println!("  ralph init --list-presets        Show builtin hat collections\n");
    println!("Backends: {}", backend_support::VALID_BACKENDS_LABEL);
    println!("\nThen run with hats, e.g.: ralph run -c ralph.yml -H builtin:code-assist");

    Ok(())
}
