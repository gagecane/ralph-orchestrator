//! TUI configuration and memory-injection mode.

use serde::{Deserialize, Serialize};

/// TUI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Prefix key combination (e.g., "ctrl-a", "ctrl-b").
    #[serde(default = "default_prefix_key")]
    pub prefix_key: String,
}

/// Memory injection mode.
///
/// Controls how memories are injected into agent context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InjectMode {
    /// Ralph automatically injects memories at the start of each iteration.
    #[default]
    Auto,
    /// Agent must explicitly run `ralph memory search` to access memories.
    Manual,
    /// Memories feature is disabled.
    None,
}

impl std::fmt::Display for InjectMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Manual => write!(f, "manual"),
            Self::None => write!(f, "none"),
        }
    }
}

fn default_prefix_key() -> String {
    "ctrl-a".to_string()
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            prefix_key: default_prefix_key(),
        }
    }
}

impl TuiConfig {
    /// Parses the prefix_key string into KeyCode and KeyModifiers.
    /// Returns an error if the format is invalid.
    pub fn parse_prefix(
        &self,
    ) -> Result<(crossterm::event::KeyCode, crossterm::event::KeyModifiers), String> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let parts: Vec<&str> = self.prefix_key.split('-').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid prefix_key format: '{}'. Expected format: 'ctrl-<key>' (e.g., 'ctrl-a', 'ctrl-b')",
                self.prefix_key
            ));
        }

        let modifier = match parts[0].to_lowercase().as_str() {
            "ctrl" => KeyModifiers::CONTROL,
            _ => {
                return Err(format!(
                    "Invalid modifier: '{}'. Only 'ctrl' is supported (e.g., 'ctrl-a')",
                    parts[0]
                ));
            }
        };

        let key_str = parts[1];
        if key_str.len() != 1 {
            return Err(format!(
                "Invalid key: '{}'. Expected a single character (e.g., 'a', 'b')",
                key_str
            ));
        }

        let key_char = key_str.chars().next().unwrap();
        let key_code = KeyCode::Char(key_char);

        Ok((key_code, modifier))
    }
}
