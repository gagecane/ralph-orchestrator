//! CLI backend configuration and per-adapter settings.

use serde::{Deserialize, Serialize};

use super::defaults::default_true;

/// CLI backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Backend to use: "claude", "kiro", "gemini", "codex", "amp", "pi", or "custom".
    #[serde(default = "default_backend")]
    pub backend: String,

    /// Command override. Required for "custom" backend.
    /// For named backends, overrides the default binary path.
    pub command: Option<String>,

    /// How to pass prompts: "arg" or "stdin".
    #[serde(default = "default_prompt_mode")]
    pub prompt_mode: String,

    /// Execution mode when --interactive not specified.
    /// Values: "autonomous" (default), "interactive"
    #[serde(default = "default_mode")]
    pub default_mode: String,

    /// Idle timeout in seconds for interactive mode.
    /// Process is terminated after this many seconds of inactivity (no output AND no user input).
    /// Set to 0 to disable idle timeout.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u32,

    /// Custom arguments to pass to the CLI command (for backend: "custom").
    /// These are inserted before the prompt argument.
    #[serde(default)]
    pub args: Vec<String>,

    /// Custom prompt flag for arg mode (for backend: "custom").
    /// If None, defaults to "-p" for arg mode.
    #[serde(default)]
    pub prompt_flag: Option<String>,
}

fn default_backend() -> String {
    "claude".to_string()
}

fn default_prompt_mode() -> String {
    "arg".to_string()
}

fn default_mode() -> String {
    "autonomous".to_string()
}

fn default_idle_timeout() -> u32 {
    30 // 30 seconds per spec
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            command: None,
            prompt_mode: default_prompt_mode(),
            default_mode: default_mode(),
            idle_timeout_secs: default_idle_timeout(),
            args: Vec::new(),
            prompt_flag: None,
        }
    }
}

/// V1 adapter settings per backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdaptersConfig {
    /// Claude adapter settings.
    #[serde(default)]
    pub claude: AdapterSettings,

    /// Gemini adapter settings.
    #[serde(default)]
    pub gemini: AdapterSettings,

    /// Kiro adapter settings.
    #[serde(default)]
    pub kiro: AdapterSettings,

    /// Codex adapter settings.
    #[serde(default)]
    pub codex: AdapterSettings,

    /// Amp adapter settings.
    #[serde(default)]
    pub amp: AdapterSettings,
}

/// Per-adapter settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterSettings {
    /// CLI execution inactivity timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Include in auto-detection.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Tool permissions (DROPPED: CLI tool manages its own permissions).
    #[serde(default)]
    pub tool_permissions: Option<Vec<String>>,
}

fn default_timeout() -> u64 {
    300 // 5 minutes
}

impl Default for AdapterSettings {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            enabled: true,
            tool_permissions: None,
        }
    }
}
