//! Workspace discovery and path resolution helpers.
//!
//! These helpers centralize the logic for locating the Ralph workspace root,
//! the default config path, and workspace-relative marker files. They are
//! shared across many CLI subcommands (emit, events, memory, task_cli, etc.).

use std::path::{Path, PathBuf};

/// Returns the default config source path.
///
/// `RALPH_CONFIG` (if set) is used before the hardcoded fallback to `ralph.yml`.
pub(crate) fn default_config_path() -> PathBuf {
    if let Ok(value) = std::env::var("RALPH_CONFIG")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }

    PathBuf::from("ralph.yml")
}

pub(crate) fn resolve_workspace_root(root: Option<&PathBuf>) -> PathBuf {
    if let Some(root) = root {
        return root.clone();
    }

    if let Ok(value) = std::env::var("RALPH_WORKSPACE_ROOT")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    discover_workspace_root(&cwd).unwrap_or(cwd)
}

pub(crate) fn resolve_path_from_workspace(
    path: impl AsRef<Path>,
    root: Option<&PathBuf>,
) -> PathBuf {
    resolve_workspace_root(root).join(path)
}

pub(crate) fn urgent_steer_path_from_workspace(root: Option<&PathBuf>) -> PathBuf {
    resolve_workspace_root(root).join(".ralph/urgent-steer.json")
}

pub(crate) fn discover_workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let has_ralph = dir.join(".ralph").is_dir();
        let has_git = dir.join(".git").exists();
        if has_ralph || has_git {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

pub(crate) fn resolve_marker_target(workspace_root: &Path, marker_value: &str) -> PathBuf {
    let path = PathBuf::from(marker_value.trim());
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_workspace_root_discovers_ancestor_ralph_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("ralph dir");
        let nested = temp_dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("nested dir");

        assert_eq!(
            discover_workspace_root(&nested),
            Some(temp_dir.path().to_path_buf())
        );
    }
}
