//! Core paths, settings, and legacy v1 adapter configuration.

use serde::{Deserialize, Serialize};

use super::scratchpad::{ScratchpadConfig, deserialize_scratchpad_config};

/// Core paths and settings shared across all hats.
///
/// Per spec: "Core behaviors (always injected, can customize paths)"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// Scratchpad configuration (path and enabled flag).
    /// Accepts both plain string (legacy) and structured object.
    #[serde(default, deserialize_with = "deserialize_scratchpad_config")]
    pub scratchpad: ScratchpadConfig,

    /// Path to the specs directory (source of truth for requirements).
    #[serde(default = "default_specs_dir")]
    pub specs_dir: String,

    /// Guardrails injected into every prompt (core behaviors).
    ///
    /// Per spec: These are always present regardless of hat.
    #[serde(default = "default_guardrails")]
    pub guardrails: Vec<String>,

    /// Root directory for workspace-relative paths (.ralph/, specs, etc.).
    ///
    /// All relative paths (scratchpad, specs_dir, memories) are resolved relative
    /// to this directory. Defaults to the current working directory.
    ///
    /// This is especially important for E2E tests that run in isolated workspaces.
    #[serde(skip)]
    pub workspace_root: std::path::PathBuf,
}

pub(super) fn default_specs_dir() -> String {
    ".ralph/specs/".to_string()
}

pub(super) fn default_guardrails() -> Vec<String> {
    vec![
        "Fresh context each iteration - scratchpad is memory".to_string(),
        "Don't assume 'not implemented' - search first".to_string(),
        "Backpressure is law - tests/typecheck/lint/audit must pass".to_string(),
        "When behavior is runnable or user-facing, exercise the real app with the strongest available harness (Playwright, tmux, real CLI/API) and try at least one adversarial path before reporting done".to_string(),
        "Confidence protocol: score decisions 0-100. >80 proceed autonomously; 50-80 proceed + document in .ralph/agent/decisions.md; <50 choose safe default + document".to_string(),
        "Commit atomically - one logical change per commit, capture the why".to_string(),
    ]
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            scratchpad: ScratchpadConfig::default(),
            specs_dir: default_specs_dir(),
            guardrails: default_guardrails(),
            workspace_root: std::env::var("RALPH_WORKSPACE_ROOT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                }),
        }
    }
}

impl CoreConfig {
    /// Sets the workspace root for resolving relative paths.
    ///
    /// This is used by E2E tests to point to their isolated test workspace.
    pub fn with_workspace_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.workspace_root = root.into();
        self
    }

    /// Resolves a relative path against the workspace root.
    ///
    /// If the path is already absolute, it is returned as-is.
    /// Otherwise, it is joined with the workspace root.
    pub fn resolve_path(&self, relative: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(relative);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
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
    #[serde(default = "super::default_true")]
    pub enabled: bool,

    /// Tool permissions (DROPPED: CLI tool manages its own permissions).
    #[serde(default)]
    pub tool_permissions: Option<Vec<String>>,
}

pub(super) fn default_timeout() -> u64 {
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
