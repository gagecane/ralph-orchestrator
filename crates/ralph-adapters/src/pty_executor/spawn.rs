//! PTY spawn-time environment helpers.
//!
//! Provides environment injection for Ralph runtime context so spawned CLI
//! tools can locate `ralph` on PATH and resolve the correct events file
//! regardless of the child's current working directory.

use portable_pty::CommandBuilder;
use std::env;

/// Injects Ralph runtime environment variables into a [`CommandBuilder`].
///
/// This ensures spawned CLI tools have:
/// - `PATH` prefixed with the directory containing the current `ralph` binary
/// - `RALPH_BIN` pointing at the current `ralph` executable
/// - `RALPH_WORKSPACE_ROOT` set to the workspace root
/// - `RALPH_EVENTS_FILE` resolved from `.ralph/current-events` (if present)
/// - `TMPDIR`/`TMP`/`TEMP` pinned to `/var/tmp` when available
pub(super) fn inject_ralph_runtime_env(
    cmd_builder: &mut CommandBuilder,
    workspace_root: &std::path::Path,
) {
    let Ok(current_exe) = env::current_exe() else {
        return;
    };
    let Some(bin_dir) = current_exe.parent() else {
        return;
    };

    let mut path_entries = vec![bin_dir.to_path_buf()];
    if let Some(existing_path) = env::var_os("PATH") {
        path_entries.extend(env::split_paths(&existing_path));
    }

    if let Ok(joined_path) = env::join_paths(path_entries) {
        cmd_builder.env("PATH", joined_path);
    }
    cmd_builder.env("RALPH_BIN", current_exe);
    cmd_builder.env("RALPH_WORKSPACE_ROOT", workspace_root);

    // Propagate RALPH_EVENTS_FILE so `ralph emit` from any CWD writes to the correct events file
    let marker = workspace_root.join(".ralph/current-events");
    if let Ok(relative) = std::fs::read_to_string(&marker) {
        let abs = workspace_root.join(relative.trim());
        cmd_builder.env("RALPH_EVENTS_FILE", abs);
    }

    if std::path::Path::new("/var/tmp").is_dir() {
        cmd_builder.env("TMPDIR", "/var/tmp");
        cmd_builder.env("TMP", "/var/tmp");
        cmd_builder.env("TEMP", "/var/tmp");
    }
}
