//! `ralph emit` — emit an event to the current run's events file with proper JSON formatting.

use crate::display::colors;
use crate::{
    ColorMode, resolve_marker_target, resolve_workspace_root, urgent_steer_path_from_workspace,
};
use anyhow::{Context, Result};
use clap::Parser;
use ralph_core::UrgentSteerStore;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Arguments for the emit subcommand.
#[derive(Parser, Debug)]
pub struct EmitArgs {
    /// Event topic (e.g., "build.done", "review.complete")
    pub topic: String,

    /// Event payload - string or JSON (optional, defaults to empty)
    #[arg(default_value = "")]
    pub payload: String,

    /// Parse payload as JSON object instead of string
    #[arg(long, short)]
    pub json: bool,

    /// Custom ISO 8601 timestamp (defaults to current time)
    #[arg(long)]
    pub ts: Option<String>,

    /// Path to events file (defaults to .ralph/events.jsonl)
    #[arg(long, default_value = ".ralph/events.jsonl")]
    pub file: PathBuf,
}

/// Emit an event to the current run's events file with proper JSON formatting.
///
/// This command provides a deterministic way for agents to emit events without
/// risking malformed JSONL from manual echo commands. All JSON serialization
/// is handled via serde_json, ensuring proper escaping of payloads.
///
/// Events are written to the path specified in `.ralph/current-events` marker file
/// (created by `ralph run`), or falls back to `.ralph/events.jsonl` if no marker exists.
pub fn run(color_mode: ColorMode, args: EmitArgs) -> Result<()> {
    run_with_root(color_mode, args, None)
}

pub fn run_with_root(color_mode: ColorMode, args: EmitArgs, root: Option<&PathBuf>) -> Result<()> {
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(root);
    let current_events_marker = workspace_root.join(".ralph/current-events");

    if std::env::var("RALPH_WAVE_ID").is_err() {
        let urgent_steer_store = UrgentSteerStore::new(urgent_steer_path_from_workspace(root));
        if let Some(record) = urgent_steer_store
            .take()
            .context("Failed to read urgent-steer marker")?
        {
            let guidance = record
                .messages
                .iter()
                .enumerate()
                .map(|(idx, message)| format!("{}. {}", idx + 1, message))
                .collect::<Vec<_>>()
                .join("\n");

            anyhow::bail!(
                "Urgent steer is pending. Do not hand off yet.\n\n\
                 Human feedback:\n{guidance}\n\n\
                 You have now seen the steer. Address it in this turn, then rerun `ralph emit` \
                 once you are ready to hand off."
            );
        }
    }

    // Generate timestamp if not provided
    let ts = args.ts.unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    // Validate JSON payload if --json flag is set
    let payload = if args.json && !args.payload.is_empty() {
        // Validate it's valid JSON
        serde_json::from_str::<serde_json::Value>(&args.payload).context("Invalid JSON payload")?;
        args.payload
    } else {
        args.payload
    };

    // Build the event record
    // We use serde_json directly to ensure proper escaping
    let payload_value = if args.json && !payload.is_empty() {
        // Parse and embed as object
        serde_json::from_str::<serde_json::Value>(&payload)?
    } else if payload.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(payload)
    };

    let mut record = serde_json::json!({
        "topic": args.topic,
        "payload": payload_value,
        "ts": ts
    });

    // Auto-tag with wave metadata from env vars (set by loop runner on wave workers)
    if let (Ok(wave_id), Ok(wave_index_str)) = (
        std::env::var("RALPH_WAVE_ID"),
        std::env::var("RALPH_WAVE_INDEX"),
    ) && let Ok(wave_index) = wave_index_str.parse::<u32>()
    {
        record["wave_id"] = serde_json::Value::String(wave_id);
        record["wave_index"] = serde_json::Value::Number(wave_index.into());
    }

    // Resolve events file: RALPH_EVENTS_FILE env > marker file > CLI arg
    // This ensures `ralph emit` writes to the same events file as the active run
    let events_file = std::env::var("RALPH_EVENTS_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            fs::read_to_string(&current_events_marker)
                .map(|s| resolve_marker_target(&workspace_root, &s))
                .unwrap_or_else(|_| args.file.clone())
        });

    // Ensure parent directory exists
    if let Some(parent) = events_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Append to file
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    // Write as single-line JSON (JSONL format)
    let json_line = serde_json::to_string(&record)?;
    writeln!(file, "{}", json_line)?;

    // Success message
    if use_colors {
        println!(
            "{}✓{} Event emitted: {}",
            colors::GREEN,
            colors::RESET,
            args.topic
        );
    } else {
        println!("Event emitted: {}", args.topic);
    }

    Ok(())
}
