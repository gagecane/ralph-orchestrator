//! Shared default-value functions referenced by `#[serde(default = ...)]` attributes.
//!
//! These live in their own module so that any submodule can reference them by
//! path (`crate::config::defaults::default_true`) without pulling the rest of
//! the config graph through circular imports.

pub(super) fn default_true() -> bool {
    true
}
