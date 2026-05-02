mod dispatch;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tracing::debug;

use crate::auth::{Authenticator, from_config};
use crate::collection_domain::CollectionDomain;
use crate::config::ApiConfig;
use crate::config_domain::ConfigDomain;
use crate::errors::{ApiError, RpcErrorCode};
use crate::idempotency::{
    IdempotencyCheck, IdempotencyStore, InMemoryIdempotencyStore, StoredResponse,
};
use crate::loop_domain::LoopDomain;
use crate::planning_domain::PlanningDomain;
use crate::preset_domain::PresetDomain;
use crate::protocol::{
    API_VERSION, KNOWN_METHODS, RpcRequestEnvelope, STREAM_TOPICS, error_envelope, is_known_method,
    is_mutating_method, parse_json_value, parse_request, request_context, success_envelope,
    validate_request_schema,
};
use crate::stream_domain::StreamDomain;
use crate::task_domain::TaskDomain;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdOnlyParams {
    pub(crate) id: String,
}

#[derive(Clone)]
pub struct RpcRuntime {
    pub(crate) config: ApiConfig,
    auth: Arc<dyn Authenticator>,
    idempotency: Arc<dyn IdempotencyStore>,
    tasks: Arc<Mutex<TaskDomain>>,
    loops: Arc<Mutex<LoopDomain>>,
    planning: Arc<Mutex<PlanningDomain>>,
    collections: Arc<Mutex<CollectionDomain>>,
    streams: StreamDomain,
    config_domain: ConfigDomain,
    preset_domain: PresetDomain,
}

enum ExecutionOutcome {
    Fresh(Value),
    Replay(StoredResponse),
}

impl RpcRuntime {
    pub fn new(config: ApiConfig) -> anyhow::Result<Self> {
        config.validate()?;

        let auth = from_config(&config)?;
        let idempotency = Arc::new(InMemoryIdempotencyStore::new(Duration::from_secs(
            config.idempotency_ttl_secs,
        )));

        Ok(Self::with_components(config, auth, idempotency))
    }

    pub fn with_components(
        config: ApiConfig,
        auth: Arc<dyn Authenticator>,
        idempotency: Arc<dyn IdempotencyStore>,
    ) -> Self {
        let tasks = Arc::new(Mutex::new(TaskDomain::new(&config.workspace_root)));
        let loops = Arc::new(Mutex::new(LoopDomain::new(
            &config.workspace_root,
            config.loop_process_interval_ms,
            config.ralph_command.clone(),
        )));
        let planning = Arc::new(Mutex::new(PlanningDomain::new(&config.workspace_root)));
        let collections = Arc::new(Mutex::new(CollectionDomain::new(&config.workspace_root)));
        let streams = StreamDomain::new();
        let config_domain = ConfigDomain::new(&config.workspace_root);
        let preset_domain = PresetDomain::new(&config.workspace_root);

        Self {
            config,
            auth,
            idempotency,
            tasks,
            loops,
            planning,
            collections,
            streams,
            config_domain,
            preset_domain,
        }
    }

    pub fn health_payload(&self) -> Value {
        json!({
            "status": "ok",
            "timestamp": crate::loop_support::now_ts()
        })
    }

    pub fn capabilities_payload(&self) -> Value {
        json!({
            "methods": KNOWN_METHODS,
            "streamTopics": STREAM_TOPICS,
            "auth": {
                "mode": self.auth.mode().as_contract_mode(),
                "supportedModes": ["trusted_local", "token"]
            },
            "idempotency": {
                "requiredForMutations": true,
                "retentionSeconds": self.config.idempotency_ttl_secs
            }
        })
    }

    pub fn invoke_method(
        &self,
        request_id: impl Into<String>,
        method: &str,
        params: Value,
        principal: &str,
        idempotency_key: Option<String>,
    ) -> Result<Value, ApiError> {
        let request_id = request_id.into();
        let mut raw = json!({
            "apiVersion": API_VERSION,
            "id": request_id,
            "method": method,
            "params": params,
        });

        if let Some(idempotency_key) = idempotency_key {
            raw["meta"] = json!({
                "idempotencyKey": idempotency_key,
            });
        }

        let request = self.parse_and_validate_request_value(raw)?;
        match self.execute_request(&request, principal)? {
            ExecutionOutcome::Fresh(result) => Ok(result),
            ExecutionOutcome::Replay(response) => self.replay_stored_response(&request, response),
        }
    }

    pub fn handle_http_request(&self, body: &[u8], headers: &HeaderMap) -> (StatusCode, Value) {
        let request = match self.parse_and_validate_request(body) {
            Ok(request) => request,
            Err(error) => {
                let status = error.status;
                let envelope = error_envelope(&error, &self.config.served_by);
                return (status, envelope);
            }
        };

        let principal =
            match self.auth.authorize(&request, headers).map_err(|error| {
                error.with_context(request.id.clone(), Some(request.method.clone()))
            }) {
                Ok(p) => p,
                Err(error) => {
                    let status = error.status;
                    let envelope = error_envelope(&error, &self.config.served_by);
                    return (status, envelope);
                }
            };

        let (status, envelope) = match self.execute_request(&request, &principal) {
            Ok(ExecutionOutcome::Fresh(result)) => (
                StatusCode::OK,
                success_envelope(&request, result, &self.config.served_by),
            ),
            Ok(ExecutionOutcome::Replay(response)) => (
                StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK),
                response.envelope,
            ),
            Err(error) => {
                let status = error.status;
                let envelope = error_envelope(&error, &self.config.served_by);
                (status, envelope)
            }
        };

        (status, envelope)
    }

    pub fn authenticate_websocket(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let dummy_request = crate::protocol::RpcRequestEnvelope {
            api_version: "v1".to_string(),
            id: "ws-upgrade".to_string(),
            method: "stream.subscribe".to_string(),
            params: serde_json::Value::Object(serde_json::Map::new()),
            meta: None,
        };

        self.auth
            .authorize(&dummy_request, headers)
            .map_err(|error| error.with_context("ws-upgrade", Some("stream.subscribe".to_string())))
    }

    pub(crate) fn task_domain_mut(&self) -> Result<MutexGuard<'_, TaskDomain>, ApiError> {
        self.tasks
            .lock()
            .map_err(|_| ApiError::internal("task domain lock poisoned"))
    }

    pub(crate) fn loop_domain_mut(&self) -> Result<MutexGuard<'_, LoopDomain>, ApiError> {
        self.loops
            .lock()
            .map_err(|_| ApiError::internal("loop domain lock poisoned"))
    }

    pub(crate) fn planning_domain_mut(&self) -> Result<MutexGuard<'_, PlanningDomain>, ApiError> {
        self.planning
            .lock()
            .map_err(|_| ApiError::internal("planning domain lock poisoned"))
    }

    pub(crate) fn collection_domain_mut(
        &self,
    ) -> Result<MutexGuard<'_, CollectionDomain>, ApiError> {
        self.collections
            .lock()
            .map_err(|_| ApiError::internal("collection domain lock poisoned"))
    }

    pub(crate) fn stream_domain(&self) -> StreamDomain {
        self.streams.clone()
    }

    pub(crate) fn config_domain(&self) -> &ConfigDomain {
        &self.config_domain
    }

    pub(crate) fn preset_domain(&self) -> &PresetDomain {
        &self.preset_domain
    }

    pub(crate) fn parse_params<T>(&self, request: &RpcRequestEnvelope) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(request.params.clone()).map_err(|error| {
            ApiError::invalid_params(format!(
                "invalid params for method '{}': {error}",
                request.method
            ))
        })
    }

    fn parse_and_validate_request(&self, body: &[u8]) -> Result<RpcRequestEnvelope, ApiError> {
        let raw = parse_json_value(body)?;
        self.parse_and_validate_request_value(raw)
    }

    fn parse_and_validate_request_value(&self, raw: Value) -> Result<RpcRequestEnvelope, ApiError> {
        let (request_id, method) = request_context(&raw);

        if !raw.is_object() {
            return Err(
                ApiError::invalid_request("request body must be a JSON object")
                    .with_context(request_id, method),
            );
        }

        let method = method.ok_or_else(|| {
            ApiError::invalid_request("missing required field 'method'")
                .with_context(request_id.clone(), None)
        })?;

        if !is_known_method(&method) {
            return Err(
                ApiError::method_not_found(method.clone()).with_context(request_id, Some(method))
            );
        }

        if let Err(errors) = validate_request_schema(&raw) {
            return Err(
                ApiError::invalid_params("request does not match rpc-v1 schema")
                    .with_context(request_id, Some(method))
                    .with_details(json!({ "errors": errors })),
            );
        }

        let request = parse_request(&raw)
            .map_err(|error| error.with_context(request_id.clone(), Some(method.clone())))?;

        if request.api_version != API_VERSION {
            return Err(ApiError::invalid_request(format!(
                "unsupported apiVersion '{}'; expected '{API_VERSION}'",
                request.api_version
            ))
            .with_context(request.id, Some(request.method)));
        }

        Ok(request)
    }

    fn execute_request(
        &self,
        request: &RpcRequestEnvelope,
        principal: &str,
    ) -> Result<ExecutionOutcome, ApiError> {
        let mut idempotency_context: Option<String> = None;
        if is_mutating_method(&request.method) {
            let key = match request
                .meta
                .as_ref()
                .and_then(|meta| meta.idempotency_key.as_deref())
            {
                Some(key) => key,
                None => {
                    return Err(ApiError::invalid_params(
                        "mutating methods require meta.idempotencyKey",
                    )
                    .with_context(request.id.clone(), Some(request.method.clone())));
                }
            };

            match self
                .idempotency
                .check(&request.method, key, &request.params)
            {
                IdempotencyCheck::Replay(response) => {
                    debug!(
                        method = %request.method,
                        request_id = %request.id,
                        "idempotency replay"
                    );
                    return Ok(ExecutionOutcome::Replay(response));
                }
                IdempotencyCheck::Conflict => {
                    return Err(ApiError::idempotency_conflict(
                        "idempotency key was already used with different parameters",
                    )
                    .with_context(request.id.clone(), Some(request.method.clone()))
                    .with_details(json!({
                        "method": request.method.clone(),
                        "idempotencyKey": key
                    })));
                }
                IdempotencyCheck::New => {
                    idempotency_context = Some(key.to_string());
                }
            }
        }

        let result = self
            .dispatch(request, principal)
            .map_err(|error| error.with_context(request.id.clone(), Some(request.method.clone())));

        if let Some(key) = idempotency_context {
            let (status, envelope) = match &result {
                Ok(value) => (
                    StatusCode::OK.as_u16(),
                    success_envelope(request, value.clone(), &self.config.served_by),
                ),
                Err(error) => (
                    error.status.as_u16(),
                    error_envelope(error, &self.config.served_by),
                ),
            };
            self.idempotency.store(
                &request.method,
                &key,
                &request.params,
                &StoredResponse { status, envelope },
            );
        }

        result.map(ExecutionOutcome::Fresh)
    }

    fn replay_stored_response(
        &self,
        request: &RpcRequestEnvelope,
        response: StoredResponse,
    ) -> Result<Value, ApiError> {
        if let Some(result) = response.envelope.get("result") {
            return Ok(result.clone());
        }

        let error_body = response
            .envelope
            .get("error")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ApiError::internal("stored idempotency response was missing an error payload")
                    .with_context(request.id.clone(), Some(request.method.clone()))
            })?;

        let code = error_body
            .get("code")
            .and_then(Value::as_str)
            .and_then(RpcErrorCode::from_contract)
            .unwrap_or(RpcErrorCode::Internal);
        let message = error_body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("stored idempotency replay failed");
        let retryable = error_body
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut error = ApiError::new(code, message)
            .with_context(request.id.clone(), Some(request.method.clone()));
        error.retryable = retryable;
        error.details = error_body.get("details").cloned();
        error.status = StatusCode::from_u16(response.status).unwrap_or(error.status);
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::{HeaderMap, StatusCode, header};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use crate::config::{ApiConfig, AuthMode};
    use crate::errors::RpcErrorCode;
    use crate::idempotency::StoredResponse;
    use crate::protocol::API_VERSION;

    /// Build a `TrustedLocal` runtime rooted at a fresh temp workspace.
    fn test_runtime() -> (RpcRuntime, TempDir) {
        let workspace = tempfile::tempdir().expect("tempdir should be created");
        let mut config = ApiConfig::default();
        config.workspace_root = workspace.path().to_path_buf();
        let runtime = RpcRuntime::new(config).expect("runtime should initialize");
        (runtime, workspace)
    }

    /// Build a `Token`-auth runtime rooted at a fresh temp workspace.
    fn token_runtime(token: &str) -> (RpcRuntime, TempDir) {
        let workspace = tempfile::tempdir().expect("tempdir should be created");
        let mut config = ApiConfig::default();
        config.workspace_root = workspace.path().to_path_buf();
        config.auth_mode = AuthMode::Token;
        config.token = Some(token.to_string());
        let runtime = RpcRuntime::new(config).expect("runtime should initialize");
        (runtime, workspace)
    }

    /// Encode a well-formed request envelope as bytes.
    fn envelope_body(id: &str, method: &str, params: Value, meta: Option<Value>) -> Vec<u8> {
        let mut raw = json!({
            "apiVersion": API_VERSION,
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(meta) = meta {
            raw["meta"] = meta;
        }
        serde_json::to_vec(&raw).expect("envelope should serialize")
    }

    // ---------------------------------------------------------------------
    // RpcRuntime::new / with_components
    // ---------------------------------------------------------------------

    #[test]
    fn new_rejects_invalid_config() {
        let mut config = ApiConfig::default();
        // Non-loopback host + TrustedLocal is invalid.
        config.host = "0.0.0.0".to_string();
        let err = match RpcRuntime::new(config) {
            Ok(_) => panic!("expected non-loopback trusted_local to be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().to_lowercase().contains("loopback"),
            "error message should mention loopback restriction: {err}"
        );
    }

    #[test]
    fn new_succeeds_with_default_config() {
        let (runtime, _workspace) = test_runtime();
        assert_eq!(runtime.config.auth_mode, AuthMode::TrustedLocal);
    }

    // ---------------------------------------------------------------------
    // health_payload / capabilities_payload
    // ---------------------------------------------------------------------

    #[test]
    fn health_payload_includes_status_and_timestamp() {
        let (runtime, _workspace) = test_runtime();
        let payload = runtime.health_payload();
        assert_eq!(payload["status"], "ok");
        assert!(
            payload["timestamp"].is_string(),
            "timestamp should be a string"
        );
        let ts = payload["timestamp"].as_str().unwrap();
        assert!(!ts.is_empty(), "timestamp should not be empty");
    }

    #[test]
    fn capabilities_payload_reflects_trusted_local_auth() {
        let (runtime, _workspace) = test_runtime();
        let caps = runtime.capabilities_payload();
        assert!(caps["methods"].is_array(), "methods must be an array");
        assert!(
            caps["streamTopics"].is_array(),
            "streamTopics must be an array"
        );
        assert_eq!(caps["auth"]["mode"], "trusted_local");
        let supported: Vec<&str> = caps["auth"]["supportedModes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(supported.contains(&"trusted_local"));
        assert!(supported.contains(&"token"));
        assert_eq!(caps["idempotency"]["requiredForMutations"], true);
        assert_eq!(
            caps["idempotency"]["retentionSeconds"],
            runtime.config.idempotency_ttl_secs
        );
    }

    #[test]
    fn capabilities_payload_reflects_token_auth() {
        let (runtime, _workspace) = token_runtime("secret");
        let caps = runtime.capabilities_payload();
        assert_eq!(caps["auth"]["mode"], "token");
    }

    // ---------------------------------------------------------------------
    // invoke_method
    // ---------------------------------------------------------------------

    #[test]
    fn invoke_method_dispatches_system_health() {
        let (runtime, _workspace) = test_runtime();
        let result = runtime
            .invoke_method("req-1", "system.health", json!({}), "trusted_local", None)
            .expect("system.health should succeed");
        assert_eq!(result["status"], "ok");
    }

    #[test]
    fn invoke_method_rejects_unknown_method() {
        let (runtime, _workspace) = test_runtime();
        let err = runtime
            .invoke_method(
                "req-2",
                "nope.does.not.exist",
                json!({}),
                "trusted_local",
                None,
            )
            .expect_err("unknown method should error");
        assert_eq!(err.code, RpcErrorCode::MethodNotFound);
        // Context should be plumbed through.
        assert_eq!(err.request_id, "req-2");
        assert_eq!(err.method.as_deref(), Some("nope.does.not.exist"));
    }

    #[test]
    fn invoke_method_requires_idempotency_key_for_mutations() {
        let (runtime, _workspace) = test_runtime();
        // `task.clear` is a mutating method. Without an idempotency key, schema
        // validation (which requires meta.idempotencyKey for mutations) rejects
        // the request before domain dispatch.
        let err = runtime
            .invoke_method("req-3", "task.clear", json!({}), "trusted_local", None)
            .expect_err("mutation without idempotency key should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert_eq!(err.method.as_deref(), Some("task.clear"));
    }

    #[test]
    fn invoke_method_replays_on_same_idempotency_key() {
        let (runtime, _workspace) = test_runtime();
        let first = runtime
            .invoke_method(
                "req-4a",
                "task.clear",
                json!({}),
                "trusted_local",
                Some("idem-clear-1".to_string()),
            )
            .expect("first call should succeed");
        let second = runtime
            .invoke_method(
                "req-4b",
                "task.clear",
                json!({}),
                "trusted_local",
                Some("idem-clear-1".to_string()),
            )
            .expect("replay should succeed");
        assert_eq!(first, second, "replay should match stored response");
    }

    #[test]
    fn invoke_method_detects_idempotency_conflict() {
        let (runtime, _workspace) = test_runtime();
        // First call: well-formed task.create succeeds and stores the response under the key.
        runtime
            .invoke_method(
                "req-5a",
                "task.create",
                json!({ "id": "task-1", "title": "first", "autoExecute": false, "status": "open" }),
                "trusted_local",
                Some("idem-conflict".to_string()),
            )
            .expect("initial task.create should succeed");

        // Second call uses the same idempotency key but different params → CONFLICT.
        let err = runtime
            .invoke_method(
                "req-5b",
                "task.create",
                json!({ "id": "task-2", "title": "second", "autoExecute": false, "status": "open" }),
                "trusted_local",
                Some("idem-conflict".to_string()),
            )
            .expect_err("differing params on same key should conflict");
        assert_eq!(err.code, RpcErrorCode::IdempotencyConflict);
    }

    // ---------------------------------------------------------------------
    // handle_http_request
    // ---------------------------------------------------------------------

    #[test]
    fn handle_http_request_returns_success_envelope_for_health() {
        let (runtime, _workspace) = test_runtime();
        let body = envelope_body("req-http-1", "system.health", json!({}), None);
        let (status, envelope) = runtime.handle_http_request(&body, &HeaderMap::new());
        assert_eq!(status, StatusCode::OK);
        assert_eq!(envelope["id"], "req-http-1");
        assert_eq!(envelope["method"], "system.health");
        assert_eq!(envelope["apiVersion"], API_VERSION);
        assert_eq!(envelope["result"]["status"], "ok");
        // Meta is populated by the server.
        assert_eq!(envelope["meta"]["servedBy"], runtime.config.served_by);
    }

    #[test]
    fn handle_http_request_rejects_malformed_json() {
        let (runtime, _workspace) = test_runtime();
        let (status, envelope) = runtime.handle_http_request(b"not json", &HeaderMap::new());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(envelope["error"]["code"], "INVALID_REQUEST");
    }

    #[test]
    fn handle_http_request_rejects_unknown_method_via_envelope() {
        let (runtime, _workspace) = test_runtime();
        let body = envelope_body("req-http-2", "bogus.method", json!({}), None);
        let (status, envelope) = runtime.handle_http_request(&body, &HeaderMap::new());
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(envelope["error"]["code"], "METHOD_NOT_FOUND");
        assert_eq!(envelope["id"], "req-http-2");
    }

    #[test]
    fn handle_http_request_rejects_missing_token_when_token_auth() {
        let (runtime, _workspace) = token_runtime("secret-token");
        let body = envelope_body("req-http-3", "system.health", json!({}), None);
        // No Authorization header, no meta.auth → unauthorized.
        let (status, envelope) = runtime.handle_http_request(&body, &HeaderMap::new());
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(envelope["error"]["code"], "UNAUTHORIZED");
        assert_eq!(envelope["id"], "req-http-3");
    }

    #[test]
    fn handle_http_request_accepts_valid_bearer_token() {
        let (runtime, _workspace) = token_runtime("secret-token");
        let body = envelope_body("req-http-4", "system.health", json!({}), None);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer secret-token".parse().unwrap(),
        );

        let (status, envelope) = runtime.handle_http_request(&body, &headers);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(envelope["result"]["status"], "ok");
    }

    #[test]
    fn handle_http_request_rejects_wrong_api_version() {
        let (runtime, _workspace) = test_runtime();
        let raw = json!({
            "apiVersion": "v999",
            "id": "req-http-5",
            "method": "system.health",
            "params": {}
        });
        let body = serde_json::to_vec(&raw).unwrap();
        let (status, envelope) = runtime.handle_http_request(&body, &HeaderMap::new());
        // Schema validator enforces apiVersion="v1", so this is rejected as
        // INVALID_PARAMS before we reach the parsed-envelope version check.
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(envelope["error"]["code"], "INVALID_PARAMS");
    }

    #[test]
    fn handle_http_request_rejects_non_object_body() {
        let (runtime, _workspace) = test_runtime();
        // Valid JSON but not an object.
        let (status, envelope) = runtime.handle_http_request(b"[1,2,3]", &HeaderMap::new());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(envelope["error"]["code"], "INVALID_REQUEST");
    }

    #[test]
    fn handle_http_request_rejects_schema_violations() {
        let (runtime, _workspace) = test_runtime();
        // Missing required `params` field — schema violation but method is known.
        let raw = json!({
            "apiVersion": API_VERSION,
            "id": "req-http-6",
            "method": "system.health"
        });
        let body = serde_json::to_vec(&raw).unwrap();
        let (status, envelope) = runtime.handle_http_request(&body, &HeaderMap::new());
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(envelope["error"]["code"], "INVALID_PARAMS");
    }

    // ---------------------------------------------------------------------
    // authenticate_websocket
    // ---------------------------------------------------------------------

    #[test]
    fn authenticate_websocket_trusted_local_always_succeeds() {
        let (runtime, _workspace) = test_runtime();
        let principal = runtime
            .authenticate_websocket(&HeaderMap::new())
            .expect("trusted_local should authenticate without headers");
        assert_eq!(principal, "trusted_local");
    }

    #[test]
    fn authenticate_websocket_token_requires_valid_bearer() {
        let (runtime, _workspace) = token_runtime("ws-token");
        // No headers → fail.
        let err = runtime
            .authenticate_websocket(&HeaderMap::new())
            .expect_err("missing token should be unauthorized");
        assert_eq!(err.code, RpcErrorCode::Unauthorized);
        assert_eq!(err.request_id, "ws-upgrade");
        assert_eq!(err.method.as_deref(), Some("stream.subscribe"));

        // Correct bearer → succeed.
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer ws-token".parse().unwrap());
        let principal = runtime
            .authenticate_websocket(&headers)
            .expect("valid bearer should authenticate");
        assert_eq!(principal, "ws-token");

        // Wrong bearer → fail.
        let mut bad = HeaderMap::new();
        bad.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        let err = runtime
            .authenticate_websocket(&bad)
            .expect_err("wrong bearer must be rejected");
        assert_eq!(err.code, RpcErrorCode::Unauthorized);
    }

    // ---------------------------------------------------------------------
    // Domain accessors
    // ---------------------------------------------------------------------

    #[test]
    fn domain_accessors_return_live_guards() {
        let (runtime, _workspace) = test_runtime();
        assert!(runtime.task_domain_mut().is_ok());
        assert!(runtime.loop_domain_mut().is_ok());
        assert!(runtime.planning_domain_mut().is_ok());
        assert!(runtime.collection_domain_mut().is_ok());
        // These must produce references without panicking.
        let _ = runtime.config_domain();
        let _ = runtime.preset_domain();
        let _ = runtime.stream_domain();
    }

    // ---------------------------------------------------------------------
    // parse_params
    // ---------------------------------------------------------------------

    #[test]
    fn parse_params_extracts_id_only_params() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-parse-1".to_string(),
            method: "task.get".to_string(),
            params: json!({ "id": "task-123" }),
            meta: None,
        };
        let parsed: IdOnlyParams = runtime
            .parse_params(&request)
            .expect("id param should parse");
        assert_eq!(parsed.id, "task-123");
    }

    #[test]
    fn parse_params_rejects_bad_shape() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-parse-2".to_string(),
            method: "task.get".to_string(),
            params: json!({ "wrong": "shape" }),
            meta: None,
        };
        let err = runtime
            .parse_params::<IdOnlyParams>(&request)
            .expect_err("missing id should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("task.get"));
    }

    // ---------------------------------------------------------------------
    // replay_stored_response
    // ---------------------------------------------------------------------

    #[test]
    fn replay_stored_response_extracts_result_for_success() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-replay-1".to_string(),
            method: "task.clear".to_string(),
            params: json!({}),
            meta: None,
        };
        let response = StoredResponse {
            status: 200,
            envelope: json!({ "result": { "success": true } }),
        };
        let result = runtime
            .replay_stored_response(&request, response)
            .expect("success replay should return Ok");
        assert_eq!(result, json!({ "success": true }));
    }

    #[test]
    fn replay_stored_response_rebuilds_error_from_envelope() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-replay-2".to_string(),
            method: "task.clear".to_string(),
            params: json!({}),
            meta: None,
        };
        let response = StoredResponse {
            status: 409,
            envelope: json!({
                "error": {
                    "code": "CONFLICT",
                    "message": "already exists",
                    "retryable": false,
                    "details": { "resource": "task" }
                }
            }),
        };
        let err = runtime
            .replay_stored_response(&request, response)
            .expect_err("error replay should return Err");
        assert_eq!(err.code, RpcErrorCode::Conflict);
        assert_eq!(err.message, "already exists");
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert_eq!(err.request_id, "req-replay-2");
        assert_eq!(err.method.as_deref(), Some("task.clear"));
        assert_eq!(err.details, Some(json!({ "resource": "task" })));
        assert!(!err.retryable);
    }

    #[test]
    fn replay_stored_response_falls_back_to_internal_on_unknown_code() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-replay-3".to_string(),
            method: "task.clear".to_string(),
            params: json!({}),
            meta: None,
        };
        let response = StoredResponse {
            status: 500,
            envelope: json!({
                "error": {
                    "code": "GIBBERISH_CODE_NOT_IN_CONTRACT",
                    "message": "weird",
                    "retryable": true
                }
            }),
        };
        let err = runtime
            .replay_stored_response(&request, response)
            .expect_err("error replay should return Err");
        assert_eq!(err.code, RpcErrorCode::Internal);
        assert_eq!(err.message, "weird");
        assert!(err.retryable);
    }

    #[test]
    fn replay_stored_response_errors_when_envelope_missing_error_body() {
        let (runtime, _workspace) = test_runtime();
        let request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "req-replay-4".to_string(),
            method: "task.clear".to_string(),
            params: json!({}),
            meta: None,
        };
        // No `result` and no `error` field.
        let response = StoredResponse {
            status: 200,
            envelope: json!({ "meta": { "servedBy": "x" } }),
        };
        let err = runtime
            .replay_stored_response(&request, response)
            .expect_err("malformed envelope should produce Err");
        assert_eq!(err.code, RpcErrorCode::Internal);
        assert!(
            err.message.contains("idempotency"),
            "message should reference idempotency: {}",
            err.message
        );
    }

    // ---------------------------------------------------------------------
    // parse_and_validate_request_value
    // ---------------------------------------------------------------------

    #[test]
    fn parse_and_validate_rejects_non_object() {
        let (runtime, _workspace) = test_runtime();
        let err = runtime
            .parse_and_validate_request_value(json!(42))
            .expect_err("integer body should be rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidRequest);
        assert!(err.message.contains("JSON object"));
    }

    #[test]
    fn parse_and_validate_requires_method_field() {
        let (runtime, _workspace) = test_runtime();
        let err = runtime
            .parse_and_validate_request_value(json!({
                "apiVersion": API_VERSION,
                "id": "x",
                "params": {}
            }))
            .expect_err("missing method should error");
        assert_eq!(err.code, RpcErrorCode::InvalidRequest);
        assert!(err.message.to_lowercase().contains("method"));
    }

    #[test]
    fn parse_and_validate_rejects_unknown_method_name() {
        let (runtime, _workspace) = test_runtime();
        let err = runtime
            .parse_and_validate_request_value(json!({
                "apiVersion": API_VERSION,
                "id": "x",
                "method": "mystery.call",
                "params": {}
            }))
            .expect_err("unknown method should error");
        assert_eq!(err.code, RpcErrorCode::MethodNotFound);
    }

    #[test]
    fn parse_and_validate_accepts_valid_envelope() {
        let (runtime, _workspace) = test_runtime();
        let request = runtime
            .parse_and_validate_request_value(json!({
                "apiVersion": API_VERSION,
                "id": "ok",
                "method": "system.health",
                "params": {}
            }))
            .expect("valid envelope should parse");
        assert_eq!(request.id, "ok");
        assert_eq!(request.method, "system.health");
        assert_eq!(request.api_version, API_VERSION);
    }
}
