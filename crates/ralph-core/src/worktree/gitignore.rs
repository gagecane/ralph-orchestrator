//! `.gitignore` coordination for the worktree directory.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use super::WorktreeError;

/// Ensure the worktree directory is in `.gitignore`.
///
/// Appends the pattern to `.gitignore` if not already present.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_dir` - The worktree directory pattern to ignore (e.g., ".worktrees")
pub fn ensure_gitignore(
    repo_root: impl AsRef<Path>,
    worktree_dir: &str,
) -> Result<(), WorktreeError> {
    let repo_root = repo_root.as_ref();
    let gitignore_path = repo_root.join(".gitignore");

    // Pattern to add (with trailing slash for directory)
    let pattern = if worktree_dir.ends_with('/') {
        worktree_dir.to_string()
    } else {
        format!("{}/", worktree_dir)
    };

    // Check if pattern already exists
    if gitignore_path.exists() {
        let file = File::open(&gitignore_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();

            // Check if this line matches our pattern (with or without trailing slash)
            if trimmed == pattern || trimmed == pattern.trim_end_matches('/') {
                tracing::debug!("Pattern {} already in .gitignore", pattern);
                return Ok(());
            }
        }
    }

    // Append the pattern
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)?;

    // Add newline before if file exists and doesn't end with newline
    if gitignore_path.exists() {
        let contents = fs::read_to_string(&gitignore_path)?;
        if !contents.is_empty() && !contents.ends_with('\n') {
            writeln!(file)?;
        }
    }

    writeln!(file, "{}", pattern)?;

    tracing::debug!("Added {} to .gitignore", pattern);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ensure_gitignore_new_file() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        assert!(!gitignore.exists());

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        assert!(gitignore.exists());
        let contents = fs::read_to_string(&gitignore).unwrap();
        assert!(contents.contains(".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        fs::write(&gitignore, "node_modules/\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        assert!(contents.contains("node_modules/"));
        assert!(contents.contains(".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_already_present() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        fs::write(&gitignore, ".worktrees/\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        // Should only appear once
        assert_eq!(contents.matches(".worktrees/").count(), 1);
    }

    #[test]
    fn test_ensure_gitignore_without_trailing_slash() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        // Existing pattern without trailing slash
        fs::write(&gitignore, ".worktrees\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        // Should not add duplicate
        assert!(!contents.contains(".worktrees/\n.worktrees/"));
    }
}
