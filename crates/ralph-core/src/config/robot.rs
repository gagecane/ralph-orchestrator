//! RObot (Ralph-Orchestrator bot) and Telegram configuration.

use serde::{Deserialize, Serialize};

use super::errors::ConfigError;

/// RObot (Ralph-Orchestrator bot) configuration.
///
/// Enables bidirectional communication between AI agents and humans
/// during orchestration loops. When enabled, agents can emit `human.interact`
/// events to request clarification (blocking the loop), and humans can
/// send proactive guidance via Telegram.
///
/// Example configuration:
/// ```yaml
/// RObot:
///   enabled: true
///   timeout_seconds: 300
///   checkin_interval_seconds: 120  # Optional: send status every 2 min
///   telegram:
///     bot_token: "..."  # Or set RALPH_TELEGRAM_BOT_TOKEN env var
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RobotConfig {
    /// Whether the RObot is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Timeout in seconds for waiting on human responses.
    /// Required when enabled (no default — must be explicit).
    pub timeout_seconds: Option<u64>,

    /// Interval in seconds between periodic check-in messages sent via Telegram.
    /// When set, Ralph sends a status message every N seconds so the human
    /// knows it's still working. If `None`, no check-ins are sent.
    pub checkin_interval_seconds: Option<u64>,

    /// Telegram bot configuration.
    #[serde(default)]
    pub telegram: Option<TelegramBotConfig>,
}

impl RobotConfig {
    /// Validates the RObot config. Returns an error if enabled but misconfigured.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.enabled {
            return Ok(());
        }

        if self.timeout_seconds.is_none() {
            return Err(ConfigError::RobotMissingField {
                field: "RObot.timeout_seconds".to_string(),
                hint: "timeout_seconds is required when RObot is enabled".to_string(),
            });
        }

        // Bot token must be available from config, keychain, or env var
        if self.resolve_bot_token().is_none() {
            return Err(ConfigError::RobotMissingField {
                field: "RObot.telegram.bot_token".to_string(),
                hint: "Run `ralph bot onboard --telegram`, set RALPH_TELEGRAM_BOT_TOKEN env var, or set RObot.telegram.bot_token in config"
                    .to_string(),
            });
        }

        Ok(())
    }

    /// Resolves the bot token from multiple sources.
    ///
    /// Resolution order (highest to lowest priority):
    /// 1. `RALPH_TELEGRAM_BOT_TOKEN` environment variable
    /// 2. `RObot.telegram.bot_token` in config file (explicit project override)
    /// 3. OS keychain (service: "ralph", user: "telegram-bot-token")
    pub fn resolve_bot_token(&self) -> Option<String> {
        // 1. Env var (highest priority)
        let env_token = std::env::var("RALPH_TELEGRAM_BOT_TOKEN").ok();
        let config_token = self
            .telegram
            .as_ref()
            .and_then(|telegram| telegram.bot_token.clone());

        if cfg!(test) {
            return env_token.or(config_token);
        }

        env_token
            // 2. Config file (explicit override)
            .or(config_token)
            // 3. OS keychain (best effort)
            .or_else(|| {
                std::panic::catch_unwind(|| {
                    keyring::Entry::new("ralph", "telegram-bot-token")
                        .ok()
                        .and_then(|e| e.get_password().ok())
                })
                .ok()
                .flatten()
            })
    }

    /// Resolves the custom Telegram API URL from multiple sources.
    ///
    /// Resolution order (highest to lowest priority):
    /// 1. `RALPH_TELEGRAM_API_URL` environment variable
    /// 2. `RObot.telegram.api_url` in config file
    pub fn resolve_api_url(&self) -> Option<String> {
        std::env::var("RALPH_TELEGRAM_API_URL").ok().or_else(|| {
            self.telegram
                .as_ref()
                .and_then(|telegram| telegram.api_url.clone())
        })
    }
}

/// Telegram bot configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramBotConfig {
    /// Bot token. Optional if `RALPH_TELEGRAM_BOT_TOKEN` env var is set.
    pub bot_token: Option<String>,

    /// Custom Telegram Bot API URL. Optional; when set, all API requests
    /// are sent to this URL instead of the default `https://api.telegram.org`.
    /// Useful for targeting a local mock server (e.g., `telegram-test-api`)
    /// in CI/CD. Can also be set via `RALPH_TELEGRAM_API_URL` env var.
    pub api_url: Option<String>,
}
