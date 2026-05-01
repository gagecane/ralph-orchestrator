//! CLI commands for the `ralph tools skill` namespace.
//!
//! Provides subcommands for interacting with skills:
//! - `load`: Load a skill by name and output its content
//! - `list`: List available skills

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::{RalphConfig, SkillRegistry};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::config_resolution;

/// Output format for skill list command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
    /// Name-only output for scripting
    Quiet,
}

/// Skill management commands.
#[derive(Parser, Debug)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum SkillCommands {
    /// Load a skill by name and output its content
    Load(LoadArgs),

    /// List available skills
    List(ListArgs),
}

#[derive(Parser, Debug)]
pub struct LoadArgs {
    /// Name of the skill to load
    pub name: String,
}

/// Arguments for the `skill list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Execute a skill command.
pub fn execute(args: SkillArgs) -> Result<()> {
    let root = resolve_root(args.root)?;

    match args.command {
        SkillCommands::Load(load_args) => execute_load(&root, &load_args.name),
        SkillCommands::List(list_args) => execute_list(&root, list_args),
    }
}

fn execute_load(root: &Path, name: &str) -> Result<()> {
    let registry = build_registry(root)?;

    match registry.load_skill(name) {
        Some(content) => {
            print!("{content}");
            Ok(())
        }
        None => {
            eprintln!("Error: skill '{}' not found", name);
            let mut names: Vec<String> = registry
                .skills_for_hat(None)
                .into_iter()
                .map(|skill| skill.name.clone())
                .collect();
            names.sort();
            if names.is_empty() {
                eprintln!("No skills discovered. Check skills.dirs in ralph.yml or use --root.");
            } else {
                eprintln!("Available skills: {}", names.join(", "));
            }
            std::process::exit(1);
        }
    }
}

fn execute_list(root: &Path, args: ListArgs) -> Result<()> {
    let registry = build_registry(root)?;
    let mut skills = registry.skills_for_hat(None);
    skills.sort_by_key(|skill| skill.name.clone());

    match args.format {
        OutputFormat::Table => {
            if skills.is_empty() {
                println!("No skills found");
                return Ok(());
            }

            println!("{:<24} {:<28} {:<60}", "Name", "Source", "Description");
            println!("{}", "-".repeat(112));

            for skill in skills {
                let name = crate::display::truncate(&skill.name, 24);
                let source = format_source(skill);
                let source_truncated = crate::display::truncate(&source, 28);
                let description = if skill.description.is_empty() {
                    "(no description)".to_string()
                } else {
                    skill.description.clone()
                };
                let description_truncated = crate::display::truncate(&description, 60);

                println!(
                    "{:<24} {:<28} {:<60}",
                    name, source_truncated, description_truncated
                );
            }
        }
        OutputFormat::Json => {
            let items: Vec<SkillListItem> = skills.into_iter().map(SkillListItem::from).collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        OutputFormat::Quiet => {
            for skill in skills {
                println!("{}", skill.name);
            }
        }
    }

    Ok(())
}

fn build_registry(root: &Path) -> Result<SkillRegistry> {
    let config = load_config(root);
    let active_backend = Some(config.cli.backend.as_str());
    SkillRegistry::from_config(&config.skills, root, active_backend)
        .context("Failed to build skill registry")
}

fn format_source(skill: &ralph_core::SkillEntry) -> String {
    match &skill.source {
        ralph_core::SkillSource::BuiltIn => "built-in".to_string(),
        ralph_core::SkillSource::File(path) => path.display().to_string(),
    }
}

#[derive(Debug, Serialize)]
struct SkillListItem {
    name: String,
    description: String,
    source: String,
    path: Option<String>,
    hats: Vec<String>,
    backends: Vec<String>,
    tags: Vec<String>,
    auto_inject: bool,
}

impl From<&ralph_core::SkillEntry> for SkillListItem {
    fn from(skill: &ralph_core::SkillEntry) -> Self {
        let (source, path) = match &skill.source {
            ralph_core::SkillSource::BuiltIn => ("built-in".to_string(), None),
            ralph_core::SkillSource::File(path) => {
                ("file".to_string(), Some(path.display().to_string()))
            }
        };

        Self {
            name: skill.name.clone(),
            description: skill.description.clone(),
            source,
            path,
            hats: skill.hats.clone(),
            backends: skill.backends.clone(),
            tags: skill.tags.clone(),
            auto_inject: skill.auto_inject,
        }
    }
}

fn resolve_root(explicit_root: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = explicit_root {
        return Ok(root);
    }

    let cwd = std::env::current_dir().context("failed to get current directory")?;
    if let Some(found) = find_workspace_root(&cwd) {
        return Ok(found);
    }

    Ok(cwd)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if config_resolution::find_workspace_config_path(dir).is_some() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn find_default_skills_dir(root: &Path) -> Option<PathBuf> {
    let default_dir = root.join(".claude/skills");
    if default_dir.is_dir() {
        return Some(default_dir);
    }

    let cwd = std::env::current_dir().ok()?;
    if !cwd.starts_with(root) {
        return None;
    }

    let mut current = Some(cwd.as_path());
    while let Some(dir) = current {
        let candidate = dir.join(".claude/skills");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if dir == root {
            break;
        }
        current = dir.parent();
    }

    // Fallback: if the workspace root is nested (ralph.yml inside a subdir),
    // allow discovering a parent-level .claude/skills directory.
    let mut current = root.parent();
    while let Some(dir) = current {
        let candidate = dir.join(".claude/skills");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }

    None
}

fn resolve_configured_skills_dir(root: &Path, dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        return dir.to_path_buf();
    }

    let candidate = root.join(dir);
    if candidate.is_dir() {
        return candidate;
    }

    let mut current = root.parent();
    while let Some(parent) = current {
        let candidate = parent.join(dir);
        if candidate.is_dir() {
            return candidate;
        }
        current = parent.parent();
    }

    candidate
}

/// Load config from workspace root, falling back to defaults.
fn load_config(root: &Path) -> RalphConfig {
    let mut merged = match config_resolution::default_core_value() {
        Ok(value) => value,
        Err(_) => return RalphConfig::default(),
    };

    if let Ok(Some((user_value, _))) = config_resolution::load_optional_user_config_value() {
        if let Ok(next) = config_resolution::merge_yaml_values(merged, user_value) {
            merged = next;
        } else {
            return RalphConfig::default();
        }
    }

    if let Some(path) = config_resolution::find_workspace_config_path(root)
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(value) =
            config_resolution::parse_yaml_value(&content, &path.display().to_string())
    {
        if let Ok(next) = config_resolution::merge_yaml_values(merged, value) {
            merged = next;
        } else {
            return RalphConfig::default();
        }
    }

    let mut config: RalphConfig = serde_yaml::from_value(merged).unwrap_or_default();

    config.normalize();

    if config.skills.dirs.is_empty() {
        if let Some(default_dir) = find_default_skills_dir(root) {
            config.skills.dirs.push(default_dir);
        }
    } else {
        config.skills.dirs = config
            .skills
            .dirs
            .iter()
            .map(|dir| resolve_configured_skills_dir(root, dir))
            .collect();
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::{SkillEntry, SkillSource};
    use tempfile::TempDir;

    fn make_skill(name: &str, source: SkillSource) -> SkillEntry {
        SkillEntry {
            name: name.to_string(),
            description: "A test skill".to_string(),
            content: "body".to_string(),
            source,
            hats: vec!["reviewer".to_string()],
            backends: vec!["claude".to_string()],
            tags: vec!["t1".to_string(), "t2".to_string()],
            auto_inject: true,
        }
    }

    // ---------- format_source ----------

    #[test]
    fn test_format_source_builtin() {
        let skill = make_skill("s", SkillSource::BuiltIn);
        assert_eq!(format_source(&skill), "built-in");
    }

    #[test]
    fn test_format_source_file() {
        let path = PathBuf::from("/tmp/skills/test.md");
        let skill = make_skill("s", SkillSource::File(path.clone()));
        assert_eq!(format_source(&skill), path.display().to_string());
    }

    // ---------- SkillListItem::from ----------

    #[test]
    fn test_skill_list_item_from_builtin() {
        let skill = make_skill("my-skill", SkillSource::BuiltIn);
        let item = SkillListItem::from(&skill);

        assert_eq!(item.name, "my-skill");
        assert_eq!(item.description, "A test skill");
        assert_eq!(item.source, "built-in");
        assert_eq!(item.path, None);
        assert_eq!(item.hats, vec!["reviewer".to_string()]);
        assert_eq!(item.backends, vec!["claude".to_string()]);
        assert_eq!(item.tags, vec!["t1".to_string(), "t2".to_string()]);
        assert!(item.auto_inject);
    }

    #[test]
    fn test_skill_list_item_from_file() {
        let path = PathBuf::from("/tmp/skills/my.md");
        let skill = make_skill("my-skill", SkillSource::File(path.clone()));
        let item = SkillListItem::from(&skill);

        assert_eq!(item.source, "file");
        assert_eq!(item.path.as_deref(), Some(path.display().to_string().as_str()));
    }

    #[test]
    fn test_skill_list_item_serializes_to_json() {
        let skill = make_skill("my-skill", SkillSource::BuiltIn);
        let item = SkillListItem::from(&skill);
        let json = serde_json::to_string(&item).expect("serialize");

        assert!(json.contains("\"name\":\"my-skill\""));
        assert!(json.contains("\"source\":\"built-in\""));
        assert!(json.contains("\"auto_inject\":true"));
        // path is None → should serialize as null
        assert!(json.contains("\"path\":null"));
    }

    // ---------- resolve_root ----------

    #[test]
    fn test_resolve_root_with_explicit_path() {
        let explicit = PathBuf::from("/some/path");
        let resolved = resolve_root(Some(explicit.clone())).expect("resolve");
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn test_resolve_root_without_explicit_returns_path() {
        // Without explicit root, resolve_root returns either the discovered
        // workspace root or the current directory — both are valid absolute paths.
        let resolved = resolve_root(None).expect("resolve");
        assert!(resolved.is_absolute() || resolved.exists());
    }

    // ---------- find_workspace_root ----------

    #[test]
    fn test_find_workspace_root_finds_ralph_yml_in_start_dir() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("ralph.yml"), "cli:\n  backend: claude\n").expect("write");

        let found = find_workspace_root(root).expect("found");
        assert_eq!(found, root);
    }

    #[test]
    fn test_find_workspace_root_finds_ralph_yaml_in_start_dir() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("ralph.yaml"), "cli:\n  backend: claude\n").expect("write");

        let found = find_workspace_root(root).expect("found");
        assert_eq!(found, root);
    }

    #[test]
    fn test_find_workspace_root_walks_up_to_find_config() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("ralph.yml"), "cli:\n  backend: claude\n").expect("write");

        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let found = find_workspace_root(&nested).expect("found");
        assert_eq!(found, root);
    }

    #[test]
    fn test_find_workspace_root_returns_none_when_no_config() {
        let tmp = TempDir::new().expect("tmp");
        let nested = tmp.path().join("deep/nested/dir");
        std::fs::create_dir_all(&nested).expect("mkdir");

        // Note: this may find a config on a parent if /tmp happens to be under
        // a ralph workspace, but in a clean TempDir it should walk to / without
        // finding one. We assert that if it does return something, it's above
        // the tmp dir (i.e., not a false positive inside our tree).
        if let Some(found) = find_workspace_root(&nested) {
            assert!(!found.starts_with(tmp.path()));
        }
    }

    // ---------- resolve_configured_skills_dir ----------

    #[test]
    fn test_resolve_configured_skills_dir_absolute_path_returned_as_is() {
        let tmp = TempDir::new().expect("tmp");
        let abs_path = tmp.path().join("abs-skills");
        std::fs::create_dir(&abs_path).expect("mkdir");

        let resolved = resolve_configured_skills_dir(tmp.path(), &abs_path);
        assert_eq!(resolved, abs_path);
    }

    #[test]
    fn test_resolve_configured_skills_dir_relative_under_root() {
        let tmp = TempDir::new().expect("tmp");
        let rel = Path::new("skills");
        let absolute = tmp.path().join("skills");
        std::fs::create_dir(&absolute).expect("mkdir");

        let resolved = resolve_configured_skills_dir(tmp.path(), rel);
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn test_resolve_configured_skills_dir_walks_up_to_parent() {
        let tmp = TempDir::new().expect("tmp");
        // Create directory in parent but not at root
        let parent = tmp.path();
        let child = parent.join("child");
        std::fs::create_dir(&child).expect("mkdir child");
        let skills = parent.join("shared-skills");
        std::fs::create_dir(&skills).expect("mkdir skills");

        let resolved = resolve_configured_skills_dir(&child, Path::new("shared-skills"));
        assert_eq!(resolved, skills);
    }

    #[test]
    fn test_resolve_configured_skills_dir_missing_returns_candidate() {
        let tmp = TempDir::new().expect("tmp");
        // No skills dir exists anywhere
        let rel = Path::new("does-not-exist-skills");
        let resolved = resolve_configured_skills_dir(tmp.path(), rel);

        // Fallback candidate is root.join(dir)
        assert_eq!(resolved, tmp.path().join("does-not-exist-skills"));
    }

    // ---------- find_default_skills_dir ----------

    #[test]
    fn test_find_default_skills_dir_finds_in_root() {
        let tmp = TempDir::new().expect("tmp");
        let skills = tmp.path().join(".claude/skills");
        std::fs::create_dir_all(&skills).expect("mkdir skills");

        let found = find_default_skills_dir(tmp.path()).expect("found");
        // Compare canonicalized paths to handle symlinks (e.g., /tmp → /private/tmp)
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&skills).unwrap()
        );
    }

    #[test]
    fn test_find_default_skills_dir_none_when_missing() {
        let tmp = TempDir::new().expect("tmp");
        // No .claude/skills exists and parents shouldn't have one either (TempDir)
        let result = find_default_skills_dir(tmp.path());

        // If system happens to have .claude/skills above /tmp, we can't assert
        // None. But we CAN assert: if something is returned, it is NOT under
        // our tmp dir (since we didn't create one there).
        if let Some(found) = result {
            assert!(!found.starts_with(tmp.path()));
        }
    }

    // ---------- load_config ----------

    #[test]
    fn test_load_config_returns_default_when_no_config_file() {
        let tmp = TempDir::new().expect("tmp");
        // No ralph.yml, no .claude/skills
        let config = load_config(tmp.path());

        // Should at least produce a usable default config
        assert!(!config.cli.backend.is_empty());
    }

    #[test]
    fn test_load_config_reads_workspace_ralph_yml() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(
            tmp.path().join("ralph.yml"),
            "cli:\n  backend: kiro\n",
        )
        .expect("write");

        let config = load_config(tmp.path());
        assert_eq!(config.cli.backend, "kiro");
    }

    #[test]
    fn test_load_config_adds_default_skills_dir_when_present() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("ralph.yml"), "cli:\n  backend: claude\n").expect("write");
        let default_skills = tmp.path().join(".claude/skills");
        std::fs::create_dir_all(&default_skills).expect("mkdir");

        let config = load_config(tmp.path());

        // With no configured dirs, default `.claude/skills` should be added.
        // Compare canonicalized paths because TempDir on macOS resolves /tmp -> /private/tmp.
        let expected = std::fs::canonicalize(&default_skills).expect("canonicalize expected");
        assert!(
            config.skills.dirs.iter().any(|dir| {
                std::fs::canonicalize(dir)
                    .map(|canon| canon == expected)
                    .unwrap_or(false)
            }),
            "expected default skills dir in config.skills.dirs, got: {:?}",
            config.skills.dirs
        );
    }

    // ---------- build_registry ----------

    #[test]
    fn test_build_registry_includes_builtins() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("ralph.yml"), "cli:\n  backend: claude\n").expect("write");

        let registry = build_registry(tmp.path()).expect("build");
        // Built-in skills should always be registered.
        assert!(registry.get("ralph-tools").is_some());
        assert!(registry.get("ralph-tools-tasks").is_some());
    }

    #[test]
    fn test_build_registry_discovers_user_skill() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("ralph.yml"), "cli:\n  backend: claude\n").expect("write");

        let skill_dir = tmp.path().join(".claude/skills");
        std::fs::create_dir_all(&skill_dir).expect("mkdir");
        std::fs::write(
            skill_dir.join("custom.md"),
            "---\nname: custom\ndescription: Custom skill\n---\n\ncustom body\n",
        )
        .expect("write");

        let registry = build_registry(tmp.path()).expect("build");
        let custom = registry.get("custom").expect("custom skill found");
        assert_eq!(custom.description, "Custom skill");
    }

    // ---------- OutputFormat ----------

    #[test]
    fn test_output_format_default_is_table() {
        assert_eq!(OutputFormat::default(), OutputFormat::Table);
    }

    #[test]
    fn test_output_format_values_distinct() {
        // Sanity: each variant is distinct (guards against future refactors
        // accidentally making variants equal).
        assert_ne!(OutputFormat::Table, OutputFormat::Json);
        assert_ne!(OutputFormat::Json, OutputFormat::Quiet);
        assert_ne!(OutputFormat::Table, OutputFormat::Quiet);
    }
}
