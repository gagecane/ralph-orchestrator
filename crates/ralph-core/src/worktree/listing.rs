//! Enumeration and inspection of git worktrees.
//!
//! Wrappers around `git worktree list --porcelain` plus a few small helpers
//! used by the lifecycle module to resolve branch and HEAD information for a
//! specific worktree path.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Worktree, WorktreeConfig, WorktreeError};

/// List all git worktrees in the repository.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository (can be any worktree)
///
/// # Returns
///
/// List of all worktrees, including the main worktree.
pub fn list_worktrees(repo_root: impl AsRef<Path>) -> Result<Vec<Worktree>, WorktreeError> {
    let repo_root = repo_root.as_ref();

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_worktree_list(&stdout)
}

/// Parse the porcelain output of `git worktree list`.
pub(crate) fn parse_worktree_list(output: &str) -> Result<Vec<Worktree>, WorktreeError> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if line.starts_with("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take()
                && !is_bare
            {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch
                        .take()
                        .unwrap_or_else(|| "(detached)".to_string()),
                    is_main: worktrees.is_empty(), // First one is main
                    head: current_head.take(),
                });
            }

            current_path = Some(PathBuf::from(line.strip_prefix("worktree ").unwrap()));
            current_head = None;
            current_branch = None;
            is_bare = false;
        } else if line.starts_with("HEAD ") {
            current_head = Some(line.strip_prefix("HEAD ").unwrap().to_string());
        } else if line.starts_with("branch ") {
            // Branch is in format "refs/heads/branch-name"
            let branch_ref = line.strip_prefix("branch ").unwrap();
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Don't forget the last one
    if let Some(path) = current_path
        && !is_bare
    {
        worktrees.push(Worktree {
            path,
            branch: current_branch.unwrap_or_else(|| "(detached)".to_string()),
            is_main: worktrees.is_empty(),
            head: current_head,
        });
    }

    Ok(worktrees)
}

/// Get the list of Ralph-specific worktrees (those with `ralph/` branches).
pub fn list_ralph_worktrees(repo_root: impl AsRef<Path>) -> Result<Vec<Worktree>, WorktreeError> {
    let all = list_worktrees(repo_root)?;
    Ok(all
        .into_iter()
        .filter(|wt| wt.branch.starts_with("ralph/"))
        .collect())
}

/// Check if a worktree exists for the given loop ID.
pub fn worktree_exists(
    repo_root: impl AsRef<Path>,
    loop_id: &str,
    config: &WorktreeConfig,
) -> bool {
    let worktree_path = config.worktree_path(repo_root.as_ref()).join(loop_id);
    worktree_path.exists()
}

/// Get the branch name for a worktree.
pub(crate) fn get_worktree_branch(worktree_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch != "HEAD" {
            return Some(branch);
        }
    }
    None
}

/// Get the HEAD commit SHA for a worktree.
pub(crate) fn get_head_commit(worktree_path: &Path) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree_path)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(WorktreeError::Git(stderr.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::lifecycle::create_worktree;
    use crate::worktree::testing::init_git_repo;
    use tempfile::TempDir;

    #[test]
    fn test_list_worktrees() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Initially just the main worktree
        let worktrees = list_worktrees(temp_dir.path()).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_main);

        // Add a worktree
        let config = WorktreeConfig::default();
        let _wt = create_worktree(temp_dir.path(), "loop-1", &config).unwrap();

        let worktrees = list_worktrees(temp_dir.path()).unwrap();
        assert_eq!(worktrees.len(), 2);
    }

    #[test]
    fn test_list_ralph_worktrees() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let _wt1 = create_worktree(temp_dir.path(), "loop-1", &config).unwrap();
        let _wt2 = create_worktree(temp_dir.path(), "loop-2", &config).unwrap();

        let ralph_worktrees = list_ralph_worktrees(temp_dir.path()).unwrap();
        assert_eq!(ralph_worktrees.len(), 2);
        assert!(
            ralph_worktrees
                .iter()
                .all(|wt| wt.branch.starts_with("ralph/"))
        );
    }

    #[test]
    fn test_worktree_exists() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "check-exists";

        assert!(!worktree_exists(temp_dir.path(), loop_id, &config));

        let _wt = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        assert!(worktree_exists(temp_dir.path(), loop_id, &config));
    }

    #[test]
    fn test_parse_worktree_list() {
        let output = r"worktree /path/to/main
HEAD abc123def
branch refs/heads/main

worktree /path/to/.worktrees/loop-1
HEAD def456ghi
branch refs/heads/ralph/loop-1

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(worktrees[0].branch, "main");
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].head, Some("abc123def".to_string()));

        assert_eq!(
            worktrees[1].path,
            PathBuf::from("/path/to/.worktrees/loop-1")
        );
        assert_eq!(worktrees[1].branch, "ralph/loop-1");
        assert!(!worktrees[1].is_main);
    }
}
