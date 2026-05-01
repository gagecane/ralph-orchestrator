//! Small standalone helpers used by the orchestration loop.
//!
//! This module hosts low-ceremony utilities that don't fit a larger
//! domain module on their own: last-commit lookup for summaries, prompt
//! content resolution with precedence rules, and planning-session
//! conversation replay.

use std::ffi::OsStr;
use std::fs;
use std::process::Command;

use anyhow::{Context, Result};
use ralph_core::EventLoop;
use ralph_proto::Event;
use tracing::{debug, info, warn};

/// Gets the last commit info (short SHA and subject) for the summary file.
pub(super) fn get_last_commit_info_with_cmd(git_cmd: &OsStr) -> Option<String> {
    let output = Command::new(git_cmd)
        .args(["log", "-1", "--format=%h: %s"])
        .output()
        .ok()?;

    if output.status.success() {
        let info = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if info.is_empty() { None } else { Some(info) }
    } else {
        None
    }
}

pub(super) fn get_last_commit_info() -> Option<String> {
    get_last_commit_info_with_cmd(OsStr::new("git"))
}

/// Resolves prompt content with proper precedence.
///
/// Precedence (highest to lowest):
/// 1. CLI -p "text" (inline prompt text)
/// 2. CLI -P path (prompt file path)
/// 3. Config event_loop.prompt (inline prompt text)
/// 4. Config event_loop.prompt_file (prompt file path)
/// 5. Default PROMPT.md
///
/// Note: CLI overrides are already applied to config before this function is called.
pub(super) fn resolve_prompt_content(
    event_loop_config: &ralph_core::EventLoopConfig,
) -> Result<String> {
    debug!(
        inline_prompt = ?event_loop_config.prompt.as_ref().map(|s| format!("{}...", &s[..s.len().min(50)])),
        prompt_file = %event_loop_config.prompt_file,
        "Resolving prompt content"
    );

    // Check for inline prompt first (CLI -p or config prompt)
    if let Some(ref inline_text) = event_loop_config.prompt {
        debug!(len = inline_text.len(), "Using inline prompt text");
        return Ok(inline_text.clone());
    }

    // Check for prompt file (CLI -P or config prompt_file or default)
    let prompt_file = &event_loop_config.prompt_file;
    if !prompt_file.is_empty() {
        let path = std::path::Path::new(prompt_file);
        debug!(path = %prompt_file, exists = path.exists(), "Checking prompt file");
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read prompt file: {}", prompt_file))?;
            debug!(path = %prompt_file, len = content.len(), "Read prompt from file");
            return Ok(content);
        } else {
            // File specified but doesn't exist - error with helpful message
            anyhow::bail!(
                "Prompt file '{}' not found. Check the path or use -p \"text\" for inline prompt.",
                prompt_file
            );
        }
    }

    // No valid prompt source found
    anyhow::bail!(
        "No prompt specified. Use -p \"text\" for inline prompt, -P path for file, \
         or create PROMPT.md in the current directory."
    )
}

/// Checks for planning session user responses and publishes them as events.
///
/// When running in planning mode (RALPH_PLANNING_SESSION_ID is set),
/// this function reads the conversation file for new user responses and
/// publishes them as `user.response` events to the event loop.
pub(super) fn check_planning_session_responses(event_loop: &mut EventLoop) -> Result<()> {
    // Get the planning session ID from environment
    let session_id = match std::env::var("RALPH_PLANNING_SESSION_ID") {
        Ok(id) => id,
        Err(_) => return Ok(()), // Not in planning mode
    };
    check_planning_session_responses_for_session(event_loop, &session_id)
}

pub(super) fn check_planning_session_responses_for_session(
    event_loop: &mut EventLoop,
    session_id: &str,
) -> Result<()> {
    // Get loop context to find the conversation file path
    let ctx = match event_loop.loop_context() {
        Some(ctx) => ctx,
        None => return Ok(()), // No context, can't find conversation file
    };

    let conversation_path = ctx.planning_conversation_path(session_id);

    // Read conversation entries and look for new responses
    // We track which response IDs we've already processed to avoid duplicates

    // Track processed response IDs (static to persist across iterations)
    static PROCESSED_RESPONSES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    let conversation_content = match fs::read_to_string(&conversation_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()), // File doesn't exist yet
        Err(e) => {
            warn!(
                session_id = %session_id,
                error = %e,
                "Failed to read planning conversation file"
            );
            return Ok(());
        }
    };

    let mut processed = PROCESSED_RESPONSES.lock().unwrap();

    for line in conversation_content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse the conversation entry
        let entry: ralph_core::planning_session::ConversationEntry =
            match serde_json::from_str(line) {
                Ok(entry) => entry,
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        line = %line,
                        error = %e,
                        "Failed to parse conversation entry"
                    );
                    continue;
                }
            };

        // Only process user_response entries
        if entry.entry_type != ralph_core::planning_session::ConversationType::UserResponse {
            continue;
        }

        // Check if we've already processed this response
        let response_key = format!("{}:{}", entry.id, entry.ts);
        if processed.contains(&response_key) {
            continue;
        }

        // Publish as user.response event
        let event = Event::new(
            "user.response",
            format!("[id: {}] {}", entry.id, entry.text),
        );
        event_loop.bus().publish(event.clone());

        info!(
            session_id = %session_id,
            response_id = %entry.id,
            "Published user response from planning session"
        );

        // Mark as processed
        processed.push(response_key);
    }

    Ok(())
}
