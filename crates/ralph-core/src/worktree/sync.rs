//! Sync working-directory files (untracked + unstaged) into a fresh worktree.
//!
//! A freshly created git worktree only carries committed history. For Ralph's
//! parallel loop workflow we also want WIP changes and untracked files to be
//! available in the new worktree so loops can reason about the same working
//! state as the main checkout. This module queries git for those files and
//! copies them over, preserving symlinks and binary content.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{SyncStats, WorktreeConfig, WorktreeError};

/// Get list of untracked files in the repository.
///
/// Uses `git ls-files --others --exclude-standard` to get files that are:
/// - Not tracked by git
/// - Not ignored by .gitignore
pub(crate) fn get_untracked_files(repo_root: &Path) -> Result<Vec<PathBuf>, WorktreeError> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Get list of tracked files with unstaged modifications.
///
/// Uses `git diff --name-only` to get files that have been modified
/// but not yet staged for commit.
pub(crate) fn get_unstaged_modified_files(repo_root: &Path) -> Result<Vec<PathBuf>, WorktreeError> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Copy a file from repo to worktree, preserving directory structure.
///
/// Creates parent directories as needed. Handles symlinks on Unix.
/// Returns Ok(false) if the source file no longer exists (race condition).
fn copy_file_with_structure(
    repo_root: &Path,
    worktree_path: &Path,
    relative_path: &Path,
) -> Result<bool, WorktreeError> {
    let source = repo_root.join(relative_path);
    let dest = worktree_path.join(relative_path);

    // Skip if source no longer exists (race condition)
    if !source.exists() && !source.is_symlink() {
        return Ok(false);
    }

    // Create parent directories
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    // Handle symlinks on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs as unix_fs;
        if source.is_symlink() {
            let link_target = fs::read_link(&source)?;
            // Remove existing file/symlink if present
            if dest.exists() || dest.is_symlink() {
                fs::remove_file(&dest)?;
            }
            unix_fs::symlink(&link_target, &dest)?;
            return Ok(true);
        }
    }

    // Copy regular file (handles binary files correctly)
    fs::copy(&source, &dest)?;
    Ok(true)
}

/// Sync untracked and unstaged files from the main repo to a worktree.
///
/// This copies files that are not committed to git, ensuring that WIP files
/// and uncommitted changes are available in the worktree for parallel loops.
///
/// # Exclusions
///
/// - `.git/` directory (never copied)
/// - The worktree directory itself (e.g., `.worktrees/`)
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_path` - Path to the target worktree
/// * `config` - Worktree configuration (for determining exclusion paths)
///
/// # Returns
///
/// Statistics about what was synced.
pub fn sync_working_directory_to_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    config: &WorktreeConfig,
) -> Result<SyncStats, WorktreeError> {
    let mut stats = SyncStats::default();

    // Get the worktree directory name for exclusion
    let worktree_dir = &config.worktree_dir;

    // Helper to check if a path should be excluded
    let should_exclude = |path: &Path| -> bool {
        let path_str = path.to_string_lossy();
        // Exclude .git directory
        if path_str.starts_with(".git/") || path_str == ".git" {
            return true;
        }
        // Exclude the worktree directory itself
        let worktree_dir_str = worktree_dir.to_string_lossy();
        if path_str.starts_with(&*worktree_dir_str)
            || path_str.starts_with(&format!("{}/", worktree_dir_str))
        {
            return true;
        }
        false
    };

    // Get untracked files
    let untracked = get_untracked_files(repo_root)?;
    for file in untracked {
        if should_exclude(&file) {
            stats.skipped += 1;
            continue;
        }
        match copy_file_with_structure(repo_root, worktree_path, &file) {
            Ok(true) => {
                tracing::trace!("Copied untracked file: {}", file.display());
                stats.untracked_copied += 1;
            }
            Ok(false) => {
                stats.skipped += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to copy untracked file {}: {}", file.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Get unstaged modified files
    let modified = get_unstaged_modified_files(repo_root)?;
    for file in modified {
        if should_exclude(&file) {
            stats.skipped += 1;
            continue;
        }
        match copy_file_with_structure(repo_root, worktree_path, &file) {
            Ok(true) => {
                tracing::trace!("Copied modified file: {}", file.display());
                stats.modified_copied += 1;
            }
            Ok(false) => {
                stats.skipped += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to copy modified file {}: {}", file.display(), e);
                stats.errors += 1;
            }
        }
    }

    tracing::debug!(
        "Synced {} untracked and {} modified files to worktree ({} skipped, {} errors)",
        stats.untracked_copied,
        stats.modified_copied,
        stats.skipped,
        stats.errors
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::lifecycle::create_worktree;
    use crate::worktree::testing::init_git_repo;
    use tempfile::TempDir;

    #[test]
    fn test_get_untracked_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create untracked files
        fs::write(temp_dir.path().join("untracked1.txt"), "content1").unwrap();
        fs::write(temp_dir.path().join("untracked2.txt"), "content2").unwrap();

        let untracked = get_untracked_files(temp_dir.path()).unwrap();
        assert_eq!(untracked.len(), 2);
        assert!(untracked.contains(&PathBuf::from("untracked1.txt")));
        assert!(untracked.contains(&PathBuf::from("untracked2.txt")));
    }

    #[test]
    fn test_get_unstaged_modified_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Modify a tracked file without staging
        fs::write(temp_dir.path().join("README.md"), "# Modified").unwrap();

        let modified = get_unstaged_modified_files(temp_dir.path()).unwrap();
        assert_eq!(modified.len(), 1);
        assert!(modified.contains(&PathBuf::from("README.md")));
    }

    #[test]
    fn test_sync_untracked_files_to_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create an untracked file
        fs::write(temp_dir.path().join("new_file.txt"), "untracked content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-untracked";

        // Create worktree - should sync untracked file
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify untracked file was copied
        let synced_file = worktree.path.join("new_file.txt");
        assert!(synced_file.exists());
        assert_eq!(
            fs::read_to_string(&synced_file).unwrap(),
            "untracked content"
        );
    }

    #[test]
    fn test_sync_unstaged_changes_to_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Modify a tracked file without staging
        fs::write(temp_dir.path().join("README.md"), "# Modified Content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-modified";

        // Create worktree - should sync modified file
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify modified content was copied (overwrote the committed version)
        let synced_file = worktree.path.join("README.md");
        assert!(synced_file.exists());
        assert_eq!(
            fs::read_to_string(&synced_file).unwrap(),
            "# Modified Content"
        );
    }

    #[test]
    fn test_sync_respects_gitignore() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Add a pattern to .gitignore
        fs::write(temp_dir.path().join(".gitignore"), "*.log\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        // Create an ignored file
        fs::write(temp_dir.path().join("debug.log"), "log content").unwrap();
        // Create a non-ignored file
        fs::write(temp_dir.path().join("valid.txt"), "valid content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-gitignore";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Ignored file should NOT be copied (git ls-files --others --exclude-standard respects .gitignore)
        assert!(!worktree.path.join("debug.log").exists());
        // Non-ignored file should be copied
        assert!(worktree.path.join("valid.txt").exists());
    }

    #[test]
    fn test_sync_excludes_worktrees_directory() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create an untracked file in the worktrees directory manually
        let worktrees_dir = temp_dir.path().join(".worktrees");
        fs::create_dir_all(&worktrees_dir).unwrap();
        fs::write(worktrees_dir.join("should_not_sync.txt"), "content").unwrap();

        // Create a normal untracked file
        fs::write(temp_dir.path().join("should_sync.txt"), "content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-exclude-worktrees";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Normal file should be synced
        assert!(worktree.path.join("should_sync.txt").exists());
        // The .worktrees directory should NOT be synced into itself
        // (this would cause recursion issues)
        assert!(
            !worktree
                .path
                .join(".worktrees/should_not_sync.txt")
                .exists()
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_sync_preserves_symlinks() {
        use std::os::unix::fs as unix_fs;

        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create a target file
        fs::write(temp_dir.path().join("target.txt"), "target content").unwrap();
        Command::new("git")
            .args(["add", "target.txt"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add target"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        // Create an untracked symlink
        unix_fs::symlink("target.txt", temp_dir.path().join("link.txt")).unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-symlinks";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify symlink was preserved
        let synced_link = worktree.path.join("link.txt");
        assert!(synced_link.is_symlink());
        assert_eq!(
            fs::read_link(&synced_link).unwrap(),
            PathBuf::from("target.txt")
        );
    }

    #[test]
    fn test_sync_handles_binary_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create a binary file (PNG header bytes)
        let binary_content: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        ];
        fs::write(temp_dir.path().join("image.png"), &binary_content).unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-binary";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify binary file was copied correctly
        let synced_file = worktree.path.join("image.png");
        assert!(synced_file.exists());
        assert_eq!(fs::read(&synced_file).unwrap(), binary_content);
    }

    #[test]
    fn test_sync_handles_nested_directories() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create nested untracked files
        let nested_dir = temp_dir.path().join("src/components/nested");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(nested_dir.join("deep.txt"), "deep content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-nested";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify nested file was copied with correct directory structure
        let synced_file = worktree.path.join("src/components/nested/deep.txt");
        assert!(synced_file.exists());
        assert_eq!(fs::read_to_string(&synced_file).unwrap(), "deep content");
    }

    #[test]
    fn test_sync_stats_returned() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create untracked files
        fs::write(temp_dir.path().join("untracked1.txt"), "content").unwrap();
        fs::write(temp_dir.path().join("untracked2.txt"), "content").unwrap();

        // Modify a tracked file
        fs::write(temp_dir.path().join("README.md"), "# Modified").unwrap();

        let config = WorktreeConfig::default();

        // Test sync_working_directory_to_worktree directly
        let worktree_path = temp_dir.path().join(".worktrees/stats-test");
        fs::create_dir_all(&worktree_path).unwrap();

        let stats =
            sync_working_directory_to_worktree(temp_dir.path(), &worktree_path, &config).unwrap();

        assert_eq!(stats.untracked_copied, 2);
        assert_eq!(stats.modified_copied, 1);
        assert_eq!(stats.errors, 0);
    }
}
