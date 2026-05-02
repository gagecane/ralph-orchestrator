//! Git worktree management for parallel Ralph loops.
//!
//! Provides filesystem isolation for concurrent loops using git worktrees.
//! Each parallel loop gets its own working directory with full filesystem
//! isolation, sharing only `.git` history. Conflicts are resolved at merge time.
//!
//! The module is organised by responsibility:
//!
//! - [`lifecycle`] — create and remove worktrees
//! - [`listing`] — enumerate and inspect existing worktrees
//! - [`gitignore`] — keep the worktree directory out of git
//! - [`sync`] — copy untracked/unstaged files into a fresh worktree
//!
//! All public functions and types are re-exported from this module so callers
//! can continue to use `ralph_core::worktree::create_worktree` etc.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::worktree::{Worktree, WorktreeConfig, create_worktree, remove_worktree, list_worktrees};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = WorktreeConfig::default();
//!
//!     // Create worktree for a parallel loop
//!     let worktree = create_worktree(".", "ralph-20250124-a3f2", &config)?;
//!     println!("Created worktree at: {}", worktree.path.display());
//!
//!     // List all worktrees
//!     let worktrees = list_worktrees(".")?;
//!     for wt in worktrees {
//!         println!("  {}: {}", wt.branch, wt.path.display());
//!     }
//!
//!     // Clean up when done
//!     remove_worktree(".", &worktree.path)?;
//!     Ok(())
//! }
//! ```

use std::io;
use std::path::{Path, PathBuf};

pub mod gitignore;
pub mod lifecycle;
pub mod listing;
pub mod sync;

#[cfg(test)]
mod testing;

pub use gitignore::ensure_gitignore;
pub use lifecycle::{create_worktree, remove_worktree};
pub use listing::{list_ralph_worktrees, list_worktrees, worktree_exists};
pub use sync::sync_working_directory_to_worktree;

/// Configuration for worktree operations.
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    /// Directory where worktrees are created (default: `.worktrees`).
    pub worktree_dir: PathBuf,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            worktree_dir: PathBuf::from(".worktrees"),
        }
    }
}

impl WorktreeConfig {
    /// Create config with custom worktree directory.
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            worktree_dir: dir.into(),
        }
    }

    /// Get the absolute path to worktree directory relative to repo root.
    pub fn worktree_path(&self, repo_root: &Path) -> PathBuf {
        if self.worktree_dir.is_absolute() {
            self.worktree_dir.clone()
        } else {
            repo_root.join(&self.worktree_dir)
        }
    }
}

/// Information about a git worktree.
#[derive(Debug, Clone)]
pub struct Worktree {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,

    /// The branch checked out in this worktree.
    pub branch: String,

    /// Whether this is the main worktree.
    pub is_main: bool,

    /// HEAD commit (if available).
    pub head: Option<String>,
}

/// Statistics about files synced to a worktree.
#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    /// Number of untracked files copied.
    pub untracked_copied: usize,
    /// Number of modified (unstaged) files copied.
    pub modified_copied: usize,
    /// Number of files skipped (e.g., no longer exists).
    pub skipped: usize,
    /// Number of files that failed to copy.
    pub errors: usize,
}

/// Errors that can occur during worktree operations.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Git command failed.
    #[error("Git command failed: {0}")]
    Git(String),

    /// Worktree already exists.
    #[error("Worktree already exists: {0}")]
    AlreadyExists(String),

    /// Worktree not found.
    #[error("Worktree not found: {0}")]
    NotFound(String),

    /// Not a git repository.
    #[error("Not a git repository: {0}")]
    NotARepo(String),

    /// Branch already exists.
    #[error("Branch already exists: {0}")]
    BranchExists(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_worktree_config_default() {
        let config = WorktreeConfig::default();
        assert_eq!(config.worktree_dir, PathBuf::from(".worktrees"));
    }

    #[test]
    fn test_worktree_config_path() {
        let config = WorktreeConfig::default();
        let repo = Path::new("/repo");
        assert_eq!(
            config.worktree_path(repo),
            PathBuf::from("/repo/.worktrees")
        );

        let absolute_config = WorktreeConfig::with_dir("/tmp/worktrees");
        assert_eq!(
            absolute_config.worktree_path(repo),
            PathBuf::from("/tmp/worktrees")
        );
    }
}
