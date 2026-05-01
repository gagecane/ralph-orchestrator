//! External entry point for starting an orchestration loop and the
//! helper that constructs the Telegram robot service.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ralph_core::{LoopContext, RalphConfig, TerminationReason};
use tracing::{info, warn};

use crate::Verbosity;

/// Start a loop from an external caller (e.g., the bot daemon).
///
/// Loads config from `ralph.yml`, applies the given prompt, acquires the
/// loop lock, and runs the orchestration loop headlessly. The caller is
/// responsible for Telegram interaction — the spawned loop has `robot.enabled`
/// disabled to prevent a second Telegram poller from conflicting.
///
/// Returns `Ok(TerminationReason)` on completion or `Err` on fatal errors.
pub async fn start_loop(
    prompt: String,
    workspace_root: PathBuf,
    config_path: Option<PathBuf>,
) -> Result<TerminationReason> {
    use crate::{ColorMode, ConfigSource, load_config_with_overrides};

    // Load config from file or defaults
    let config_source = config_path.unwrap_or_else(|| workspace_root.join("ralph.yml"));
    let sources = vec![ConfigSource::File(config_source)];
    let mut config = load_config_with_overrides(&sources)?;

    // Set workspace root to the provided path
    config.core.workspace_root = workspace_root.clone();

    // Apply the prompt
    config.event_loop.prompt = Some(prompt);
    config.event_loop.prompt_file = String::new();

    // Keep robot.enabled as-is from config. When the daemon starts a loop,
    // the loop's own TelegramService handles all Telegram interaction
    // (commands, guidance, responses, check-ins). The daemon stops polling
    // while the loop runs, so there's no conflict.

    // Force autonomous headless mode (no TUI, no interactive)
    config.cli.default_mode = "autonomous".to_string();

    // Normalize and validate
    config.normalize();
    let warnings = config
        .validate()
        .context("Configuration validation failed")?;
    for warning in &warnings {
        tracing::warn!("{}", warning);
    }

    // Auto-detect backend if needed
    if config.cli.backend == "auto" {
        let priority = config.get_agent_priority();
        let detected = ralph_adapters::detect_backend(&priority, |backend| {
            config.adapter_settings(backend).enabled
        });
        match detected {
            Ok(backend) => {
                info!("Auto-detected backend: {}", backend);
                config.cli.backend = backend;
            }
            Err(e) => return Err(anyhow::Error::new(e)),
        }
    }

    // Ensure scratchpad directory exists
    crate::ensure_scratchpad_directory(&config)?;

    // Acquire the loop lock (primary loop)
    let prompt_summary = config.event_loop.prompt.as_deref().unwrap_or("[daemon]");
    let prompt_summary = ralph_core::truncate_with_ellipsis(prompt_summary, 100);

    let _lock_guard = ralph_core::LoopLock::try_acquire(&workspace_root, &prompt_summary)
        .context("Failed to acquire loop lock — another loop may be running")?;

    let loop_context = ralph_core::LoopContext::primary(workspace_root);

    // Run the loop headlessly
    super::run_loop_impl(
        config,
        ColorMode::Never,
        false, // not resume
        false, // no TUI
        false, // no RPC
        Verbosity::Normal,
        None,               // no session recording
        Some(loop_context), // loop context
        Vec::new(),         // no custom args
        None,               // default auto-merge
        None,               // no explicit loop ID
    )
    .await
}

/// Creates a robot service (Telegram) for human-in-the-loop communication.
///
/// Called by `run_loop_impl` when `robot.enabled` is true and this is the primary loop.
/// Returns `None` if the service cannot be created or started.
pub(super) fn create_robot_service(
    config: &RalphConfig,
    context: &LoopContext,
) -> Option<Box<dyn ralph_proto::RobotService>> {
    let workspace_root = context.workspace().to_path_buf();
    let bot_token = config.robot.resolve_bot_token();
    let api_url = config.robot.resolve_api_url();
    let timeout_secs = config.robot.timeout_seconds.unwrap_or(300);
    let loop_id = context
        .loop_id()
        .map(String::from)
        .unwrap_or_else(|| "main".to_string());

    match ralph_telegram::TelegramService::new(
        workspace_root,
        bot_token,
        api_url,
        timeout_secs,
        loop_id,
    ) {
        Ok(service) => {
            if let Err(e) = service.start() {
                warn!(error = %e, "Failed to start robot service");
                return None;
            }
            info!(
                bot_token = %service.bot_token_masked(),
                timeout_secs = service.timeout_secs(),
                "Robot human-in-the-loop service active"
            );
            Some(Box::new(service))
        }
        Err(e) => {
            warn!(error = %e, "Failed to create robot service");
            None
        }
    }
}
