//! Shared test helpers for worktree submodules.

#![cfg(test)]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Initialise a minimal git repository with an initial commit.
///
/// The worktree submodules all need a real git repo on disk to exercise
/// `git worktree` commands; this helper centralises the boilerplate.
pub(crate) fn init_git_repo(dir: &Path) {
    Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(dir)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.local"])
        .current_dir(dir)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .output()
        .unwrap();

    // Create initial commit (required for worktrees)
    fs::write(dir.join("README.md"), "# Test").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(dir)
        .output()
        .unwrap();
}
