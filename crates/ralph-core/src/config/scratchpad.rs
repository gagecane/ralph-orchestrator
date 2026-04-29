//! Scratchpad configuration and custom deserializers.

use serde::{Deserialize, Deserializer, Serialize};

/// Scratchpad configuration with enabled flag and path.
///
/// Supports both plain string (legacy) and structured object in YAML:
/// ```yaml
/// # Legacy (plain string) — treated as { enabled: true, path: "..." }
/// core:
///   scratchpad: ".ralph/agent/scratchpad.md"
///
/// # Structured object
/// core:
///   scratchpad:
///     enabled: true
///     path: .ralph/agent/scratchpad.md
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchpadConfig {
    #[serde(default = "scratchpad_enabled_default")]
    pub enabled: bool,

    #[serde(default = "default_scratchpad_path")]
    pub path: String,
}

pub(super) fn scratchpad_enabled_default() -> bool {
    true
}

pub(super) fn default_scratchpad_path() -> String {
    ".ralph/agent/scratchpad.md".to_string()
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            enabled: scratchpad_enabled_default(),
            path: default_scratchpad_path(),
        }
    }
}

impl ScratchpadConfig {
    /// Resolves the effective scratchpad config for a hat run.
    ///
    /// Resolution order: hat override → global core config → defaults.
    pub fn resolve(
        hat_config: Option<&ScratchpadConfig>,
        global: &ScratchpadConfig,
    ) -> ScratchpadConfig {
        match hat_config {
            Some(override_config) => override_config.clone(),
            None => global.clone(),
        }
    }
}

/// Custom deserializer that accepts both a plain string and a structured object.
///
/// - Plain string → `ScratchpadConfig { enabled: true, path: <string> }`
/// - Map → normal `ScratchpadConfig` deserialization
pub(super) fn deserialize_scratchpad_config<'de, D>(
    deserializer: D,
) -> Result<ScratchpadConfig, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct ScratchpadConfigVisitor;

    impl<'de> de::Visitor<'de> for ScratchpadConfigVisitor {
        type Value = ScratchpadConfig;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a scratchpad config object")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<ScratchpadConfig, E> {
            Ok(ScratchpadConfig {
                enabled: true,
                path: value.to_string(),
            })
        }

        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<ScratchpadConfig, M::Error> {
            Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(ScratchpadConfigVisitor)
}

/// Custom deserializer for optional scratchpad config on hats.
///
/// Handles: absent (None), plain string, or structured object.
pub(super) fn deserialize_optional_scratchpad_config<'de, D>(
    deserializer: D,
) -> Result<Option<ScratchpadConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct OptionalScratchpadConfigVisitor;

    impl<'de> de::Visitor<'de> for OptionalScratchpadConfigVisitor {
        type Value = Option<ScratchpadConfig>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null, a string, or a scratchpad config object")
        }

        fn visit_none<E: de::Error>(self) -> Result<Option<ScratchpadConfig>, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Option<ScratchpadConfig>, E> {
            Ok(None)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Option<ScratchpadConfig>, E> {
            Ok(Some(ScratchpadConfig {
                enabled: true,
                path: value.to_string(),
            }))
        }

        fn visit_map<M: de::MapAccess<'de>>(
            self,
            map: M,
        ) -> Result<Option<ScratchpadConfig>, M::Error> {
            let config: ScratchpadConfig =
                Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(config))
        }
    }

    deserializer.deserialize_any(OptionalScratchpadConfigVisitor)
}
