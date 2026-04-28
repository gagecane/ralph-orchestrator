//! Environment setup for child CLI processes.
//!
//! Injects Ralph-specific environment variables (`RALPH_BIN`,
//! `RALPH_WORKSPACE_ROOT`, `PATH` prepended with the current binary's
//! directory, and `TMPDIR` overrides on hosts that have `/var/tmp`) so
//! backends can locate helper tools.

use std::env;
use tokio::process::Command;

/// Sets Ralph-specific environment variables on `command`.
///
/// Silently no-ops if the current executable path can't be determined — this
/// keeps the executor usable in odd environments (e.g. some CI sandboxes)
/// without hard-failing.
pub(super) fn inject_ralph_runtime_env(command: &mut Command, workspace_root: &std::path::Path) {
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
        command.env("PATH", joined_path);
    }
    command.env("RALPH_BIN", &current_exe);
    command.env("RALPH_WORKSPACE_ROOT", workspace_root);
    if std::path::Path::new("/var/tmp").is_dir() {
        command.env("TMPDIR", "/var/tmp");
        command.env("TMP", "/var/tmp");
        command.env("TEMP", "/var/tmp");
    }
}
