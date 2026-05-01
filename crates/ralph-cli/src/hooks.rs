//! CLI commands for the `ralph hooks` namespace.
//!
//! This command surface validates hook configuration and command wiring
//! without starting loop execution.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::RalphConfig;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{ConfigSource, HatsSource, preflight};

/// Manage hook-related commands.
#[derive(Parser, Debug)]
pub struct HooksArgs {
    #[command(subcommand)]
    pub command: HooksCommands,
}

#[derive(Subcommand, Debug)]
pub enum HooksCommands {
    /// Validate hooks configuration and command wiring
    Validate(ValidateArgs),
}

/// Output format for `ralph hooks validate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HooksValidateFormat {
    Human,
    Json,
}

/// Arguments for `ralph hooks validate`.
#[derive(Parser, Debug)]
pub struct ValidateArgs {
    /// Output format (human or json)
    #[arg(long, value_enum, default_value_t = HooksValidateFormat::Human)]
    pub format: HooksValidateFormat,
}

#[derive(Debug, Serialize)]
struct HooksValidateReport {
    pass: bool,
    source: String,
    hooks_enabled: bool,
    checked_hooks: usize,
    diagnostics: Vec<HooksDiagnostic>,
}

impl HooksValidateReport {
    fn new(source: String) -> Self {
        Self {
            pass: true,
            source,
            hooks_enabled: false,
            checked_hooks: 0,
            diagnostics: Vec::new(),
        }
    }

    fn push_diagnostic(
        &mut self,
        code: &str,
        message: impl Into<String>,
        phase_event: Option<String>,
        hook: Option<String>,
        command: Option<String>,
    ) {
        self.diagnostics.push(HooksDiagnostic {
            code: code.to_string(),
            message: message.into(),
            phase_event,
            hook,
            command,
        });
        self.pass = false;
    }
}

#[derive(Debug, Serialize)]
struct HooksDiagnostic {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

/// Execute a hooks command.
pub async fn execute(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: HooksArgs,
    use_colors: bool,
) -> Result<()> {
    match args.command {
        HooksCommands::Validate(validate_args) => {
            execute_validate(config_sources, hats_source, validate_args, use_colors).await
        }
    }
}

async fn execute_validate(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
    args: ValidateArgs,
    use_colors: bool,
) -> Result<()> {
    let report = build_report(config_sources, hats_source).await;

    match args.format {
        HooksValidateFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        HooksValidateFormat::Human => {
            print_human_report(&report, use_colors);
        }
    }

    if !report.pass {
        std::process::exit(1);
    }

    Ok(())
}

async fn build_report(
    config_sources: &[ConfigSource],
    hats_source: Option<&HatsSource>,
) -> HooksValidateReport {
    let source_label = preflight::config_source_label(config_sources, hats_source);
    let mut report = HooksValidateReport::new(source_label);

    let config = match preflight::load_config_for_preflight(config_sources, hats_source).await {
        Ok(config) => config,
        Err(error) => {
            report.push_diagnostic("config.load", error.to_string(), None, None, None);
            return report;
        }
    };

    report.hooks_enabled = config.hooks.enabled;
    report.checked_hooks = count_configured_hooks(&config);

    if let Err(error) = config.validate() {
        report.push_diagnostic("hooks.semantic", error.to_string(), None, None, None);
    }

    validate_duplicate_names(&config, &mut report);
    validate_command_resolvability(&config, &mut report);

    report
}

fn count_configured_hooks(config: &RalphConfig) -> usize {
    config.hooks.events.values().map(Vec::len).sum()
}

fn validate_duplicate_names(config: &RalphConfig, report: &mut HooksValidateReport) {
    let mut phase_events: Vec<_> = config.hooks.events.iter().collect();
    phase_events.sort_by_key(|(phase_event, _)| phase_event.as_str());

    for (phase_event, hooks) in phase_events {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (index, hook) in hooks.iter().enumerate() {
            let name = hook.name.trim();
            if name.is_empty() {
                continue;
            }

            if let Some(first_index) = seen.insert(name, index) {
                report.push_diagnostic(
                    "hooks.duplicate_name",
                    format!(
                        "Duplicate hook name '{name}' in phase-event '{}': indices [{first_index}] and [{index}]. Hook names must be unique per phase-event.",
                        phase_event.as_str()
                    ),
                    Some(phase_event.as_str().to_string()),
                    Some(name.to_string()),
                    hook.command.first().cloned(),
                );
            }
        }
    }
}

fn validate_command_resolvability(config: &RalphConfig, report: &mut HooksValidateReport) {
    let mut phase_events: Vec<_> = config.hooks.events.iter().collect();
    phase_events.sort_by_key(|(phase_event, _)| phase_event.as_str());

    for (phase_event, hooks) in phase_events {
        for hook in hooks {
            let Some(command) = hook
                .command
                .first()
                .map(|entry| entry.trim())
                .filter(|entry| !entry.is_empty())
            else {
                continue;
            };

            let cwd = resolve_hook_cwd(&config.core.workspace_root, hook.cwd.as_deref());
            let path_override = hook_path_override(&hook.env);

            if let Err(message) = resolve_hook_command(command, &cwd, path_override) {
                report.push_diagnostic(
                    "hooks.command_resolvable",
                    format!(
                        "{message}\nFix: ensure command exists and is executable, or invoke the script through an interpreter (for example: ['bash', 'script.sh'])."
                    ),
                    Some(phase_event.as_str().to_string()),
                    non_empty_trimmed(&hook.name),
                    Some(command.to_string()),
                );
            }
        }
    }
}

fn hook_path_override(env_map: &HashMap<String, String>) -> Option<&str> {
    env_map
        .get("PATH")
        .or_else(|| env_map.get("Path"))
        .map(String::as_str)
}

fn resolve_hook_cwd(workspace_root: &Path, hook_cwd: Option<&Path>) -> PathBuf {
    match hook_cwd {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => workspace_root.join(path),
        None => workspace_root.to_path_buf(),
    }
}

fn resolve_hook_command(
    command: &str,
    cwd: &Path,
    path_override: Option<&str>,
) -> std::result::Result<PathBuf, String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() || command_path.components().count() > 1 {
        let resolved = if command_path.is_absolute() {
            command_path.to_path_buf()
        } else {
            cwd.join(command_path)
        };

        if !resolved.exists() {
            return Err(format!(
                "Command '{command}' resolves to '{}' but the file does not exist.",
                resolved.display()
            ));
        }

        if !is_executable_file(&resolved) {
            return Err(format!(
                "Command '{command}' resolves to '{}' but it is not executable.",
                resolved.display()
            ));
        }

        return Ok(resolved);
    }

    let path_value = path_override
        .map(OsString::from)
        .or_else(|| env::var_os("PATH"))
        .ok_or_else(|| {
            format!(
                "PATH is not set while resolving command '{command}'. Set PATH in the environment or hook env override."
            )
        })?;

    let extensions = executable_extensions();
    let mut seen_paths = HashSet::new();

    for dir in env::split_paths(&path_value) {
        if !seen_paths.insert(dir.clone()) {
            continue;
        }

        for extension in &extensions {
            let candidate = if extension.is_empty() {
                dir.join(command)
            } else {
                dir.join(format!("{command}{}", extension.to_string_lossy()))
            };

            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }

    let path_source = if path_override.is_some() {
        "hook env PATH"
    } else {
        "process PATH"
    };

    Err(format!(
        "Command '{command}' was not found in {path_source}."
    ))
}

fn executable_extensions() -> Vec<OsString> {
    if cfg!(windows) {
        let exts = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        exts.split(';')
            .filter(|ext| !ext.trim().is_empty())
            .map(|ext| OsString::from(ext.trim().to_string()))
            .collect()
    } else {
        vec![OsString::new()]
    }
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn print_human_report(report: &HooksValidateReport, use_colors: bool) {
    use crate::display::colors;

    println!("Hooks validation for {}", report.source);
    println!();
    println!("Hooks enabled: {}", report.hooks_enabled);
    println!("Hooks checked: {}", report.checked_hooks);

    if report.diagnostics.is_empty() {
        println!("Diagnostics: none");
    } else {
        println!("Diagnostics:");
        for diagnostic in &report.diagnostics {
            print_human_diagnostic(diagnostic, use_colors);
        }
    }

    println!();

    let result = if report.pass { "PASS" } else { "FAIL" };
    let detail = if report.diagnostics.is_empty() {
        String::new()
    } else {
        format!(" ({} issue(s))", report.diagnostics.len())
    };

    if use_colors {
        let color = if report.pass {
            colors::GREEN
        } else {
            colors::RED
        };
        println!(
            "Result: {color}{result}{reset}{detail}",
            reset = colors::RESET
        );
    } else {
        println!("Result: {result}{detail}");
    }
}

fn print_human_diagnostic(diagnostic: &HooksDiagnostic, use_colors: bool) {
    use crate::display::colors;

    let status = if use_colors {
        format!("{}FAIL{}", colors::RED, colors::RESET)
    } else {
        "FAIL".to_string()
    };

    let mut lines = diagnostic.message.lines();
    let first_line = lines.next().unwrap_or_default();
    println!("  {status} {}: {first_line}", diagnostic.code);

    for line in lines {
        println!("       {line}");
    }

    if let Some(phase_event) = &diagnostic.phase_event {
        println!("       phase_event: {phase_event}");
    }
    if let Some(hook) = &diagnostic.hook {
        println!("       hook: {hook}");
    }
    if let Some(command) = &diagnostic.command {
        println!("       command: {command}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::{HookOnError, HookPhaseEvent, HookSpec, RalphConfig};
    use tempfile::TempDir;

    // ---------- pure helpers ----------

    #[test]
    fn non_empty_trimmed_returns_none_for_empty_or_whitespace() {
        assert_eq!(non_empty_trimmed(""), None);
        assert_eq!(non_empty_trimmed("   "), None);
        assert_eq!(non_empty_trimmed("\t\n "), None);
    }

    #[test]
    fn non_empty_trimmed_returns_trimmed_value() {
        assert_eq!(non_empty_trimmed("hook"), Some("hook".to_string()));
        assert_eq!(non_empty_trimmed("  hook  "), Some("hook".to_string()));
    }

    #[test]
    fn hook_path_override_prefers_upper_case_path() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("Path".to_string(), "/ignored".to_string());
        assert_eq!(hook_path_override(&env), Some("/usr/bin"));
    }

    #[test]
    fn hook_path_override_falls_back_to_windows_style_path() {
        let mut env = HashMap::new();
        env.insert("Path".to_string(), "C:/bin".to_string());
        assert_eq!(hook_path_override(&env), Some("C:/bin"));
    }

    #[test]
    fn hook_path_override_returns_none_when_absent() {
        let env: HashMap<String, String> = HashMap::new();
        assert_eq!(hook_path_override(&env), None);
    }

    #[test]
    fn resolve_hook_cwd_uses_workspace_root_when_hook_has_no_cwd() {
        let workspace = Path::new("/workspace");
        assert_eq!(resolve_hook_cwd(workspace, None), PathBuf::from("/workspace"));
    }

    #[test]
    fn resolve_hook_cwd_joins_relative_cwd_with_workspace() {
        let workspace = Path::new("/workspace");
        let resolved = resolve_hook_cwd(workspace, Some(Path::new("scripts")));
        assert_eq!(resolved, PathBuf::from("/workspace/scripts"));
    }

    #[test]
    fn resolve_hook_cwd_uses_absolute_cwd_directly() {
        let workspace = Path::new("/workspace");
        let resolved = resolve_hook_cwd(workspace, Some(Path::new("/etc/hook")));
        assert_eq!(resolved, PathBuf::from("/etc/hook"));
    }

    #[test]
    #[cfg(unix)]
    fn executable_extensions_on_unix_returns_single_empty_string() {
        let extensions = executable_extensions();
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0], OsString::new());
    }

    #[test]
    #[cfg(windows)]
    fn executable_extensions_on_windows_returns_pathext() {
        // Just sanity check that we get something non-empty on Windows.
        let extensions = executable_extensions();
        assert!(!extensions.is_empty());
    }

    #[test]
    fn count_configured_hooks_is_zero_for_default_config() {
        let config = RalphConfig::default();
        assert_eq!(count_configured_hooks(&config), 0);
    }

    #[test]
    fn count_configured_hooks_sums_across_events() {
        let mut config = RalphConfig::default();
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![
                build_hook("a", vec!["true".to_string()]),
                build_hook("b", vec!["true".to_string()]),
            ],
        );
        config.hooks.events.insert(
            HookPhaseEvent::PostLoopComplete,
            vec![build_hook("c", vec!["true".to_string()])],
        );

        assert_eq!(count_configured_hooks(&config), 3);
    }

    // ---------- is_executable_file / resolve_hook_command ----------

    #[test]
    fn is_executable_file_false_for_missing() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("nope");
        assert!(!is_executable_file(&missing));
    }

    #[test]
    fn is_executable_file_false_for_directory() {
        let temp = TempDir::new().unwrap();
        assert!(!is_executable_file(temp.path()));
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_file_requires_exec_bit_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let script = temp.path().join("script.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();

        // No exec bit yet.
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&script, perms).unwrap();
        assert!(!is_executable_file(&script));

        // With exec bit.
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        assert!(is_executable_file(&script));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_hook_command_accepts_absolute_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let script = temp.path().join("hook.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let resolved = resolve_hook_command(
            &script.to_string_lossy(),
            Path::new("/tmp"),
            None,
        )
        .expect("absolute executable should resolve");
        assert_eq!(resolved, script);
    }

    #[test]
    fn resolve_hook_command_absolute_path_missing_fails() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("nope.sh");

        let err = resolve_hook_command(
            &missing.to_string_lossy(),
            Path::new("/tmp"),
            None,
        )
        .expect_err("missing absolute command should fail");
        assert!(err.contains("does not exist"), "err: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_hook_command_absolute_path_non_executable_fails() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let script = temp.path().join("hook.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = resolve_hook_command(
            &script.to_string_lossy(),
            Path::new("/tmp"),
            None,
        )
        .expect_err("non-executable absolute command should fail");
        assert!(err.contains("not executable"), "err: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_hook_command_relative_resolves_against_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let scripts = temp.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let script = scripts.join("hook.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let resolved = resolve_hook_command("scripts/hook.sh", temp.path(), None)
            .expect("relative multi-segment command should resolve against cwd");
        assert_eq!(resolved, script);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_hook_command_uses_path_override_bin_dir() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cmd = bin.join("mytool");
        std::fs::write(&cmd, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&cmd, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path_override = bin.to_string_lossy().to_string();
        let resolved = resolve_hook_command("mytool", Path::new("/tmp"), Some(&path_override))
            .expect("command should resolve via PATH override");
        assert_eq!(resolved, cmd);
    }

    #[test]
    fn resolve_hook_command_reports_path_override_source_on_miss() {
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("empty-bin");
        std::fs::create_dir_all(&bin).unwrap();

        let path_override = bin.to_string_lossy().to_string();
        let err = resolve_hook_command(
            "nonexistent-ralph-tool-xyz",
            Path::new("/tmp"),
            Some(&path_override),
        )
        .expect_err("missing command should fail lookup");
        assert!(err.contains("hook env PATH"), "err: {err}");
    }

    // ---------- HooksValidateReport ----------

    #[test]
    fn report_new_starts_in_passing_state() {
        let report = HooksValidateReport::new("label".to_string());
        assert!(report.pass);
        assert_eq!(report.source, "label");
        assert!(!report.hooks_enabled);
        assert_eq!(report.checked_hooks, 0);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn push_diagnostic_flips_pass_and_records_details() {
        let mut report = HooksValidateReport::new("label".to_string());
        report.push_diagnostic(
            "hooks.command_resolvable",
            "boom",
            Some("pre.loop.start".to_string()),
            Some("env-guard".to_string()),
            Some("./run.sh".to_string()),
        );

        assert!(!report.pass);
        assert_eq!(report.diagnostics.len(), 1);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "hooks.command_resolvable");
        assert_eq!(diagnostic.message, "boom");
        assert_eq!(diagnostic.phase_event.as_deref(), Some("pre.loop.start"));
        assert_eq!(diagnostic.hook.as_deref(), Some("env-guard"));
        assert_eq!(diagnostic.command.as_deref(), Some("./run.sh"));
    }

    #[test]
    fn push_diagnostic_accumulates_multiple_failures() {
        let mut report = HooksValidateReport::new("label".to_string());
        report.push_diagnostic("a", "one", None, None, None);
        report.push_diagnostic("b", "two", None, None, None);
        assert!(!report.pass);
        assert_eq!(report.diagnostics.len(), 2);
    }

    // ---------- validate_duplicate_names ----------

    #[test]
    fn validate_duplicate_names_passes_on_unique_names() {
        let mut config = RalphConfig::default();
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![
                build_hook("alpha", vec!["true".to_string()]),
                build_hook("beta", vec!["true".to_string()]),
            ],
        );
        let mut report = HooksValidateReport::new("label".to_string());
        validate_duplicate_names(&config, &mut report);
        assert!(report.pass);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn validate_duplicate_names_flags_duplicates_in_same_phase() {
        let mut config = RalphConfig::default();
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![
                build_hook("alpha", vec!["true".to_string()]),
                build_hook("alpha", vec!["true".to_string()]),
            ],
        );
        let mut report = HooksValidateReport::new("label".to_string());
        validate_duplicate_names(&config, &mut report);

        assert!(!report.pass);
        assert_eq!(report.diagnostics.len(), 1);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "hooks.duplicate_name");
        assert!(diagnostic.message.contains("Duplicate hook name 'alpha'"));
        assert_eq!(diagnostic.phase_event.as_deref(), Some("pre.loop.start"));
        assert_eq!(diagnostic.hook.as_deref(), Some("alpha"));
    }

    #[test]
    fn validate_duplicate_names_allows_same_name_in_different_phases() {
        let mut config = RalphConfig::default();
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![build_hook("shared", vec!["true".to_string()])],
        );
        config.hooks.events.insert(
            HookPhaseEvent::PostLoopComplete,
            vec![build_hook("shared", vec!["true".to_string()])],
        );

        let mut report = HooksValidateReport::new("label".to_string());
        validate_duplicate_names(&config, &mut report);
        assert!(report.pass);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn validate_duplicate_names_ignores_empty_names() {
        // Two empty name entries — validator should not dedupe because name is empty.
        // (Real config validation rejects empty names; this guards the boundary.)
        let mut config = RalphConfig::default();
        config.hooks.events.insert(
            HookPhaseEvent::PreLoopStart,
            vec![
                build_hook("", vec!["true".to_string()]),
                build_hook("", vec!["true".to_string()]),
            ],
        );
        let mut report = HooksValidateReport::new("label".to_string());
        validate_duplicate_names(&config, &mut report);
        assert!(report.pass);
        assert!(report.diagnostics.is_empty());
    }

    // ---------- validate_command_resolvability ----------

    #[cfg(unix)]
    #[test]
    fn validate_command_resolvability_passes_for_existing_command() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cmd = bin.join("ralph-test-hook");
        std::fs::write(&cmd, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&cmd, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let mut hook = build_hook("good-hook", vec!["ralph-test-hook".to_string()]);
        hook.env
            .insert("PATH".to_string(), bin.to_string_lossy().to_string());
        config
            .hooks
            .events
            .insert(HookPhaseEvent::PreLoopStart, vec![hook]);

        let mut report = HooksValidateReport::new("label".to_string());
        validate_command_resolvability(&config, &mut report);
        assert!(report.pass, "diagnostics: {:?}", report.diagnostics);
    }

    #[test]
    fn validate_command_resolvability_flags_missing_absolute_command() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("does-not-exist");

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let hook = build_hook("bad-hook", vec![missing.to_string_lossy().to_string()]);
        config
            .hooks
            .events
            .insert(HookPhaseEvent::PreLoopStart, vec![hook]);

        let mut report = HooksValidateReport::new("label".to_string());
        validate_command_resolvability(&config, &mut report);

        assert!(!report.pass);
        assert_eq!(report.diagnostics.len(), 1);
        let diagnostic = &report.diagnostics[0];
        assert_eq!(diagnostic.code, "hooks.command_resolvable");
        assert_eq!(diagnostic.phase_event.as_deref(), Some("pre.loop.start"));
        assert_eq!(diagnostic.hook.as_deref(), Some("bad-hook"));
        assert!(diagnostic.message.contains("does not exist"));
        // Hint line is appended for the user.
        assert!(diagnostic.message.contains("Fix: ensure command exists"));
    }

    #[test]
    fn validate_command_resolvability_skips_hook_with_empty_command() {
        let mut config = RalphConfig::default();
        // Zero-argv hook is skipped by the validator (higher-level validation
        // catches this as a semantic error separately).
        let hook = build_hook("empty-cmd", vec![]);
        config
            .hooks
            .events
            .insert(HookPhaseEvent::PreLoopStart, vec![hook]);

        let mut report = HooksValidateReport::new("label".to_string());
        validate_command_resolvability(&config, &mut report);
        assert!(report.pass);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn validate_command_resolvability_skips_whitespace_only_command() {
        let mut config = RalphConfig::default();
        let hook = build_hook("blank-cmd", vec!["   ".to_string()]);
        config
            .hooks
            .events
            .insert(HookPhaseEvent::PreLoopStart, vec![hook]);

        let mut report = HooksValidateReport::new("label".to_string());
        validate_command_resolvability(&config, &mut report);
        assert!(report.pass);
        assert!(report.diagnostics.is_empty());
    }

    // ---------- build_report (async, integration-ish) ----------

    #[tokio::test]
    async fn build_report_passes_for_default_config_without_hooks() {
        // No config file on disk — loader uses defaults; hooks default to disabled/empty.
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("does-not-exist.yml");
        let sources = vec![ConfigSource::File(missing)];

        let report = build_report(&sources, None).await;
        assert!(report.pass, "diagnostics: {:?}", report.diagnostics);
        assert!(!report.hooks_enabled);
        assert_eq!(report.checked_hooks, 0);
    }

    #[tokio::test]
    async fn build_report_flags_duplicate_hook_names_via_config_file() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("ralph.yml");
        // Two hooks under the same phase-event with the same name. Both reference
        // /bin/sh so resolvability passes on Unix CI; the focus is duplicate detection.
        std::fs::write(
            &config_path,
            r#"
cli:
  backend: claude
event_loop:
  max_iterations: 1
  completion_promise: LOOP_COMPLETE
hats:
  builder:
    name: Builder
    description: stub
hooks:
  enabled: true
  events:
    pre.loop.start:
      - name: dup
        command: ["/bin/sh", "-c", "true"]
        on_error: warn
      - name: dup
        command: ["/bin/sh", "-c", "true"]
        on_error: warn
"#,
        )
        .unwrap();

        let sources = vec![ConfigSource::File(config_path)];
        let report = build_report(&sources, None).await;

        assert!(!report.pass, "expected failure, got {:?}", report);
        assert!(report.hooks_enabled);
        assert_eq!(report.checked_hooks, 2);
        let duplicate = report
            .diagnostics
            .iter()
            .find(|d| d.code == "hooks.duplicate_name");
        assert!(
            duplicate.is_some(),
            "expected hooks.duplicate_name diagnostic, got {:?}",
            report.diagnostics
        );
    }

    // ---------- helpers ----------

    /// Construct a HookSpec with sensible defaults for validator tests.
    fn build_hook(name: &str, command: Vec<String>) -> HookSpec {
        let yaml = format!(
            "name: {name}\ncommand: {command:?}\non_error: warn\n",
            name = name,
            command = command,
        );
        let mut spec: HookSpec = serde_yaml::from_str(&yaml).unwrap_or_else(|e| {
            panic!("failed to parse HookSpec YAML for tests: {e}\nYAML:\n{yaml}")
        });
        // Belt-and-suspenders: ensure on_error is set for hook-semantic paths.
        if spec.on_error.is_none() {
            spec.on_error = Some(HookOnError::Warn);
        }
        spec
    }
}
