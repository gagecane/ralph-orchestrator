//! Config and hats source parsing, overrides, and synchronous config loading.
//!
//! This module centralizes the CLI's handling of `-c` and `-H` flags, the
//! parsing of config override strings (`core.field=value`), and the
//! synchronous code path used by `resume` and `clean` to load `RalphConfig`
//! with overrides applied.
//!
//! The async loading path (which supports remote URLs and full preflight)
//! lives in `preflight::load_config_for_preflight` and is used by `run_command`.

use anyhow::Context;
use ralph_core::RalphConfig;
use std::path::PathBuf;
use tracing::{info, warn};

use crate::config_resolution;
use crate::workspace::default_config_path;

/// Source for core configuration.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Local file path (default behavior)
    File(PathBuf),
    /// Legacy builtin preset source (no longer valid for core config).
    ///
    /// Kept so we can emit actionable migration errors.
    Builtin(String),
    /// Remote URL (e.g., "http://example.com/ralph.core.yml")
    Remote(String),
    /// Config override (e.g., "core.scratchpad=.ralph/feature/scratchpad.md")
    Override { key: String, value: String },
}

impl ConfigSource {
    /// Parse a core config source string into its variant.
    ///
    /// Format:
    /// - `core.field=value` → Override (for core.* fields)
    /// - `builtin:preset-name` → Legacy builtin preset (rejected with migration message)
    /// - `http://...` or `https://...` → Remote URL
    /// - Anything else → File path
    pub(crate) fn parse(s: &str) -> Self {
        // Check for core.* override pattern first (prevents false positives on paths with '=')
        // Only treat as override if it starts with "core." AND contains '='
        if s.starts_with("core.")
            && let Some((key, value)) = s.split_once('=')
        {
            return ConfigSource::Override {
                key: key.to_string(),
                value: value.to_string(),
            };
        }

        if let Some(name) = s.strip_prefix("builtin:") {
            ConfigSource::Builtin(name.to_string())
        } else if s.starts_with("http://") || s.starts_with("https://") {
            ConfigSource::Remote(s.to_string())
        } else {
            ConfigSource::File(PathBuf::from(s))
        }
    }

    /// Convert back to CLI string representation for forwarding to subprocess.
    pub(crate) fn to_cli_string(&self) -> String {
        match self {
            ConfigSource::File(path) => path.display().to_string(),
            ConfigSource::Builtin(name) => format!("builtin:{}", name),
            ConfigSource::Remote(url) => url.clone(),
            ConfigSource::Override { key, value } => format!("{}={}", key, value),
        }
    }
}

/// Source for hat collection configuration.
#[derive(Debug, Clone)]
pub enum HatsSource {
    /// Local file path
    File(PathBuf),
    /// Builtin hat collection name (e.g., "builtin:code-assist")
    Builtin(String),
    /// Remote URL (e.g., "http://example.com/hats.yml")
    Remote(String),
}

impl HatsSource {
    /// Parse a hats source string into its variant.
    pub(crate) fn parse(s: &str) -> Self {
        if let Some(name) = s.strip_prefix("builtin:") {
            HatsSource::Builtin(name.to_string())
        } else if s.starts_with("http://") || s.starts_with("https://") {
            HatsSource::Remote(s.to_string())
        } else {
            HatsSource::File(PathBuf::from(s))
        }
    }

    /// Human-readable source label.
    pub fn label(&self) -> String {
        match self {
            HatsSource::File(path) => path.display().to_string(),
            HatsSource::Builtin(name) => format!("builtin:{}", name),
            HatsSource::Remote(url) => url.clone(),
        }
    }
}

/// Known core fields that can be overridden via CLI.
const KNOWN_CORE_FIELDS: &[&str] = &["scratchpad", "specs_dir"];

/// Applies CLI config overrides to the loaded configuration.
///
/// Overrides are in the format `core.field=value` and take precedence
/// over values from the config file.
pub(crate) fn apply_config_overrides(
    config: &mut RalphConfig,
    sources: &[ConfigSource],
) -> anyhow::Result<()> {
    for source in sources {
        if let ConfigSource::Override { key, value } = source {
            match key.as_str() {
                "core.scratchpad" => {
                    config.core.scratchpad.path = value.clone();
                }
                "core.specs_dir" => {
                    config.core.specs_dir = value.clone();
                }
                other => {
                    // Note: with core.* prefix requirement in parse(), this branch
                    // only handles unknown core.* fields
                    let field = other.strip_prefix("core.").unwrap_or(other);
                    warn!(
                        "Unknown core field '{}'. Known fields: {}",
                        field,
                        KNOWN_CORE_FIELDS.join(", ")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Ensures the scratchpad's parent directory exists, creating it if needed.
pub(crate) fn ensure_scratchpad_directory(config: &RalphConfig) -> anyhow::Result<()> {
    let scratchpad_path = config.core.resolve_path(&config.core.scratchpad.path);
    if let Some(parent) = scratchpad_path.parent()
        && !parent.exists()
    {
        info!("Creating scratchpad directory: {}", parent.display());
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Loads configuration from file sources with override support.
///
/// This is the common sync path used by resume_command and clean_command.
/// For the full async path (including Remote URLs), see run_command.
///
/// Returns the loaded config with overrides applied and workspace_root set.
pub(crate) fn load_config_with_overrides(
    config_sources: &[ConfigSource],
) -> anyhow::Result<RalphConfig> {
    let (primary_sources, overrides) = config_resolution::split_config_sources(config_sources);
    if primary_sources.len() > 1 {
        warn!("Multiple config sources specified, using first one. Others ignored.");
    }

    let (primary_value, primary_label, primary_uses_defaults) = match primary_sources.first() {
        Some(ConfigSource::File(path)) => {
            if path.exists() {
                let label = path.display().to_string();
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to load config from {}", label))?;
                let value = config_resolution::parse_yaml_value(&content, &label)?;
                (Some(value), label, false)
            } else {
                warn!("Config file {:?} not found, using defaults", path);
                (None, path.display().to_string(), false)
            }
        }
        Some(ConfigSource::Builtin(name)) => {
            anyhow::bail!(
                "`-c builtin:{name}` is no longer supported.\n\nBuiltin presets are now hat collections.\nUse:\n  ralph run -c ralph.yml -H builtin:{name}"
            );
        }
        Some(ConfigSource::Remote(url)) => {
            anyhow::bail!(
                "Remote core config sources are not supported for this command: {}",
                url
            );
        }
        Some(ConfigSource::Override { .. }) => unreachable!("Overrides are partitioned out"),
        None => {
            let default_path = default_config_path();
            if default_path.exists() {
                let label = default_path.display().to_string();
                let content = std::fs::read_to_string(&default_path)
                    .with_context(|| format!("Failed to load config from {}", label))?;
                let value = config_resolution::parse_yaml_value(&content, &label)?;
                (Some(value), label, false)
            } else {
                warn!(
                    "Config file {} not found, using defaults",
                    default_path.display()
                );
                (None, default_path.display().to_string(), true)
            }
        }
    };

    let user_layer = config_resolution::load_optional_user_config_value()?;
    let mut merged_value = config_resolution::default_core_value()?;
    if let Some((user_value, _)) = &user_layer {
        merged_value = config_resolution::merge_yaml_values(merged_value, user_value.clone())?;
    }
    if let Some(primary_value) = primary_value {
        merged_value = config_resolution::merge_yaml_values(merged_value, primary_value)?;
    }

    let merged_label = config_resolution::compose_core_label(
        user_layer.as_ref().map(|(_, label)| label.as_str()),
        &primary_label,
        primary_uses_defaults,
    );

    let mut config: RalphConfig = serde_yaml::from_value(merged_value)
        .with_context(|| format!("Failed to parse merged core config from {}", merged_label))?;

    config.normalize();

    // Set workspace_root to current directory
    config.core.workspace_root =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Apply CLI config overrides
    apply_config_overrides(&mut config, &overrides)?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CwdGuard;

    #[test]
    fn test_config_source_parse_builtin() {
        let source = ConfigSource::parse("builtin:code-assist");
        match source {
            ConfigSource::Builtin(name) => assert_eq!(name, "code-assist"),
            _ => panic!("Expected Builtin variant"),
        }
    }

    #[test]
    fn test_hats_source_parse_builtin() {
        let source = HatsSource::parse("builtin:code-assist");
        match source {
            HatsSource::Builtin(name) => assert_eq!(name, "code-assist"),
            _ => panic!("Expected Builtin variant"),
        }
    }

    #[test]
    fn test_hats_source_parse_file() {
        let source = HatsSource::parse("hats/feature.yml");
        match source {
            HatsSource::File(path) => {
                assert_eq!(path, std::path::PathBuf::from("hats/feature.yml"))
            }
            _ => panic!("Expected File variant"),
        }
    }

    #[test]
    fn test_config_source_parse_remote_https() {
        let source = ConfigSource::parse("https://example.com/preset.yml");
        match source {
            ConfigSource::Remote(url) => assert_eq!(url, "https://example.com/preset.yml"),
            _ => panic!("Expected Remote variant"),
        }
    }

    #[test]
    fn test_config_source_parse_remote_http() {
        let source = ConfigSource::parse("http://example.com/preset.yml");
        match source {
            ConfigSource::Remote(url) => assert_eq!(url, "http://example.com/preset.yml"),
            _ => panic!("Expected Remote variant"),
        }
    }

    #[test]
    fn test_config_source_parse_file() {
        let source = ConfigSource::parse("ralph.yml");
        match source {
            ConfigSource::File(path) => assert_eq!(path, std::path::PathBuf::from("ralph.yml")),
            _ => panic!("Expected File variant"),
        }
    }

    #[test]
    fn test_config_source_parse_override_scratchpad() {
        let source = ConfigSource::parse("core.scratchpad=.ralph/feature/scratchpad.md");
        match source {
            ConfigSource::Override { key, value } => {
                assert_eq!(key, "core.scratchpad");
                assert_eq!(value, ".ralph/feature/scratchpad.md");
            }
            _ => panic!("Expected Override variant"),
        }
    }

    #[test]
    fn test_config_source_parse_override_specs_dir() {
        let source = ConfigSource::parse("core.specs_dir=./my-specs/");
        match source {
            ConfigSource::Override { key, value } => {
                assert_eq!(key, "core.specs_dir");
                assert_eq!(value, "./my-specs/");
            }
            _ => panic!("Expected Override variant"),
        }
    }

    #[test]
    fn test_config_source_to_cli_string_roundtrips() {
        // File path
        let source = ConfigSource::File(PathBuf::from("ralph.yml"));
        assert_eq!(source.to_cli_string(), "ralph.yml");

        // Builtin (legacy)
        let source = ConfigSource::Builtin("code-assist".to_string());
        assert_eq!(source.to_cli_string(), "builtin:code-assist");

        // Remote URL
        let source = ConfigSource::Remote("https://example.com/ralph.yml".to_string());
        assert_eq!(source.to_cli_string(), "https://example.com/ralph.yml");

        // Override
        let source = ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".ralph/feature/scratchpad.md".to_string(),
        };
        assert_eq!(
            source.to_cli_string(),
            "core.scratchpad=.ralph/feature/scratchpad.md"
        );
    }

    #[test]
    fn test_config_source_parse_file_with_equals() {
        // Paths containing '=' but not starting with 'core.' should be treated as files
        let source = ConfigSource::parse("path/with=equals.yml");
        match source {
            ConfigSource::File(path) => {
                assert_eq!(path, std::path::PathBuf::from("path/with=equals.yml"))
            }
            _ => panic!("Expected File variant for path with equals sign"),
        }
    }

    #[test]
    fn test_config_source_parse_core_without_equals() {
        // "core.field" without '=' should be treated as a file path (will fail to load)
        let source = ConfigSource::parse("core.field");
        match source {
            ConfigSource::File(path) => assert_eq!(path, std::path::PathBuf::from("core.field")),
            _ => panic!("Expected File variant for core.field without ="),
        }
    }

    #[test]
    fn test_config_source_parse_non_core_with_equals_is_file() {
        // Non-core.* prefix with '=' should be treated as file path per spec
        let source = ConfigSource::parse("event_loop.max_iterations=5");
        match source {
            ConfigSource::File(path) => {
                assert_eq!(
                    path,
                    std::path::PathBuf::from("event_loop.max_iterations=5")
                )
            }
            _ => panic!("Expected File variant, not Override"),
        }
    }

    #[test]
    fn test_apply_config_overrides_scratchpad() {
        let mut config = RalphConfig::default();
        let sources = vec![ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratch.md".to_string(),
        }];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
    }

    #[test]
    fn test_apply_config_overrides_specs_dir() {
        let mut config = RalphConfig::default();
        let sources = vec![ConfigSource::Override {
            key: "core.specs_dir".to_string(),
            value: "./specifications/".to_string(),
        }];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.specs_dir, "./specifications/");
    }

    #[test]
    fn test_apply_config_overrides_multiple() {
        let mut config = RalphConfig::default();
        let sources = vec![
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratch.md".to_string(),
            },
            ConfigSource::Override {
                key: "core.specs_dir".to_string(),
                value: "./my-specs/".to_string(),
            },
        ];
        apply_config_overrides(&mut config, &sources).unwrap();
        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        assert_eq!(config.core.specs_dir, "./my-specs/");
    }

    #[test]
    fn test_apply_config_overrides_unknown_field() {
        // Unknown core.* fields should warn but not error
        let mut config = RalphConfig::default();
        let original_scratchpad = config.core.scratchpad.path.clone();
        let sources = vec![ConfigSource::Override {
            key: "core.unknown_field".to_string(),
            value: "some_value".to_string(),
        }];
        // Should not error
        apply_config_overrides(&mut config, &sources).unwrap();
        // Original values should be unchanged
        assert_eq!(config.core.scratchpad.path, original_scratchpad);
    }

    #[test]
    fn test_ensure_scratchpad_directory_creates_nested() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();

        config.core.scratchpad.path = "a/b/c/scratchpad.md".to_string();

        let result = ensure_scratchpad_directory(&config);
        assert!(result.is_ok());

        // Verify directory was created
        let expected_dir = temp_dir.path().join("a/b/c");
        assert!(expected_dir.exists());
    }

    #[test]
    fn test_ensure_scratchpad_directory_noop_when_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = RalphConfig::default();
        config.core.workspace_root = temp_dir.path().to_path_buf();

        // Pre-create the directory
        let subdir = temp_dir.path().join("existing");
        std::fs::create_dir_all(&subdir).unwrap();
        config.core.scratchpad.path = "existing/scratchpad.md".to_string();

        // Should succeed without error (no-op)
        let result = ensure_scratchpad_directory(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_partition_config_sources_separates_overrides() {
        let sources = [
            ConfigSource::File(PathBuf::from("ralph.yml")),
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratchpad.md".to_string(),
            },
            ConfigSource::Builtin("tdd".to_string()),
            ConfigSource::Override {
                key: "core.specs_dir".to_string(),
                value: "./specs/".to_string(),
            },
        ];

        let (primary, overrides): (Vec<_>, Vec<_>) = sources
            .iter()
            .partition(|s| !matches!(s, ConfigSource::Override { .. }));

        assert_eq!(primary.len(), 2); // File + Builtin
        assert_eq!(overrides.len(), 2); // Two overrides
        assert!(matches!(primary[0], ConfigSource::File(_)));
        assert!(matches!(primary[1], ConfigSource::Builtin(_)));
    }

    #[test]
    fn test_partition_config_sources_only_overrides() {
        let sources = [ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratchpad.md".to_string(),
        }];

        let (primary, overrides): (Vec<_>, Vec<_>) = sources
            .iter()
            .partition(|s| !matches!(s, ConfigSource::Override { .. }));

        assert_eq!(primary.len(), 0); // No primary sources
        assert_eq!(overrides.len(), 1); // One override
    }

    #[test]
    fn test_load_config_from_file_with_overrides() {
        // Integration test: load a real config file and apply overrides
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("test.yml");
        std::fs::write(
            &config_path,
            r"
cli:
  backend: claude
core:
  scratchpad: .agent/scratchpad.md
  specs_dir: ./specs/
",
        )
        .unwrap();

        let mut config = RalphConfig::from_file(&config_path).unwrap();
        assert_eq!(config.core.scratchpad.path, ".agent/scratchpad.md");

        // Apply override
        let overrides = vec![ConfigSource::Override {
            key: "core.scratchpad".to_string(),
            value: ".custom/scratch.md".to_string(),
        }];
        apply_config_overrides(&mut config, &overrides).unwrap();

        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        assert_eq!(config.core.specs_dir, "./specs/"); // Unchanged
    }

    #[test]
    fn test_load_config_with_overrides_applies_override_sources() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());
        let config_path = temp_dir.path().join("ralph.yml");
        std::fs::write(&config_path, "core:\n  scratchpad: .agent/scratchpad.md\n").unwrap();

        let sources = vec![
            ConfigSource::File(config_path),
            ConfigSource::Override {
                key: "core.scratchpad".to_string(),
                value: ".custom/scratch.md".to_string(),
            },
        ];

        let config = load_config_with_overrides(&sources).unwrap();

        assert_eq!(config.core.scratchpad.path, ".custom/scratch.md");
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }

    #[test]
    fn test_load_config_with_overrides_only_overrides_uses_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let sources = vec![ConfigSource::Override {
            key: "core.specs_dir".to_string(),
            value: "custom-specs".to_string(),
        }];

        let config = load_config_with_overrides(&sources).unwrap();

        assert_eq!(config.core.specs_dir, "custom-specs");
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }

    #[test]
    fn test_load_config_with_overrides_missing_file_falls_back_to_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let _cwd = CwdGuard::set(temp_dir.path());

        let sources = vec![ConfigSource::File(PathBuf::from("missing.yml"))];

        let config = load_config_with_overrides(&sources).unwrap();

        assert!(!config.core.scratchpad.path.is_empty());
        let expected_root = std::fs::canonicalize(temp_dir.path())
            .unwrap_or_else(|_| temp_dir.path().to_path_buf());
        let actual_root = std::fs::canonicalize(&config.core.workspace_root)
            .unwrap_or_else(|_| config.core.workspace_root.clone());
        assert_eq!(actual_root, expected_root);
    }
}
