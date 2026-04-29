//! Feature flags for optional Ralph capabilities.

use serde::{Deserialize, Serialize};

/// Preflight check configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreflightConfig {
    /// Whether to run preflight checks before `ralph run`.
    #[serde(default)]
    pub enabled: bool,

    /// Whether to treat warnings as failures.
    #[serde(default)]
    pub strict: bool,

    /// Specific checks to skip (by name). Empty = run all checks.
    #[serde(default)]
    pub skip: Vec<String>,
}

/// Feature flags for optional Ralph capabilities.
///
/// Example configuration:
/// ```yaml
/// features:
///   parallel: true  # Enable parallel loops via git worktrees
///   auto_merge: false  # Auto-merge worktree branches on completion
///   preflight:
///     enabled: false      # Opt-in: run preflight checks before `ralph run`
///     strict: false       # Treat warnings as failures
///     skip: ["telegram"]  # Skip specific checks by name
///   loop_naming:
///     format: human-readable  # or "timestamp" for legacy format
///     max_length: 50
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesConfig {
    /// Whether parallel loops are enabled.
    ///
    /// When true (default), if another loop holds the lock, Ralph spawns
    /// a parallel loop in a git worktree. When false, Ralph errors instead.
    #[serde(default = "super::default_true")]
    pub parallel: bool,

    /// Whether to automatically merge worktree branches on completion.
    ///
    /// When false (default), completed worktree loops queue for manual merge.
    /// When true, Ralph automatically merges the worktree branch into the
    /// main branch after a parallel loop completes.
    #[serde(default)]
    pub auto_merge: bool,

    /// Loop naming configuration for worktree branches.
    ///
    /// Controls how loop IDs are generated for parallel loops.
    /// Default uses human-readable format: `fix-header-swift-peacock`
    /// Legacy timestamp format: `ralph-YYYYMMDD-HHMMSS-XXXX`
    #[serde(default)]
    pub loop_naming: crate::loop_name::LoopNamingConfig,

    /// Preflight check configuration.
    #[serde(default)]
    pub preflight: PreflightConfig,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            parallel: true,    // Parallel loops enabled by default
            auto_merge: false, // Auto-merge disabled by default for safety
            loop_naming: crate::loop_name::LoopNamingConfig::default(),
            preflight: PreflightConfig::default(),
        }
    }
}
