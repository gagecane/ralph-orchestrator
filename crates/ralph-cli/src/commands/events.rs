//! `ralph events` — view event history for debugging.

use crate::{ColorMode, OutputFormat, display, resolve_marker_target, resolve_workspace_root};
use crate::display::colors;
use anyhow::Result;
use clap::Parser;
use ralph_core::EventHistory;
use std::fs;
use std::path::PathBuf;

/// Arguments for the events subcommand.
#[derive(Parser, Debug)]
pub struct EventsArgs {
    /// Show only the last N events
    #[arg(long)]
    pub last: Option<usize>,

    /// Filter by topic (e.g., "build.blocked")
    #[arg(long)]
    pub topic: Option<String>,

    /// Filter by iteration number
    #[arg(long)]
    pub iteration: Option<u32>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    /// Path to events file (default: auto-detects current run)
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Clear the event history
    #[arg(long)]
    pub clear: bool,
}

pub fn run(color_mode: ColorMode, args: EventsArgs) -> Result<()> {
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(None);
    let current_events_marker = workspace_root.join(".ralph/current-events");

    // Read events path from marker file, fall back to default if marker doesn't exist
    // This ensures `ralph events` reads from the same events file as the active run
    let history = match args.file {
        Some(path) => EventHistory::new(path),
        None => fs::read_to_string(&current_events_marker)
            .map(|s| EventHistory::new(resolve_marker_target(&workspace_root, &s)))
            .unwrap_or_else(|_| EventHistory::new(workspace_root.join(".ralph/events.jsonl"))),
    };

    // Handle clear command
    if args.clear {
        history.clear()?;
        if use_colors {
            println!("{}✓{} Event history cleared", colors::GREEN, colors::RESET);
        } else {
            println!("Event history cleared");
        }
        return Ok(());
    }

    if !history.exists() {
        if use_colors {
            println!(
                "{}No event history found.{} Run `ralph` to generate events.",
                colors::DIM,
                colors::RESET
            );
        } else {
            println!("No event history found. Run `ralph` to generate events.");
        }
        return Ok(());
    }

    // Read and filter events
    let mut records = history.read_all()?;

    // Apply filters in sequence
    if let Some(ref topic) = args.topic {
        records.retain(|r| r.topic == *topic);
    }

    if let Some(iteration) = args.iteration {
        records.retain(|r| r.iteration == iteration);
    }

    // Apply 'last' filter after other filters (to get last N of filtered results)
    if let Some(n) = args.last
        && records.len() > n
    {
        records = records.into_iter().rev().take(n).rev().collect();
    }

    if records.is_empty() {
        if use_colors {
            println!("{}No matching events found.{}", colors::DIM, colors::RESET);
        } else {
            println!("No matching events found.");
        }
        return Ok(());
    }

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&records)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            display::print_events_table(&records, use_colors);
        }
    }

    Ok(())
}
