//! Worktree creation and teardown.
//!
//! Functions for bringing worktrees into existence and cleaning them up,
//! including deletion of the associated `ralph/*` branch.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::listing::{get_head_commit, get_worktree_branch};
use super::sync::sync_working_directory_to_worktree;
use super::{Worktree, WorktreeConfig, WorktreeError};

/// Create a new worktree for a parallel Ralph loop.
///
/// Creates a new branch and worktree at `{config.worktree_dir}/{loop_id}`.
/// The branch is created from HEAD of the current branch.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `loop_id` - Unique identifier for the loop (e.g., "ralph-20250124-a3f2")
/// * `config` - Worktree configuration
///
/// # Returns
///
/// Information about the created worktree.
pub fn create_worktree(
    repo_root: impl AsRef<Path>,
    loop_id: &str,
    config: &WorktreeConfig,
) -> Result<Worktree, WorktreeError> {
    let repo_root = repo_root.as_ref();

    // Verify this is a git repository
    if !repo_root.join(".git").exists() && !repo_root.join(".git").is_file() {
        return Err(WorktreeError::NotARepo(
            repo_root.to_string_lossy().to_string(),
        ));
    }

    let worktree_base = config.worktree_path(repo_root);
    let worktree_path = worktree_base.join(loop_id);
    let branch_name = format!("ralph/{loop_id}");

    // Check if worktree already exists
    if worktree_path.exists() {
        return Err(WorktreeError::AlreadyExists(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    // Ensure worktree directory exists
    fs::create_dir_all(&worktree_base)?;

    // Create worktree with new branch
    // git worktree add -b <branch> <path>
    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_path)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for specific error cases
        if stderr.contains("already exists") {
            if stderr.contains("branch") {
                return Err(WorktreeError::BranchExists(branch_name));
            }
            return Err(WorktreeError::AlreadyExists(
                worktree_path.to_string_lossy().to_string(),
            ));
        }

        return Err(WorktreeError::Git(stderr.to_string()));
    }

    // Sync untracked files and unstaged changes
    let sync_stats = sync_working_directory_to_worktree(repo_root, &worktree_path, config)?;

    if sync_stats.errors > 0 {
        tracing::warn!(
            "Some files failed to sync to worktree: {} errors",
            sync_stats.errors
        );
    }

    // Get the HEAD commit
    let head = get_head_commit(&worktree_path).ok();

    tracing::debug!(
        "Created worktree at {} on branch {} (synced {} untracked, {} modified files)",
        worktree_path.display(),
        branch_name,
        sync_stats.untracked_copied,
        sync_stats.modified_copied
    );

    Ok(Worktree {
        path: worktree_path,
        branch: branch_name,
        is_main: false,
        head,
    })
}

/// Remove a worktree and optionally its branch.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_path` - Path to the worktree to remove
///
/// # Note
///
/// This also deletes the associated branch if it exists.
pub fn remove_worktree(
    repo_root: impl AsRef<Path>,
    worktree_path: impl AsRef<Path>,
) -> Result<(), WorktreeError> {
    let repo_root = repo_root.as_ref();
    let worktree_path = worktree_path.as_ref();

    if !worktree_path.exists() {
        return Err(WorktreeError::NotFound(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    // Get the branch name before removing
    let branch = get_worktree_branch(worktree_path);

    // Remove the worktree (--force handles uncommitted changes)
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    // Delete the branch if it was a ralph/* branch
    if let Some(branch) = branch
        && branch.starts_with("ralph/")
    {
        let output = Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(repo_root)
            .output()?;

        if !output.status.success() {
            // Non-fatal: branch might already be deleted
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::debug!("Failed to delete branch {}: {}", branch, stderr);
        }
    }

    // Prune worktree refs
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output();

    tracing::debug!("Removed worktree at {}", worktree_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::testing::init_git_repo;
    use tempfile::TempDir;

    #[test]
    fn test_create_and_remove_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "test-loop-123";

        // Create worktree
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        assert!(worktree.path.exists());
        assert_eq!(worktree.branch, "ralph/test-loop-123");
        assert!(!worktree.is_main);
        assert!(worktree.head.is_some());

        // Verify README was copied
        assert!(worktree.path.join("README.md").exists());

        // Remove worktree
        remove_worktree(temp_dir.path(), &worktree.path).unwrap();
        assert!(!worktree.path.exists());
    }

    #[test]
    fn test_create_worktree_already_exists() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "duplicate";

        // Create first worktree
        let _wt = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Try to create duplicate
        let result = create_worktree(temp_dir.path(), loop_id, &config);
        assert!(matches!(result, Err(WorktreeError::AlreadyExists(_))));
    }

    #[test]
    fn test_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        // Don't init git

        let config = WorktreeConfig::default();
        let result = create_worktree(temp_dir.path(), "loop-1", &config);

        assert!(matches!(result, Err(WorktreeError::NotARepo(_))));
    }

    #[test]
    fn test_remove_nonexistent_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let result = remove_worktree(temp_dir.path(), temp_dir.path().join("nonexistent"));

        assert!(matches!(result, Err(WorktreeError::NotFound(_))));
    }
}
