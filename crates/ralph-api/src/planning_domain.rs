use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::errors::ApiError;
use crate::loop_support::now_ts;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningStartParams {
    pub prompt: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningRespondParams {
    pub session_id: String,
    pub prompt_id: String,
    pub response: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningGetArtifactParams {
    pub session_id: String,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningSessionSummary {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u64,
    pub iterations: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningSessionDetail {
    pub id: String,
    pub prompt: String,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub conversation: Vec<FrontendConversationEntry>,
    pub artifacts: Vec<String>,
    pub message_count: u64,
    pub iterations: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningSessionRecord {
    pub id: String,
    pub prompt: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub iterations: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadata {
    id: String,
    prompt: String,
    status: String,
    created_at: String,
    updated_at: String,
    iterations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationEntry {
    #[serde(rename = "type")]
    entry_type: String,
    id: String,
    text: String,
    ts: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendConversationEntry {
    #[serde(rename = "type")]
    entry_type: String,
    id: String,
    content: String,
    timestamp: String,
}

pub struct PlanningDomain {
    sessions_dir: PathBuf,
}

const MAX_SESSION_ID_LEN: usize = 120;

impl PlanningDomain {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            sessions_dir: workspace_root.as_ref().join(".ralph/planning-sessions"),
        }
    }

    pub fn list(&mut self) -> Result<Vec<PlanningSessionSummary>, ApiError> {
        self.ensure_sessions_dir()?;

        let entries = fs::read_dir(&self.sessions_dir).map_err(|error| {
            ApiError::internal(format!(
                "failed reading planning sessions directory '{}': {error}",
                self.sessions_dir.display()
            ))
        })?;

        let mut sessions = Vec::new();

        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(session_id) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };

            let Ok(metadata) = self.read_metadata(session_id) else {
                warn!(session_id, "skipping malformed planning session metadata");
                continue;
            };

            let message_count = self.count_messages(session_id);
            sessions.push(PlanningSessionSummary {
                id: metadata.id.clone(),
                title: generate_title(&metadata.prompt),
                prompt: metadata.prompt.clone(),
                status: to_frontend_status(&metadata.status),
                created_at: metadata.created_at.clone(),
                updated_at: metadata.updated_at.clone(),
                message_count,
                iterations: metadata.iterations,
            });
        }

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        Ok(sessions)
    }

    pub fn get(&self, session_id: &str) -> Result<PlanningSessionDetail, ApiError> {
        validate_session_id(session_id)?;

        let metadata = self.read_metadata(session_id)?;
        let conversation = self.read_conversation(session_id);
        let artifacts = self.read_artifacts(session_id);

        let completed_at = (metadata.status == "completed").then_some(metadata.updated_at.clone());

        Ok(PlanningSessionDetail {
            id: metadata.id,
            prompt: metadata.prompt.clone(),
            title: generate_title(&metadata.prompt),
            status: to_frontend_status(&metadata.status),
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            completed_at,
            conversation: conversation.clone(),
            artifacts,
            message_count: u64::try_from(conversation.len()).unwrap_or(u64::MAX),
            iterations: metadata.iterations,
        })
    }

    pub fn start(
        &mut self,
        params: PlanningStartParams,
    ) -> Result<PlanningSessionRecord, ApiError> {
        self.ensure_sessions_dir()?;

        let (session_id, session_dir) = self.create_unique_session_dir()?;

        fs::create_dir_all(session_dir.join("artifacts")).map_err(|error| {
            ApiError::internal(format!(
                "failed creating planning session directory '{}': {error}",
                session_dir.display()
            ))
        })?;

        let now = now_ts();
        let metadata = SessionMetadata {
            id: session_id.clone(),
            prompt: params.prompt,
            status: "active".to_string(),
            created_at: now.clone(),
            updated_at: now,
            iterations: 0,
        };

        self.write_metadata(&metadata)?;
        self.write_empty_conversation(&session_id)?;

        Ok(PlanningSessionRecord {
            id: metadata.id,
            prompt: metadata.prompt,
            status: metadata.status,
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            iterations: metadata.iterations,
        })
    }

    pub fn respond(&mut self, params: PlanningRespondParams) -> Result<(), ApiError> {
        validate_session_id(&params.session_id)?;

        let mut metadata = self.read_metadata(&params.session_id)?;

        let entry = ConversationEntry {
            entry_type: "user_response".to_string(),
            id: params.prompt_id,
            text: params.response,
            ts: now_ts(),
        };
        self.append_conversation(&params.session_id, &entry)?;

        metadata.status = "active".to_string();
        metadata.updated_at = now_ts();
        self.write_metadata(&metadata)
    }

    pub fn resume(&mut self, session_id: &str) -> Result<(), ApiError> {
        validate_session_id(session_id)?;

        let mut metadata = self.read_metadata(session_id)?;
        metadata.status = "active".to_string();
        metadata.updated_at = now_ts();
        self.write_metadata(&metadata)
    }

    pub fn delete(&mut self, session_id: &str) -> Result<(), ApiError> {
        validate_session_id(session_id)?;

        let session_dir = self.session_dir(session_id);
        if !session_dir.exists() {
            return Err(planning_session_not_found_error(session_id));
        }

        fs::remove_dir_all(&session_dir).map_err(|error| {
            ApiError::internal(format!(
                "failed deleting planning session '{}': {error}",
                session_dir.display()
            ))
        })
    }

    pub fn get_artifact(
        &self,
        params: PlanningGetArtifactParams,
    ) -> Result<ArtifactRecord, ApiError> {
        validate_session_id(&params.session_id)?;

        if is_invalid_filename(&params.filename) {
            return Err(ApiError::invalid_params(
                "planning.get_artifact filename must be a plain file name",
            ));
        }

        // Keep get/list contract consistent: if a filename would not appear in
        // `planning.get` artifact listings, reject direct access as not found.
        if !is_listed_artifact_name(&params.filename) {
            return Err(ApiError::not_found(format!(
                "artifact '{}' not found for planning session '{}'",
                params.filename, params.session_id
            )));
        }

        let session_dir = self.session_dir(&params.session_id);
        if !session_dir.exists() {
            return Err(planning_session_not_found_error(&params.session_id));
        }

        let artifact_path = session_dir.join("artifacts").join(&params.filename);

        // Use symlink_metadata so we inspect the path entry itself, not any
        // target it may point to.  A symlink (or directory, device node, …)
        // must be treated the same as "not found" so the API leaks nothing.
        let fmeta = fs::symlink_metadata(&artifact_path).map_err(|_| {
            ApiError::not_found(format!(
                "artifact '{}' not found for planning session '{}'",
                params.filename, params.session_id
            ))
        })?;
        if !fmeta.is_file() {
            return Err(ApiError::not_found(format!(
                "artifact '{}' not found for planning session '{}'",
                params.filename, params.session_id
            )));
        }

        let content = fs::read_to_string(&artifact_path).map_err(|error| {
            ApiError::not_found(format!(
                "artifact '{}' not found for planning session '{}': {error}",
                params.filename, params.session_id
            ))
        })?;

        Ok(ArtifactRecord {
            filename: params.filename,
            content,
        })
    }

    fn next_session_id(&self) -> String {
        format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S"),
            uuid::Uuid::new_v4().simple()
        )
    }

    fn create_unique_session_dir(&self) -> Result<(String, PathBuf), ApiError> {
        for _ in 0..8 {
            let session_id = self.next_session_id();
            let session_dir = self.session_dir(&session_id);

            match fs::create_dir(&session_dir) {
                Ok(()) => return Ok((session_id, session_dir)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ApiError::internal(format!(
                        "failed creating planning session directory '{}': {error}",
                        session_dir.display()
                    )));
                }
            }
        }

        Err(ApiError::internal(
            "failed allocating unique planning session id after multiple attempts",
        ))
    }

    fn ensure_sessions_dir(&self) -> Result<(), ApiError> {
        fs::create_dir_all(&self.sessions_dir).map_err(|error| {
            ApiError::internal(format!(
                "failed creating planning sessions directory '{}': {error}",
                self.sessions_dir.display()
            ))
        })
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(session_id)
    }

    fn metadata_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn conversation_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("conversation.jsonl")
    }

    fn read_metadata(&self, session_id: &str) -> Result<SessionMetadata, ApiError> {
        validate_session_id(session_id)?;

        let path = self.metadata_path(session_id);

        let content =
            fs::read_to_string(&path).map_err(|_| planning_session_not_found_error(session_id))?;

        serde_json::from_str::<SessionMetadata>(&content).map_err(|error| {
            ApiError::internal(format!(
                "failed parsing planning metadata '{}': {error}",
                path.display()
            ))
        })
    }

    fn write_metadata(&self, metadata: &SessionMetadata) -> Result<(), ApiError> {
        let path = self.metadata_path(&metadata.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "failed creating planning metadata directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }

        let payload = serde_json::to_string_pretty(metadata).map_err(|error| {
            ApiError::internal(format!("failed serializing planning metadata: {error}"))
        })?;

        fs::write(&path, payload).map_err(|error| {
            ApiError::internal(format!(
                "failed writing planning metadata '{}': {error}",
                path.display()
            ))
        })
    }

    fn write_empty_conversation(&self, session_id: &str) -> Result<(), ApiError> {
        let path = self.conversation_path(session_id);
        fs::write(&path, "").map_err(|error| {
            ApiError::internal(format!(
                "failed creating planning conversation '{}': {error}",
                path.display()
            ))
        })
    }

    fn append_conversation(
        &self,
        session_id: &str,
        entry: &ConversationEntry,
    ) -> Result<(), ApiError> {
        let path = self.conversation_path(session_id);
        let mut payload = serde_json::to_string(entry).map_err(|error| {
            ApiError::internal(format!("failed serializing conversation entry: {error}"))
        })?;
        payload.push('\n');

        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "failed opening planning conversation '{}': {error}",
                    path.display()
                ))
            })?;

        file.write_all(payload.as_bytes()).map_err(|error| {
            ApiError::internal(format!(
                "failed appending planning conversation '{}': {error}",
                path.display()
            ))
        })
    }

    fn read_conversation(&self, session_id: &str) -> Vec<FrontendConversationEntry> {
        let path = self.conversation_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return Vec::new();
        };

        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<ConversationEntry>(line).ok())
            .map(|entry| FrontendConversationEntry {
                entry_type: if entry.entry_type == "user_prompt" {
                    "prompt".to_string()
                } else {
                    "response".to_string()
                },
                id: entry.id,
                content: entry.text,
                timestamp: entry.ts,
            })
            .collect()
    }

    fn count_messages(&self, session_id: &str) -> u64 {
        let path = self.conversation_path(session_id);
        let Ok(content) = fs::read_to_string(path) else {
            return 0;
        };

        u64::try_from(
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count(),
        )
        .unwrap_or(u64::MAX)
    }

    fn read_artifacts(&self, session_id: &str) -> Vec<String> {
        let artifacts_dir = self.session_dir(session_id).join("artifacts");
        let Ok(entries) = fs::read_dir(artifacts_dir) else {
            return Vec::new();
        };

        let mut artifacts: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                // file_type() does NOT follow symlinks, so symlinks return
                // is_symlink()=true / is_file()=false and are excluded here.
                let ftype = entry.file_type().ok()?;
                if !ftype.is_file() {
                    return None;
                }
                entry
                    .file_name()
                    .to_str()
                    .map(std::string::ToString::to_string)
            })
            .filter(|name| is_listed_artifact_name(name))
            .collect();
        artifacts.sort();
        artifacts
    }
}

fn validate_session_id(session_id: &str) -> Result<(), ApiError> {
    if session_id.is_empty() || session_id.len() > MAX_SESSION_ID_LEN {
        return Err(ApiError::invalid_params(format!(
            "planning session id must be 1..={MAX_SESSION_ID_LEN} characters"
        ))
        .with_details(serde_json::json!({ "sessionId": session_id })));
    }

    if !session_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(ApiError::invalid_params(
            "planning session id may only contain ASCII letters, digits, '-' or '_'",
        )
        .with_details(serde_json::json!({ "sessionId": session_id })));
    }

    Ok(())
}

fn is_invalid_filename(filename: &str) -> bool {
    let mut components = Path::new(filename).components();

    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => name.to_string_lossy().is_empty(),
        _ => true,
    }
}

fn is_listed_artifact_name(filename: &str) -> bool {
    !filename.starts_with('.')
        && filename.len() <= 255
        && !filename.is_empty()
        && filename
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        && !is_invalid_filename(filename)
}

fn planning_session_not_found_error(session_id: &str) -> ApiError {
    ApiError::planning_session_not_found(format!("Planning session '{session_id}' not found"))
        .with_details(serde_json::json!({ "sessionId": session_id }))
}

fn to_frontend_status(status: &str) -> String {
    if status == "waiting_for_input" {
        return "paused".to_string();
    }

    status.to_string()
}

fn generate_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.chars().count() <= 60 {
        return trimmed.to_string();
    }

    let mut shortened: String = trimmed.chars().take(57).collect();
    shortened.push_str("...");
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use crate::errors::RpcErrorCode;

    fn domain() -> (TempDir, PlanningDomain) {
        let temp = TempDir::new().expect("tempdir");
        let domain = PlanningDomain::new(temp.path());
        (temp, domain)
    }

    // ----- validate_session_id -----

    #[test]
    fn validate_session_id_accepts_ascii_alnum_and_allowed_punctuation() {
        validate_session_id("abc123").expect("alnum");
        validate_session_id("session-id_42").expect("hyphen and underscore");
        validate_session_id("A").expect("single char");
        validate_session_id(&"x".repeat(MAX_SESSION_ID_LEN)).expect("max length");
    }

    #[test]
    fn validate_session_id_rejects_empty() {
        let err = validate_session_id("").expect_err("empty should be invalid");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn validate_session_id_rejects_too_long() {
        let long = "a".repeat(MAX_SESSION_ID_LEN + 1);
        let err = validate_session_id(&long).expect_err("too long");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn validate_session_id_rejects_invalid_characters() {
        for candidate in ["abc/def", "abc def", "abc.def", "abc;def", "../sneaky", "abc\u{00e9}"] {
            let err =
                validate_session_id(candidate).expect_err(&format!("should reject {candidate}"));
            assert_eq!(err.code, RpcErrorCode::InvalidParams, "for {candidate}");
        }
    }

    #[test]
    fn validate_session_id_error_carries_session_id_details() {
        let err = validate_session_id("bad id").expect_err("spaces are invalid");
        let details = err.details.expect("details set");
        assert_eq!(details.get("sessionId").and_then(|v| v.as_str()), Some("bad id"));
    }

    // ----- is_invalid_filename -----

    #[test]
    fn is_invalid_filename_accepts_plain_names() {
        assert!(!is_invalid_filename("plan.md"));
        assert!(!is_invalid_filename("artifact_1.txt"));
        assert!(!is_invalid_filename("a"));
    }

    #[test]
    fn is_invalid_filename_rejects_empty_and_traversal_and_paths() {
        assert!(is_invalid_filename(""));
        assert!(is_invalid_filename("."));
        assert!(is_invalid_filename(".."));
        assert!(is_invalid_filename("../escape"));
        assert!(is_invalid_filename("sub/plan.md"));
        assert!(is_invalid_filename("/abs/plan.md"));
    }

    // ----- is_listed_artifact_name -----

    #[test]
    fn is_listed_artifact_name_accepts_allowed_characters() {
        assert!(is_listed_artifact_name("plan.md"));
        assert!(is_listed_artifact_name("a-b_c.1"));
        assert!(is_listed_artifact_name("A"));
    }

    #[test]
    fn is_listed_artifact_name_rejects_hidden_and_overlong_and_bad_chars() {
        assert!(!is_listed_artifact_name(""));
        assert!(!is_listed_artifact_name(".hidden"));
        assert!(!is_listed_artifact_name("with space.md"));
        assert!(!is_listed_artifact_name("sub/plan.md"));
        let overlong = "a".repeat(256);
        assert!(!is_listed_artifact_name(&overlong));
        // Boundary: exactly 255 is allowed, 256 is not.
        let at_limit = "a".repeat(255);
        assert!(is_listed_artifact_name(&at_limit));
    }

    // ----- to_frontend_status -----

    #[test]
    fn to_frontend_status_maps_waiting_for_input_to_paused() {
        assert_eq!(to_frontend_status("waiting_for_input"), "paused");
    }

    #[test]
    fn to_frontend_status_passes_through_other_statuses() {
        for value in ["active", "completed", "error", "custom"] {
            assert_eq!(to_frontend_status(value), value);
        }
    }

    // ----- generate_title -----

    #[test]
    fn generate_title_short_prompt_returned_trimmed() {
        assert_eq!(generate_title("   hello   "), "hello");
    }

    #[test]
    fn generate_title_long_prompt_is_truncated_with_ellipsis() {
        let prompt = "a".repeat(120);
        let title = generate_title(&prompt);
        assert_eq!(title.chars().count(), 60, "57 chars + '...'");
        assert!(title.ends_with("..."));
        assert!(title.starts_with(&"a".repeat(57)));
    }

    #[test]
    fn generate_title_60_char_prompt_is_not_truncated() {
        let prompt = "a".repeat(60);
        assert_eq!(generate_title(&prompt), prompt);
    }

    #[test]
    fn generate_title_61_char_prompt_is_truncated() {
        let prompt = "a".repeat(61);
        let title = generate_title(&prompt);
        assert!(title.ends_with("..."));
        assert_eq!(title.chars().count(), 60);
    }

    #[test]
    fn generate_title_handles_multi_byte_characters() {
        // Each "é" is multi-byte but a single char — we must truncate by chars, not bytes.
        let prompt = "é".repeat(120);
        let title = generate_title(&prompt);
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with("..."));
    }

    // ----- planning_session_not_found_error -----

    #[test]
    fn planning_session_not_found_error_has_expected_code_and_details() {
        let err = planning_session_not_found_error("abc");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
        let details = err.details.expect("details");
        assert_eq!(details.get("sessionId").and_then(|v| v.as_str()), Some("abc"));
    }

    // ----- PlanningDomain::new -----

    #[test]
    fn new_points_sessions_dir_inside_workspace() {
        let temp = TempDir::new().expect("tempdir");
        let domain = PlanningDomain::new(temp.path());
        assert_eq!(domain.sessions_dir, temp.path().join(".ralph/planning-sessions"));
    }

    // ----- list -----

    #[test]
    fn list_on_empty_workspace_creates_dir_and_returns_empty() {
        let (temp, mut domain) = domain();
        assert!(!domain.sessions_dir.exists());
        let sessions = domain.list().expect("list");
        assert!(sessions.is_empty());
        assert!(temp.path().join(".ralph/planning-sessions").exists());
    }

    #[test]
    fn list_skips_malformed_metadata_and_non_directories() {
        let (_temp, mut domain) = domain();
        domain.ensure_sessions_dir().unwrap();

        // Valid session.
        let valid = domain
            .start(PlanningStartParams { prompt: "Valid".into() })
            .expect("start");

        // Stray file (not a directory) — should be skipped silently.
        fs::write(domain.sessions_dir.join("stray.txt"), "nope").unwrap();

        // Directory with malformed session.json — should be skipped via warn.
        let bogus_dir = domain.sessions_dir.join("bogus");
        fs::create_dir(&bogus_dir).unwrap();
        fs::write(bogus_dir.join("session.json"), "not valid json").unwrap();

        // Directory with no session.json at all — also skipped.
        fs::create_dir(domain.sessions_dir.join("empty-dir")).unwrap();

        let sessions = domain.list().expect("list succeeds");
        assert_eq!(sessions.len(), 1, "only the valid session should surface");
        assert_eq!(sessions[0].id, valid.id);
    }

    #[test]
    fn list_sorts_by_updated_at_descending_then_id_ascending() {
        let (_temp, mut domain) = domain();
        domain.ensure_sessions_dir().unwrap();

        // Construct three sessions with known metadata.
        for (id, ts) in [
            ("session-a", "2026-01-01T00:00:00Z"),
            ("session-b", "2026-01-02T00:00:00Z"),
            ("session-c", "2026-01-02T00:00:00Z"),
        ] {
            let dir = domain.session_dir(id);
            fs::create_dir_all(dir.join("artifacts")).unwrap();
            fs::write(dir.join("conversation.jsonl"), "").unwrap();
            let metadata = SessionMetadata {
                id: id.into(),
                prompt: format!("prompt for {id}"),
                status: "active".into(),
                created_at: ts.into(),
                updated_at: ts.into(),
                iterations: 0,
            };
            domain.write_metadata(&metadata).unwrap();
        }

        let sessions = domain.list().expect("list");
        let ids: Vec<_> = sessions.into_iter().map(|s| s.id).collect();
        // b and c share updated_at; alpha-ascending id breaks the tie.
        assert_eq!(ids, ["session-b", "session-c", "session-a"]);
    }

    #[test]
    fn list_populates_summary_fields_from_metadata_and_conversation() {
        let (_temp, mut domain) = domain();
        domain.ensure_sessions_dir().unwrap();

        let record = domain
            .start(PlanningStartParams { prompt: "My plan".into() })
            .expect("start");

        // Append two conversation entries.
        domain
            .append_conversation(
                &record.id,
                &ConversationEntry {
                    entry_type: "user_prompt".into(),
                    id: "p1".into(),
                    text: "hi".into(),
                    ts: "2026-01-01T00:00:00Z".into(),
                },
            )
            .unwrap();
        domain
            .append_conversation(
                &record.id,
                &ConversationEntry {
                    entry_type: "assistant".into(),
                    id: "r1".into(),
                    text: "hello".into(),
                    ts: "2026-01-01T00:00:01Z".into(),
                },
            )
            .unwrap();

        // Mark status as waiting_for_input to verify frontend mapping.
        let mut meta = domain.read_metadata(&record.id).unwrap();
        meta.status = "waiting_for_input".into();
        domain.write_metadata(&meta).unwrap();

        let sessions = domain.list().expect("list");
        assert_eq!(sessions.len(), 1);
        let summary = &sessions[0];
        assert_eq!(summary.id, record.id);
        assert_eq!(summary.title, "My plan");
        assert_eq!(summary.prompt, "My plan");
        assert_eq!(summary.status, "paused", "waiting_for_input -> paused");
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.iterations, 0);
    }

    // ----- start -----

    #[test]
    fn start_creates_session_directory_and_metadata_and_empty_conversation() {
        let (_temp, mut domain) = domain();

        let record = domain
            .start(PlanningStartParams { prompt: "Hello".into() })
            .expect("start");

        assert_eq!(record.prompt, "Hello");
        assert_eq!(record.status, "active");
        assert_eq!(record.iterations, 0);
        assert_eq!(record.created_at, record.updated_at);
        assert!(!record.id.is_empty());

        let session_dir = domain.session_dir(&record.id);
        assert!(session_dir.is_dir());
        assert!(session_dir.join("artifacts").is_dir());
        assert!(session_dir.join("session.json").is_file());

        let conversation_path = session_dir.join("conversation.jsonl");
        assert!(conversation_path.is_file());
        assert_eq!(fs::read_to_string(conversation_path).unwrap(), "");

        // Session id must be a valid one per our public validator.
        validate_session_id(&record.id).expect("generated id must validate");
    }

    #[test]
    fn start_generates_unique_session_ids() {
        let (_temp, mut domain) = domain();
        let a = domain
            .start(PlanningStartParams { prompt: "A".into() })
            .expect("a");
        let b = domain
            .start(PlanningStartParams { prompt: "B".into() })
            .expect("b");
        assert_ne!(a.id, b.id);
    }

    // ----- get -----

    #[test]
    fn get_returns_detail_with_conversation_and_artifacts_and_completed_at() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "Prompt".into() })
            .expect("start");

        // Add conversation entries (one prompt, one other).
        domain
            .append_conversation(
                &record.id,
                &ConversationEntry {
                    entry_type: "user_prompt".into(),
                    id: "p1".into(),
                    text: "ask".into(),
                    ts: "2026-01-01T00:00:00Z".into(),
                },
            )
            .unwrap();
        domain
            .append_conversation(
                &record.id,
                &ConversationEntry {
                    entry_type: "user_response".into(),
                    id: "r1".into(),
                    text: "ans".into(),
                    ts: "2026-01-01T00:00:01Z".into(),
                },
            )
            .unwrap();

        // Add an allowed artifact and a hidden one (must be filtered).
        let artifacts_dir = domain.session_dir(&record.id).join("artifacts");
        fs::write(artifacts_dir.join("plan.md"), "# plan").unwrap();
        fs::write(artifacts_dir.join(".hidden"), "nope").unwrap();
        fs::write(artifacts_dir.join("with space.md"), "nope").unwrap();

        // Mark completed and bump updated_at so completed_at is populated.
        let mut meta = domain.read_metadata(&record.id).unwrap();
        meta.status = "completed".into();
        meta.updated_at = "2026-02-02T02:02:02Z".into();
        domain.write_metadata(&meta).unwrap();

        let detail = domain.get(&record.id).expect("get");
        assert_eq!(detail.id, record.id);
        assert_eq!(detail.status, "completed");
        assert_eq!(detail.completed_at.as_deref(), Some("2026-02-02T02:02:02Z"));
        assert_eq!(detail.message_count, 2);
        assert_eq!(detail.conversation.len(), 2);
        assert_eq!(detail.conversation[0].entry_type, "prompt");
        assert_eq!(detail.conversation[0].content, "ask");
        assert_eq!(detail.conversation[1].entry_type, "response");
        assert_eq!(detail.conversation[1].content, "ans");
        assert_eq!(detail.artifacts, vec!["plan.md".to_string()]);
    }

    #[test]
    fn get_returns_no_completed_at_for_non_completed_sessions() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        let detail = domain.get(&record.id).expect("get");
        assert!(detail.completed_at.is_none());
        assert_eq!(detail.status, "active");
        assert_eq!(detail.message_count, 0);
        assert!(detail.conversation.is_empty());
        assert!(detail.artifacts.is_empty());
    }

    #[test]
    fn get_missing_session_returns_planning_session_not_found() {
        let (_temp, domain) = domain();
        let err = domain.get("no-such-session").expect_err("not found");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn get_invalid_session_id_returns_invalid_params() {
        let (_temp, domain) = domain();
        let err = domain.get("").expect_err("empty id rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);

        let err = domain.get("bad id").expect_err("spaces rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    // ----- respond -----

    #[test]
    fn respond_appends_user_response_entry_and_marks_active() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");

        // Force status away from active so we can see respond() reset it.
        let mut meta = domain.read_metadata(&record.id).unwrap();
        meta.status = "waiting_for_input".into();
        domain.write_metadata(&meta).unwrap();

        domain
            .respond(PlanningRespondParams {
                session_id: record.id.clone(),
                prompt_id: "prompt-42".into(),
                response: "my answer".into(),
            })
            .expect("respond");

        // Status re-activated.
        let meta = domain.read_metadata(&record.id).unwrap();
        assert_eq!(meta.status, "active");

        // Conversation now has exactly one entry, and it is a user_response.
        let path = domain.conversation_path(&record.id);
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1);
        let entry: ConversationEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry.entry_type, "user_response");
        assert_eq!(entry.id, "prompt-42");
        assert_eq!(entry.text, "my answer");
    }

    #[test]
    fn respond_unknown_session_returns_not_found() {
        let (_temp, mut domain) = domain();
        let err = domain
            .respond(PlanningRespondParams {
                session_id: "missing".into(),
                prompt_id: "p".into(),
                response: "r".into(),
            })
            .expect_err("session missing");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn respond_invalid_session_id_returns_invalid_params() {
        let (_temp, mut domain) = domain();
        let err = domain
            .respond(PlanningRespondParams {
                session_id: "bad id".into(),
                prompt_id: "p".into(),
                response: "r".into(),
            })
            .expect_err("invalid id");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    // ----- resume -----

    #[test]
    fn resume_sets_status_to_active() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");

        let mut meta = domain.read_metadata(&record.id).unwrap();
        meta.status = "waiting_for_input".into();
        domain.write_metadata(&meta).unwrap();

        domain.resume(&record.id).expect("resume");

        let meta = domain.read_metadata(&record.id).unwrap();
        assert_eq!(meta.status, "active");
    }

    #[test]
    fn resume_unknown_session_returns_not_found() {
        let (_temp, mut domain) = domain();
        let err = domain.resume("missing").expect_err("missing");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn resume_invalid_session_id_returns_invalid_params() {
        let (_temp, mut domain) = domain();
        let err = domain.resume("").expect_err("empty id");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    // ----- delete -----

    #[test]
    fn delete_removes_session_directory() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        let dir = domain.session_dir(&record.id);
        assert!(dir.exists());

        domain.delete(&record.id).expect("delete");
        assert!(!dir.exists());
    }

    #[test]
    fn delete_unknown_session_returns_not_found() {
        let (_temp, mut domain) = domain();
        domain.ensure_sessions_dir().unwrap();
        let err = domain.delete("no-such-session").expect_err("missing");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn delete_invalid_session_id_returns_invalid_params() {
        let (_temp, mut domain) = domain();
        let err = domain.delete("").expect_err("empty id");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    // ----- get_artifact -----

    #[test]
    fn get_artifact_returns_content_for_valid_file() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        fs::write(
            domain.session_dir(&record.id).join("artifacts").join("plan.md"),
            "# plan body",
        )
        .unwrap();

        let artifact = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id.clone(),
                filename: "plan.md".into(),
            })
            .expect("get artifact");
        assert_eq!(artifact.filename, "plan.md");
        assert_eq!(artifact.content, "# plan body");
    }

    #[test]
    fn get_artifact_rejects_path_traversal_as_invalid_params() {
        let (_temp, domain) = domain();
        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: "abc123".into(),
                filename: "../escape".into(),
            })
            .expect_err("traversal");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn get_artifact_rejects_nested_paths_as_invalid_params() {
        let (_temp, domain) = domain();
        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: "abc123".into(),
                filename: "sub/plan.md".into(),
            })
            .expect_err("nested path");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn get_artifact_rejects_empty_filename_as_invalid_params() {
        let (_temp, domain) = domain();
        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: "abc123".into(),
                filename: String::new(),
            })
            .expect_err("empty");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn get_artifact_rejects_hidden_file_as_not_found() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        // Even if we write the file on disk, the API must not expose hidden names.
        fs::write(
            domain.session_dir(&record.id).join("artifacts").join(".secret"),
            "shh",
        )
        .unwrap();

        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id,
                filename: ".secret".into(),
            })
            .expect_err("hidden file");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn get_artifact_rejects_name_with_disallowed_characters_as_not_found() {
        // "with space.md" fails is_listed_artifact_name but passes is_invalid_filename
        // (it is a single normal component), so the contract is "NotFound".
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        fs::write(
            domain
                .session_dir(&record.id)
                .join("artifacts")
                .join("with space.md"),
            "body",
        )
        .unwrap();

        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id,
                filename: "with space.md".into(),
            })
            .expect_err("space in name");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn get_artifact_missing_session_returns_session_not_found() {
        let (_temp, domain) = domain();
        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: "no-session".into(),
                filename: "plan.md".into(),
            })
            .expect_err("missing session");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn get_artifact_missing_file_returns_not_found() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id,
                filename: "missing.md".into(),
            })
            .expect_err("missing file");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn get_artifact_rejects_symlink_as_not_found() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");

        // Place a real target outside the artifacts dir and symlink to it.
        let target = domain.session_dir(&record.id).join("secret.txt");
        fs::write(&target, "secret").unwrap();

        let link = domain.session_dir(&record.id).join("artifacts").join("link.md");
        symlink(&target, &link).unwrap();

        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id,
                filename: "link.md".into(),
            })
            .expect_err("symlink rejected");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn get_artifact_rejects_subdirectory_as_not_found() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        fs::create_dir(
            domain
                .session_dir(&record.id)
                .join("artifacts")
                .join("nested.md"),
        )
        .unwrap();

        let err = domain
            .get_artifact(PlanningGetArtifactParams {
                session_id: record.id,
                filename: "nested.md".into(),
            })
            .expect_err("directory rejected");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    // ----- read_conversation / read_artifacts edge cases (private helpers) -----

    #[test]
    fn read_conversation_ignores_blank_and_malformed_lines() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");

        // Write a mix of valid, blank, and malformed lines directly.
        let path = domain.conversation_path(&record.id);
        let valid = serde_json::to_string(&ConversationEntry {
            entry_type: "user_prompt".into(),
            id: "p".into(),
            text: "hi".into(),
            ts: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
        let content = format!("{valid}\n\n   \nnot json\n{valid}\n");
        fs::write(&path, content).unwrap();

        let entries = domain.read_conversation(&record.id);
        assert_eq!(entries.len(), 2, "blanks and malformed lines are dropped");
        assert!(entries.iter().all(|e| e.entry_type == "prompt"));
    }

    #[test]
    fn read_conversation_returns_empty_when_file_missing() {
        let (_temp, domain) = domain();
        // No session on disk at all — should yield no panic and empty vec.
        assert!(domain.read_conversation("nonexistent").is_empty());
    }

    #[test]
    fn count_messages_returns_zero_when_file_missing() {
        let (_temp, domain) = domain();
        assert_eq!(domain.count_messages("nonexistent"), 0);
    }

    #[test]
    fn read_artifacts_returns_empty_when_directory_missing() {
        let (_temp, domain) = domain();
        assert!(domain.read_artifacts("nonexistent").is_empty());
    }

    #[test]
    fn read_artifacts_filters_hidden_and_sorts_results() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        let artifacts = domain.session_dir(&record.id).join("artifacts");
        fs::write(artifacts.join("b.md"), "b").unwrap();
        fs::write(artifacts.join("a.md"), "a").unwrap();
        fs::write(artifacts.join(".hidden"), "h").unwrap();
        // Subdirectory should not appear.
        fs::create_dir(artifacts.join("sub")).unwrap();

        let names = domain.read_artifacts(&record.id);
        assert_eq!(names, vec!["a.md".to_string(), "b.md".to_string()]);
    }

    #[test]
    fn read_artifacts_excludes_symlinks() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        let artifacts = domain.session_dir(&record.id).join("artifacts");
        fs::write(artifacts.join("real.md"), "r").unwrap();

        let target = domain.session_dir(&record.id).join("outside.md");
        fs::write(&target, "t").unwrap();
        symlink(&target, artifacts.join("link.md")).unwrap();

        assert_eq!(domain.read_artifacts(&record.id), vec!["real.md".to_string()]);
    }

    #[test]
    fn read_metadata_invalid_id_returns_invalid_params() {
        let (_temp, domain) = domain();
        let err = domain.read_metadata("bad id").expect_err("invalid id");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn read_metadata_corrupt_file_returns_internal_error() {
        let (_temp, mut domain) = domain();
        let record = domain
            .start(PlanningStartParams { prompt: "P".into() })
            .expect("start");
        fs::write(domain.metadata_path(&record.id), "not valid json").unwrap();
        let err = domain.read_metadata(&record.id).expect_err("corrupt");
        assert_eq!(err.code, RpcErrorCode::Internal);
    }
}
