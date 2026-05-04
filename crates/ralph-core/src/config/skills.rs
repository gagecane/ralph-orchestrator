//! Skills discovery and injection configuration.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::defaults::default_true;

/// Skills configuration.
///
/// Controls the skill discovery and injection system that makes tool
/// knowledge and domain expertise available to agents during loops.
///
/// Skills use a two-tier injection model: a compact skill index is always
/// present in every prompt, and the agent loads full skill content on demand
/// via `ralph tools skill load <name>`.
///
/// Example configuration:
/// ```yaml
/// skills:
///   enabled: true
///   dirs:
///     - ".claude/skills"
///   overrides:
///     pdd:
///       enabled: false
///     memories:
///       auto_inject: true
///       hats: ["ralph"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Whether the skills system is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Directories to scan for skill files.
    /// Relative paths resolved against workspace root.
    #[serde(default)]
    pub dirs: Vec<PathBuf>,

    /// Per-skill overrides keyed by skill name.
    #[serde(default)]
    pub overrides: HashMap<String, SkillOverride>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Skills enabled by default
            dirs: vec![],
            overrides: HashMap::new(),
        }
    }
}

/// Per-skill configuration override.
///
/// Allows enabling/disabling individual skills and overriding their
/// frontmatter fields (hats, backends, tags, auto_inject).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillOverride {
    /// Disable a discovered skill.
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Restrict skill to specific hats.
    #[serde(default)]
    pub hats: Vec<String>,

    /// Restrict skill to specific backends.
    #[serde(default)]
    pub backends: Vec<String>,

    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Inject full content into prompt (not just index entry).
    #[serde(default)]
    pub auto_inject: Option<bool>,
}
