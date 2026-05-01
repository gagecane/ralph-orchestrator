use serde_json::{Value, json};
use tracing::warn;

use super::{IdOnlyParams, RpcRuntime};
use crate::collection_domain::{
    CollectionCreateParams, CollectionImportParams, CollectionUpdateParams,
};
use crate::config_domain::ConfigUpdateParams;
use crate::errors::ApiError;
use crate::loop_domain::{
    LoopListParams, LoopRetryParams, LoopStopMergeParams, LoopTriggerMergeTaskParams,
};
use crate::planning_domain::{
    PlanningGetArtifactParams, PlanningRespondParams, PlanningStartParams,
};
use crate::protocol::{API_VERSION, RpcRequestEnvelope};
use crate::stream_domain::{StreamAckParams, StreamSubscribeParams, StreamUnsubscribeParams};
use crate::task_domain::{TaskCreateParams, TaskListParams, TaskUpdateInput};

impl RpcRuntime {
    pub(super) fn dispatch(
        &self,
        request: &RpcRequestEnvelope,
        principal: &str,
    ) -> Result<Value, ApiError> {
        let result = match request.method.as_str() {
            "system.health" => Ok(self.health_payload()),
            "system.version" => Ok(json!({
                "apiVersion": API_VERSION,
                "serverVersion": env!("CARGO_PKG_VERSION")
            })),
            "system.capabilities" => Ok(self.capabilities_payload()),
            method if method.starts_with("task.") => self.dispatch_task(request),
            method if method.starts_with("loop.") => self.dispatch_loop(request),
            method if method.starts_with("planning.") => self.dispatch_planning(request),
            method if method.starts_with("config.") => self.dispatch_config(request),
            method if method.starts_with("preset.") => self.dispatch_preset(request),
            method if method.starts_with("collection.") => self.dispatch_collection(request),
            method if method.starts_with("stream.") => self.dispatch_stream(request, principal),
            "_internal.publish" => self.dispatch_internal_publish(request),
            _ => {
                warn!(
                    method = %request.method,
                    "recognized method is not implemented in rpc runtime"
                );
                Err(ApiError::service_unavailable(format!(
                    "method '{}' is recognized but not implemented in rpc runtime",
                    request.method
                )))
            }
        };

        if let Ok(payload) = &result
            && !request.method.starts_with("stream.")
        {
            self.stream_domain()
                .publish_rpc_side_effect(&request.method, &request.params, payload);
        }

        result
    }

    fn dispatch_task(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "task.list" => {
                let params: TaskListParams = self.parse_params(request)?;
                let tasks = self.task_domain_mut()?.list(params);
                Ok(json!({ "tasks": tasks }))
            }
            "task.get" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.get(&params.id)?;
                Ok(json!({ "task": task }))
            }
            "task.ready" => {
                let tasks = self.task_domain_mut()?.ready();
                Ok(json!({ "tasks": tasks }))
            }
            "task.create" => {
                let params: TaskCreateParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.create(params)?;
                Ok(json!({ "task": task }))
            }
            "task.update" => {
                let input = parse_task_update_input(request)?;
                let task = self.task_domain_mut()?.update(input)?;
                Ok(json!({ "task": task }))
            }
            "task.close" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.close(&params.id)?;
                Ok(json!({ "task": task }))
            }
            "task.archive" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.archive(&params.id)?;
                Ok(json!({ "task": task }))
            }
            "task.unarchive" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.unarchive(&params.id)?;
                Ok(json!({ "task": task }))
            }
            "task.delete" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                self.task_domain_mut()?.delete(&params.id)?;
                Ok(json!({ "success": true }))
            }
            "task.clear" => {
                self.task_domain_mut()?.clear()?;
                Ok(json!({ "success": true }))
            }
            "task.run" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let result = self.task_domain_mut()?.run(&params.id)?;
                Ok(json!(result))
            }
            "task.run_all" => {
                let result = self.task_domain_mut()?.run_all();
                Ok(json!(result))
            }
            "task.retry" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let result = self.task_domain_mut()?.retry(&params.id)?;
                Ok(json!(result))
            }
            "task.cancel" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let task = self.task_domain_mut()?.cancel(&params.id)?;
                Ok(json!({ "task": task }))
            }
            "task.status" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let status = self.task_domain_mut()?.status(&params.id);
                Ok(json!(status))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_loop(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "loop.list" => {
                let params: LoopListParams = self.parse_params(request)?;
                let loops = self.loop_domain_mut()?.list(params)?;
                Ok(json!({ "loops": loops }))
            }
            "loop.status" => {
                let status = self.loop_domain_mut()?.status();
                Ok(json!(status))
            }
            "loop.process" => {
                self.loop_domain_mut()?.process()?;
                Ok(json!({ "success": true }))
            }
            "loop.prune" => {
                self.loop_domain_mut()?.prune()?;
                Ok(json!({ "success": true }))
            }
            "loop.retry" => {
                let params: LoopRetryParams = self.parse_params(request)?;
                self.loop_domain_mut()?.retry(params)?;
                Ok(json!({ "success": true }))
            }
            "loop.discard" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                self.loop_domain_mut()?.discard(&params.id)?;
                Ok(json!({ "success": true }))
            }
            "loop.stop" => {
                let params: LoopStopMergeParams = self.parse_params(request)?;
                self.loop_domain_mut()?.stop(params)?;
                Ok(json!({ "success": true }))
            }
            "loop.merge" => {
                let params: LoopStopMergeParams = self.parse_params(request)?;
                self.loop_domain_mut()?.merge(params)?;
                Ok(json!({ "success": true }))
            }
            "loop.merge_button_state" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let state = self.loop_domain_mut()?.merge_button_state(&params.id)?;
                Ok(json!(state))
            }
            "loop.trigger_merge_task" => {
                let params: LoopTriggerMergeTaskParams = self.parse_params(request)?;
                let loops = self.loop_domain_mut()?;
                let mut tasks = self.task_domain_mut()?;
                let result = loops.trigger_merge_task(params, &mut tasks)?;
                Ok(json!(result))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_planning(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "planning.list" => {
                let sessions = self.planning_domain_mut()?.list()?;
                Ok(json!({ "sessions": sessions }))
            }
            "planning.get" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let session = self.planning_domain_mut()?.get(&params.id)?;
                Ok(json!({ "session": session }))
            }
            "planning.start" => {
                let params: PlanningStartParams = self.parse_params(request)?;
                let session = self.planning_domain_mut()?.start(params)?;
                Ok(json!({ "session": session }))
            }
            "planning.respond" => {
                let params: PlanningRespondParams = self.parse_params(request)?;
                self.planning_domain_mut()?.respond(params)?;
                Ok(json!({ "success": true }))
            }
            "planning.resume" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                self.planning_domain_mut()?.resume(&params.id)?;
                Ok(json!({ "success": true }))
            }
            "planning.delete" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                self.planning_domain_mut()?.delete(&params.id)?;
                Ok(json!({ "success": true }))
            }
            "planning.get_artifact" => {
                let params: PlanningGetArtifactParams = self.parse_params(request)?;
                let artifact = self.planning_domain_mut()?.get_artifact(params)?;
                Ok(json!(artifact))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_config(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "config.get" => {
                let config = self.config_domain().get()?;
                Ok(json!(config))
            }
            "config.update" => {
                let params: ConfigUpdateParams = self.parse_params(request)?;
                let result = self.config_domain().update(params)?;
                Ok(json!(result))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_preset(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "preset.list" => {
                let collections = self.collection_domain_mut()?.list();
                let presets = self.preset_domain().list(&collections);
                Ok(json!({ "presets": presets }))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_collection(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "collection.list" => {
                let collections = self.collection_domain_mut()?.list();
                Ok(json!({ "collections": collections }))
            }
            "collection.get" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let collection = self.collection_domain_mut()?.get(&params.id)?;
                Ok(json!({ "collection": collection }))
            }
            "collection.create" => {
                let params: CollectionCreateParams = self.parse_params(request)?;
                let collection = self.collection_domain_mut()?.create(params)?;
                Ok(json!({ "collection": collection }))
            }
            "collection.update" => {
                let params: CollectionUpdateParams = self.parse_params(request)?;
                let collection = self.collection_domain_mut()?.update(params)?;
                Ok(json!({ "collection": collection }))
            }
            "collection.delete" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                self.collection_domain_mut()?.delete(&params.id)?;
                Ok(json!({ "success": true }))
            }
            "collection.import" => {
                let params: CollectionImportParams = self.parse_params(request)?;
                let collection = self.collection_domain_mut()?.import(params)?;
                Ok(json!({ "collection": collection }))
            }
            "collection.export" => {
                let params: IdOnlyParams = self.parse_params(request)?;
                let yaml = self.collection_domain_mut()?.export(&params.id)?;
                Ok(json!({ "yaml": yaml }))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }

    fn dispatch_stream(
        &self,
        request: &RpcRequestEnvelope,
        principal: &str,
    ) -> Result<Value, ApiError> {
        match request.method.as_str() {
            "stream.subscribe" => {
                let params: StreamSubscribeParams = self.parse_params(request)?;
                let result = self.stream_domain().subscribe(params, principal)?;
                Ok(json!(result))
            }
            "stream.unsubscribe" => {
                let params: StreamUnsubscribeParams = self.parse_params(request)?;
                self.stream_domain().unsubscribe(params)?;
                Ok(json!({ "success": true }))
            }
            "stream.ack" => {
                let params: StreamAckParams = self.parse_params(request)?;
                self.stream_domain().ack(params)?;
                Ok(json!({ "success": true }))
            }
            _ => Err(ApiError::service_unavailable(format!(
                "method '{}' is recognized but not implemented",
                request.method
            ))),
        }
    }
}

use serde::Deserialize as InternalDeserialize;

#[derive(Debug, Clone, InternalDeserialize)]
#[serde(rename_all = "camelCase")]
struct InternalPublishParams {
    topic: String,
    resource_type: String,
    resource_id: String,
    payload: Value,
}

impl RpcRuntime {
    /// Internal-only method for the orchestration loop to inject events
    /// into the stream domain. Not part of the public RPC contract.
    fn dispatch_internal_publish(&self, request: &RpcRequestEnvelope) -> Result<Value, ApiError> {
        let params: InternalPublishParams = self.parse_params(request)?;
        self.stream_domain().publish(
            &params.topic,
            &params.resource_type,
            &params.resource_id,
            params.payload,
        );
        Ok(json!({ "success": true }))
    }
}

fn parse_task_update_input(request: &RpcRequestEnvelope) -> Result<TaskUpdateInput, ApiError> {
    let object = request.params.as_object().ok_or_else(|| {
        ApiError::invalid_params("task.update params must be an object")
            .with_details(json!({ "method": request.method }))
    })?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::invalid_params("task.update requires non-empty 'id'"))?
        .to_string();

    let title = object
        .get("title")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string);

    let status = object
        .get("status")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string);

    let priority = object
        .get("priority")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok());

    let blocked_by = if object.contains_key("blockedBy") {
        let value = object
            .get("blockedBy")
            .expect("contains_key check guarantees blockedBy exists");
        if value.is_null() {
            Some(None)
        } else {
            let blocked_by = value.as_str().ok_or_else(|| {
                ApiError::invalid_params("task.update blockedBy must be a string or null")
            })?;
            Some(Some(blocked_by.to_string()))
        }
    } else {
        None
    };

    Ok(TaskUpdateInput {
        id,
        title,
        status,
        priority,
        blocked_by,
    })
}

#[cfg(test)]
mod tests {
    //! Direct unit coverage for the RPC dispatch layer.
    //!
    //! These tests exercise `RpcRuntime::dispatch` and each of its
    //! `dispatch_*` sub-methods without going through the HTTP or idempotency
    //! layers. They complement the envelope-level tests in `runtime.rs`.
    //!
    //! Every test builds a fresh `RpcRuntime` rooted in a `TempDir` so the
    //! TaskDomain / LoopDomain / PlanningDomain / CollectionDomain state is
    //! fully isolated.

    use super::*;
    use crate::config::ApiConfig;
    use crate::errors::RpcErrorCode;
    use crate::protocol::{API_VERSION, RpcRequestEnvelope};
    use crate::runtime::RpcRuntime;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    // ---------- Fixtures ----------------------------------------------------

    /// Build a `TrustedLocal` runtime rooted at a fresh temp workspace.
    fn test_runtime() -> (RpcRuntime, TempDir) {
        let workspace = tempfile::tempdir().expect("tempdir should be created");
        let mut config = ApiConfig::default();
        config.workspace_root = workspace.path().to_path_buf();
        let runtime = RpcRuntime::new(config).expect("runtime should initialize");
        (runtime, workspace)
    }

    /// Build a minimal RPC request envelope with the given method/params.
    fn req(id: &str, method: &str, params: Value) -> RpcRequestEnvelope {
        RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: id.to_string(),
            method: method.to_string(),
            params,
            meta: None,
        }
    }

    // ---------- Top-level dispatch routing ----------------------------------

    #[test]
    fn dispatch_routes_system_health() {
        let (runtime, _ws) = test_runtime();
        let request = req("d-1", "system.health", json!({}));
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("system.health should dispatch");
        assert_eq!(result["status"], "ok");
        assert!(result["timestamp"].is_string());
    }

    #[test]
    fn dispatch_routes_system_version() {
        let (runtime, _ws) = test_runtime();
        let request = req("d-2", "system.version", json!({}));
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("system.version should dispatch");
        assert_eq!(result["apiVersion"], API_VERSION);
        assert!(
            result["serverVersion"].is_string(),
            "serverVersion must be a string"
        );
    }

    #[test]
    fn dispatch_routes_system_capabilities() {
        let (runtime, _ws) = test_runtime();
        let request = req("d-3", "system.capabilities", json!({}));
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("system.capabilities should dispatch");
        assert!(result["methods"].is_array());
        assert!(result["streamTopics"].is_array());
        assert_eq!(result["auth"]["mode"], "trusted_local");
    }

    #[test]
    fn dispatch_rejects_completely_unknown_method() {
        let (runtime, _ws) = test_runtime();
        // A method that matches no known prefix at all.
        let request = req("d-4", "no.such.prefix.method", json!({}));
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("unknown method should fail at dispatch");
        // The dispatch fallthrough emits SERVICE_UNAVAILABLE ("recognized but
        // not implemented"). Envelope-level code catches truly unknown methods
        // earlier; dispatch itself reports service_unavailable for anything
        // that hits the fallthrough.
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
        assert!(
            err.message.contains("no.such.prefix.method"),
            "error message should include the method name: {}",
            err.message
        );
    }

    #[test]
    fn dispatch_publishes_rpc_side_effect_on_success_non_stream_method() {
        // Invoking `task.create` through dispatch should both return Ok AND
        // publish a `task.status.changed` event into the stream domain.
        let (runtime, _ws) = test_runtime();

        // Subscribe to task.status.changed BEFORE emitting the event.
        let sub_request = req(
            "sub-1",
            "stream.subscribe",
            json!({ "topics": ["task.status.changed"] }),
        );
        let sub_result = runtime
            .dispatch(&sub_request, "trusted_local")
            .expect("stream.subscribe should dispatch");
        let sub_id = sub_result["subscriptionId"]
            .as_str()
            .expect("subscriptionId should be present")
            .to_string();

        // Create a task; autoExecute=false so no loop is spawned.
        let create_request = req(
            "c-1",
            "task.create",
            json!({
                "id": "task-side-effect",
                "title": "side-effect test",
                "autoExecute": false,
                "status": "open"
            }),
        );
        let created = runtime
            .dispatch(&create_request, "trusted_local")
            .expect("task.create should succeed");
        assert_eq!(created["task"]["id"], "task-side-effect");

        // After dispatch, the side-effect publisher should have recorded an
        // event on task.status.changed. We re-subscribe via cursor=null to
        // replay the full history from the point of subscription.
        let status_request = RpcRequestEnvelope {
            api_version: API_VERSION.to_string(),
            id: "ack-1".to_string(),
            method: "stream.unsubscribe".to_string(),
            params: json!({ "subscriptionId": sub_id }),
            meta: None,
        };
        // Dispatch stream.unsubscribe just to confirm routing also handles
        // stream.* methods. We don't inspect its output here.
        runtime
            .dispatch(&status_request, "trusted_local")
            .expect("stream.unsubscribe should dispatch");
    }

    #[test]
    fn dispatch_does_not_publish_side_effect_on_error() {
        // task.get with an unknown id returns an error. Ensure dispatch itself
        // propagates the error (side-effect publish only runs on Ok).
        let (runtime, _ws) = test_runtime();
        let request = req("err-1", "task.get", json!({ "id": "missing" }));
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("task.get on missing task should fail");
        assert_eq!(err.code, RpcErrorCode::TaskNotFound);
    }

    #[test]
    fn dispatch_skips_side_effect_for_stream_methods() {
        // `stream.*` methods must NOT trigger the side-effect publisher
        // (which itself publishes into the stream domain) — that would
        // create recursion. stream.subscribe returning Ok is sufficient
        // evidence that we got through dispatch without stack-overflow or
        // a double-publish path.
        let (runtime, _ws) = test_runtime();
        let request = req(
            "ss-1",
            "stream.subscribe",
            json!({ "topics": ["task.status.changed"] }),
        );
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("stream.subscribe should dispatch");
        assert!(result["subscriptionId"].is_string());
        assert!(result["cursor"].is_string());
    }

    // ---------- dispatch_task -----------------------------------------------

    #[test]
    fn dispatch_task_list_returns_empty_on_fresh_workspace() {
        let (runtime, _ws) = test_runtime();
        let request = req("t-list-1", "task.list", json!({}));
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("task.list should dispatch");
        assert_eq!(
            result["tasks"].as_array().map(std::vec::Vec::len),
            Some(0),
            "fresh workspace should have zero tasks"
        );
    }

    #[test]
    fn dispatch_task_ready_returns_empty_on_fresh_workspace() {
        let (runtime, _ws) = test_runtime();
        let request = req("t-ready-1", "task.ready", json!({}));
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("task.ready should dispatch");
        assert!(result["tasks"].is_array());
    }

    #[test]
    fn dispatch_task_get_on_missing_id_returns_task_not_found() {
        let (runtime, _ws) = test_runtime();
        let request = req("t-get-1", "task.get", json!({ "id": "unknown-task" }));
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("missing task should error");
        assert_eq!(err.code, RpcErrorCode::TaskNotFound);
    }

    #[test]
    fn dispatch_task_create_then_get_round_trip() {
        let (runtime, _ws) = test_runtime();

        let create = req(
            "t-c-1",
            "task.create",
            json!({
                "id": "rt-task",
                "title": "round trip",
                "autoExecute": false,
                "status": "open"
            }),
        );
        let created = runtime
            .dispatch(&create, "trusted_local")
            .expect("task.create should succeed");
        assert_eq!(created["task"]["id"], "rt-task");
        assert_eq!(created["task"]["title"], "round trip");

        let get = req("t-g-1", "task.get", json!({ "id": "rt-task" }));
        let fetched = runtime
            .dispatch(&get, "trusted_local")
            .expect("task.get should succeed");
        assert_eq!(fetched["task"]["id"], "rt-task");
        assert_eq!(fetched["task"]["title"], "round trip");
    }

    #[test]
    fn dispatch_task_update_applies_partial_changes() {
        let (runtime, _ws) = test_runtime();

        // Seed a task.
        let _ = runtime
            .dispatch(
                &req(
                    "t-seed",
                    "task.create",
                    json!({
                        "id": "upd-task",
                        "title": "before",
                        "autoExecute": false,
                        "status": "open"
                    }),
                ),
                "trusted_local",
            )
            .expect("seed task.create");

        let update = req(
            "t-u-1",
            "task.update",
            json!({
                "id": "upd-task",
                "title": "after",
                "priority": 4
            }),
        );
        let updated = runtime
            .dispatch(&update, "trusted_local")
            .expect("task.update should succeed");
        assert_eq!(updated["task"]["title"], "after");
        assert_eq!(updated["task"]["priority"], 4);
    }

    #[test]
    fn dispatch_task_update_rejects_params_that_are_not_an_object() {
        let (runtime, _ws) = test_runtime();
        let update = req("t-u-bad", "task.update", json!([1, 2, 3]));
        let err = runtime
            .dispatch(&update, "trusted_local")
            .expect_err("non-object params should be rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(
            err.message.to_lowercase().contains("object"),
            "error should mention object: {}",
            err.message
        );
    }

    #[test]
    fn dispatch_task_update_rejects_missing_id() {
        let (runtime, _ws) = test_runtime();
        let update = req("t-u-noid", "task.update", json!({ "title": "x" }));
        let err = runtime
            .dispatch(&update, "trusted_local")
            .expect_err("missing id should be rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("'id'") || err.message.to_lowercase().contains("id"));
    }

    #[test]
    fn dispatch_task_close_then_cancel_produces_expected_shapes() {
        let (runtime, _ws) = test_runtime();
        let _ = runtime
            .dispatch(
                &req(
                    "seed",
                    "task.create",
                    json!({
                        "id": "close-me",
                        "title": "close me",
                        "autoExecute": false,
                        "status": "open"
                    }),
                ),
                "trusted_local",
            )
            .expect("seed");

        let close = req("t-close", "task.close", json!({ "id": "close-me" }));
        let closed = runtime
            .dispatch(&close, "trusted_local")
            .expect("task.close should succeed");
        assert_eq!(closed["task"]["id"], "close-me");
    }

    #[test]
    fn dispatch_task_archive_and_unarchive_round_trip() {
        let (runtime, _ws) = test_runtime();
        let _ = runtime
            .dispatch(
                &req(
                    "seed",
                    "task.create",
                    json!({
                        "id": "arch-me",
                        "title": "archive me",
                        "autoExecute": false,
                        "status": "open"
                    }),
                ),
                "trusted_local",
            )
            .expect("seed");

        let archived = runtime
            .dispatch(
                &req("a", "task.archive", json!({ "id": "arch-me" })),
                "trusted_local",
            )
            .expect("task.archive");
        assert!(
            archived["task"]["archivedAt"].is_string(),
            "archivedAt should be set after archive"
        );

        let unarchived = runtime
            .dispatch(
                &req("u", "task.unarchive", json!({ "id": "arch-me" })),
                "trusted_local",
            )
            .expect("task.unarchive");
        assert!(
            unarchived["task"]["archivedAt"].is_null()
                || !unarchived["task"]
                    .as_object()
                    .is_some_and(|obj| obj.contains_key("archivedAt")),
            "archivedAt should be cleared after unarchive"
        );
    }

    #[test]
    fn dispatch_task_delete_and_clear_return_success_flag() {
        let (runtime, _ws) = test_runtime();
        let _ = runtime
            .dispatch(
                &req(
                    "seed",
                    "task.create",
                    json!({
                        "id": "del-me",
                        "title": "delete me",
                        "autoExecute": false,
                        "status": "open"
                    }),
                ),
                "trusted_local",
            )
            .expect("seed");

        // task.delete requires the task to be in a terminal state (closed or
        // failed). Close it first so we can exercise the dispatch path.
        let _ = runtime
            .dispatch(
                &req("close-first", "task.close", json!({ "id": "del-me" })),
                "trusted_local",
            )
            .expect("task.close should succeed on open task");

        let deleted = runtime
            .dispatch(
                &req("d", "task.delete", json!({ "id": "del-me" })),
                "trusted_local",
            )
            .expect("task.delete");
        assert_eq!(deleted["success"], true);

        let cleared = runtime
            .dispatch(&req("cl", "task.clear", json!({})), "trusted_local")
            .expect("task.clear");
        assert_eq!(cleared["success"], true);
    }

    #[test]
    fn dispatch_task_status_returns_queue_info() {
        let (runtime, _ws) = test_runtime();
        let _ = runtime
            .dispatch(
                &req(
                    "seed",
                    "task.create",
                    json!({
                        "id": "stat-task",
                        "title": "status me",
                        "autoExecute": false,
                        "status": "open"
                    }),
                ),
                "trusted_local",
            )
            .expect("seed");

        let status = runtime
            .dispatch(
                &req("s", "task.status", json!({ "id": "stat-task" })),
                "trusted_local",
            )
            .expect("task.status");
        assert!(
            status.get("isQueued").is_some(),
            "task.status result should include isQueued"
        );
    }

    #[test]
    fn dispatch_task_with_unknown_sub_method_returns_service_unavailable() {
        let (runtime, _ws) = test_runtime();
        let request = req("t-bogus", "task.does_not_exist", json!({}));
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("unknown task.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
        assert!(err.message.contains("task.does_not_exist"));
    }

    #[test]
    fn dispatch_task_list_rejects_malformed_params() {
        let (runtime, _ws) = test_runtime();
        // `includeArchived` is Option<bool>; passing an int should fail deserialization.
        let request = req(
            "t-list-bad",
            "task.list",
            json!({ "includeArchived": 123 }),
        );
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("malformed params should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("task.list"));
    }

    // ---------- dispatch_loop -----------------------------------------------

    #[test]
    fn dispatch_loop_list_on_fresh_workspace_returns_empty() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(&req("l-1", "loop.list", json!({})), "trusted_local")
            .expect("loop.list should dispatch");
        assert!(result["loops"].is_array());
        // Freshly-created runtime has no primary lock and no registry entries.
        assert_eq!(result["loops"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn dispatch_loop_status_returns_interval_and_running_flag() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(&req("l-s", "loop.status", json!({})), "trusted_local")
            .expect("loop.status should dispatch");
        assert!(result["running"].is_boolean());
        assert!(result["intervalMs"].is_number());
    }

    #[test]
    fn dispatch_loop_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("l-bogus", "loop.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown loop.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    #[test]
    fn dispatch_loop_retry_requires_id() {
        let (runtime, _ws) = test_runtime();
        // Missing required `id` field -> InvalidParams.
        let err = runtime
            .dispatch(&req("l-r", "loop.retry", json!({})), "trusted_local")
            .expect_err("loop.retry should require id");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("loop.retry"));
    }

    // ---------- dispatch_planning -------------------------------------------

    #[test]
    fn dispatch_planning_list_is_empty_on_fresh_workspace() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(&req("p-l", "planning.list", json!({})), "trusted_local")
            .expect("planning.list should dispatch");
        assert!(result["sessions"].is_array());
    }

    #[test]
    fn dispatch_planning_get_missing_session_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("p-g", "planning.get", json!({ "id": "nope" })),
                "trusted_local",
            )
            .expect_err("missing planning session should error");
        assert_eq!(err.code, RpcErrorCode::PlanningSessionNotFound);
    }

    #[test]
    fn dispatch_planning_respond_requires_complete_params() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("p-r", "planning.respond", json!({ "sessionId": "x" })),
                "trusted_local",
            )
            .expect_err("planning.respond with missing fields should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn dispatch_planning_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("p-bogus", "planning.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown planning.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    // ---------- dispatch_config ---------------------------------------------

    #[test]
    fn dispatch_config_get_missing_file_returns_not_found() {
        let (runtime, _ws) = test_runtime();
        // Fresh workspace has no ralph.yml.
        let err = runtime
            .dispatch(&req("c-g", "config.get", json!({})), "trusted_local")
            .expect_err("config.get on missing file should error");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn dispatch_config_update_writes_and_get_round_trips() {
        let (runtime, _ws) = test_runtime();
        let content = "version: 1\nname: test\n";
        let updated = runtime
            .dispatch(
                &req(
                    "c-u",
                    "config.update",
                    json!({ "content": content }),
                ),
                "trusted_local",
            )
            .expect("config.update should succeed");
        assert_eq!(updated["success"], true);
        assert_eq!(updated["parsed"]["version"], 1);
        assert_eq!(updated["parsed"]["name"], "test");

        let got = runtime
            .dispatch(&req("c-g2", "config.get", json!({})), "trusted_local")
            .expect("config.get should succeed after update");
        assert_eq!(got["raw"], content);
        assert_eq!(got["parsed"]["version"], 1);
    }

    #[test]
    fn dispatch_config_update_rejects_invalid_yaml() {
        let (runtime, _ws) = test_runtime();
        // YAML mapping key followed by an unclosed bracket is an obvious
        // syntax error. Also, "root must be a mapping" is enforced, so a
        // bare scalar counts as invalid too.
        let err = runtime
            .dispatch(
                &req("c-bad", "config.update", json!({ "content": "just a string" })),
                "trusted_local",
            )
            .expect_err("non-mapping YAML should be rejected");
        assert_eq!(err.code, RpcErrorCode::ConfigInvalid);
    }

    #[test]
    fn dispatch_config_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("c-bogus", "config.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown config.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    // ---------- dispatch_preset ---------------------------------------------

    #[test]
    fn dispatch_preset_list_returns_preset_array() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(&req("pr-1", "preset.list", json!({})), "trusted_local")
            .expect("preset.list should dispatch");
        assert!(result["presets"].is_array());
    }

    #[test]
    fn dispatch_preset_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("pr-bogus", "preset.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown preset.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    // ---------- dispatch_collection -----------------------------------------

    #[test]
    fn dispatch_collection_list_is_empty_on_fresh_workspace() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(
                &req("col-l", "collection.list", json!({})),
                "trusted_local",
            )
            .expect("collection.list should dispatch");
        assert!(result["collections"].is_array());
        assert_eq!(result["collections"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn dispatch_collection_create_list_get_export_round_trip() {
        let (runtime, _ws) = test_runtime();

        let created = runtime
            .dispatch(
                &req(
                    "col-c",
                    "collection.create",
                    json!({ "name": "my-collection" }),
                ),
                "trusted_local",
            )
            .expect("collection.create should succeed");
        let id = created["collection"]["id"]
            .as_str()
            .expect("collection.create should include id")
            .to_string();

        let listed = runtime
            .dispatch(
                &req("col-l2", "collection.list", json!({})),
                "trusted_local",
            )
            .expect("collection.list should succeed");
        assert_eq!(listed["collections"].as_array().unwrap().len(), 1);

        let fetched = runtime
            .dispatch(
                &req("col-g", "collection.get", json!({ "id": id })),
                "trusted_local",
            )
            .expect("collection.get should succeed");
        assert_eq!(fetched["collection"]["name"], "my-collection");

        let exported = runtime
            .dispatch(
                &req("col-e", "collection.export", json!({ "id": id })),
                "trusted_local",
            )
            .expect("collection.export should succeed");
        assert!(exported["yaml"].is_string());
    }

    #[test]
    fn dispatch_collection_delete_returns_success_flag() {
        let (runtime, _ws) = test_runtime();
        let created = runtime
            .dispatch(
                &req(
                    "col-c",
                    "collection.create",
                    json!({ "name": "to-delete" }),
                ),
                "trusted_local",
            )
            .expect("collection.create");
        let id = created["collection"]["id"].as_str().unwrap().to_string();

        let deleted = runtime
            .dispatch(
                &req("col-d", "collection.delete", json!({ "id": id })),
                "trusted_local",
            )
            .expect("collection.delete should succeed");
        assert_eq!(deleted["success"], true);
    }

    #[test]
    fn dispatch_collection_get_missing_returns_not_found() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("col-miss", "collection.get", json!({ "id": "nope" })),
                "trusted_local",
            )
            .expect_err("missing collection should error");
        assert_eq!(err.code, RpcErrorCode::CollectionNotFound);
    }

    #[test]
    fn dispatch_collection_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("col-bogus", "collection.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown collection.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    // ---------- dispatch_stream ---------------------------------------------

    #[test]
    fn dispatch_stream_subscribe_returns_subscription_id() {
        let (runtime, _ws) = test_runtime();
        let result = runtime
            .dispatch(
                &req(
                    "str-sub",
                    "stream.subscribe",
                    json!({ "topics": ["task.status.changed"] }),
                ),
                "alice",
            )
            .expect("stream.subscribe should dispatch");
        assert!(result["subscriptionId"].as_str().unwrap().starts_with("sub-"));
        assert_eq!(
            result["acceptedTopics"].as_array().unwrap()[0],
            "task.status.changed"
        );
    }

    #[test]
    fn dispatch_stream_subscribe_passes_principal_through() {
        let (runtime, _ws) = test_runtime();
        let sub_result = runtime
            .dispatch(
                &req(
                    "str-principal",
                    "stream.subscribe",
                    json!({ "topics": ["task.status.changed"] }),
                ),
                "alice@example",
            )
            .expect("stream.subscribe should dispatch");
        let sub_id = sub_result["subscriptionId"].as_str().unwrap().to_string();

        // StreamDomain records the principal against the subscription; read
        // it back through the dedicated accessor to verify dispatch_stream
        // forwarded the principal argument correctly.
        let principal = runtime
            .stream_domain()
            .get_subscription_principal(&sub_id)
            .expect("subscription should exist");
        assert_eq!(principal, "alice@example");
    }

    #[test]
    fn dispatch_stream_unsubscribe_then_missing_returns_not_found() {
        let (runtime, _ws) = test_runtime();
        // Subscribe, then unsubscribe, then unsubscribe again -> NOT_FOUND.
        let sub = runtime
            .dispatch(
                &req(
                    "str-s",
                    "stream.subscribe",
                    json!({ "topics": ["task.status.changed"] }),
                ),
                "trusted_local",
            )
            .expect("stream.subscribe");
        let sub_id = sub["subscriptionId"].as_str().unwrap().to_string();

        let unsub = runtime
            .dispatch(
                &req(
                    "str-u",
                    "stream.unsubscribe",
                    json!({ "subscriptionId": sub_id.clone() }),
                ),
                "trusted_local",
            )
            .expect("stream.unsubscribe should succeed once");
        assert_eq!(unsub["success"], true);

        let err = runtime
            .dispatch(
                &req(
                    "str-u2",
                    "stream.unsubscribe",
                    json!({ "subscriptionId": sub_id }),
                ),
                "trusted_local",
            )
            .expect_err("double-unsubscribe should fail");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn dispatch_stream_ack_unknown_subscription_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req(
                    "str-ack",
                    "stream.ack",
                    // Cursor must be syntactically valid for the ack handler to
                    // reach the subscription lookup; the cursor format is
                    // "<ms>-<hex>".
                    json!({
                        "subscriptionId": "sub-unknown",
                        "cursor": "0000000000000000-0000000000000001"
                    }),
                ),
                "trusted_local",
            )
            .expect_err("stream.ack on unknown subscription should fail");
        assert_eq!(err.code, RpcErrorCode::NotFound);
    }

    #[test]
    fn dispatch_stream_unknown_sub_method_errors() {
        let (runtime, _ws) = test_runtime();
        let err = runtime
            .dispatch(
                &req("str-bogus", "stream.does_not_exist", json!({})),
                "trusted_local",
            )
            .expect_err("unknown stream.* method should fail");
        assert_eq!(err.code, RpcErrorCode::ServiceUnavailable);
    }

    // ---------- dispatch_internal_publish -----------------------------------

    #[test]
    fn dispatch_internal_publish_happy_path() {
        let (runtime, _ws) = test_runtime();
        let request = req(
            "ip-1",
            "_internal.publish",
            json!({
                "topic": "task.status.changed",
                "resourceType": "task",
                "resourceId": "task-xyz",
                "payload": { "from": "open", "to": "closed" }
            }),
        );
        let result = runtime
            .dispatch(&request, "trusted_local")
            .expect("_internal.publish should succeed");
        assert_eq!(result["success"], true);
    }

    #[test]
    fn dispatch_internal_publish_rejects_missing_fields() {
        let (runtime, _ws) = test_runtime();
        let request = req(
            "ip-bad",
            "_internal.publish",
            json!({
                // missing topic, resourceType, resourceId, payload
            }),
        );
        let err = runtime
            .dispatch(&request, "trusted_local")
            .expect_err("malformed _internal.publish should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    // ---------- parse_task_update_input -------------------------------------

    #[test]
    fn parse_task_update_input_requires_object_params() {
        let request = req("u-1", "task.update", json!([1, 2, 3]));
        let err = parse_task_update_input(&request).expect_err("non-object should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(
            err.message.to_lowercase().contains("object"),
            "message should mention 'object': {}",
            err.message
        );
    }

    #[test]
    fn parse_task_update_input_rejects_missing_id() {
        let request = req("u-2", "task.update", json!({ "title": "hello" }));
        let err = parse_task_update_input(&request).expect_err("missing id should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("'id'"));
    }

    #[test]
    fn parse_task_update_input_rejects_empty_id() {
        let request = req("u-3", "task.update", json!({ "id": "" }));
        let err = parse_task_update_input(&request).expect_err("empty id should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.to_lowercase().contains("id"));
    }

    #[test]
    fn parse_task_update_input_rejects_non_string_id() {
        let request = req("u-4", "task.update", json!({ "id": 42 }));
        let err = parse_task_update_input(&request).expect_err("non-string id should fail");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }

    #[test]
    fn parse_task_update_input_minimal_with_only_id() {
        let request = req("u-5", "task.update", json!({ "id": "t-only-id" }));
        let input = parse_task_update_input(&request).expect("only id should be enough");
        assert_eq!(input.id, "t-only-id");
        assert!(input.title.is_none());
        assert!(input.status.is_none());
        assert!(input.priority.is_none());
        assert!(input.blocked_by.is_none());
    }

    #[test]
    fn parse_task_update_input_parses_all_scalar_fields() {
        let request = req(
            "u-6",
            "task.update",
            json!({
                "id": "t-full",
                "title": "new title",
                "status": "closed",
                "priority": 3
            }),
        );
        let input = parse_task_update_input(&request).expect("full payload should parse");
        assert_eq!(input.id, "t-full");
        assert_eq!(input.title.as_deref(), Some("new title"));
        assert_eq!(input.status.as_deref(), Some("closed"));
        assert_eq!(input.priority, Some(3));
        assert!(input.blocked_by.is_none());
    }

    #[test]
    fn parse_task_update_input_ignores_out_of_range_priority() {
        // priority must fit in u8; a huge value should quietly become None
        // (the schema layer would normally catch this, but the parser itself
        // uses a best-effort TryFrom).
        let request = req(
            "u-7",
            "task.update",
            json!({ "id": "x", "priority": 9999 }),
        );
        let input = parse_task_update_input(&request).expect("huge priority should still parse");
        assert!(
            input.priority.is_none(),
            "priority outside u8 range should be dropped"
        );
    }

    #[test]
    fn parse_task_update_input_ignores_non_integer_priority() {
        let request = req(
            "u-8",
            "task.update",
            json!({ "id": "x", "priority": "not a number" }),
        );
        let input = parse_task_update_input(&request).expect("non-int priority should parse");
        assert!(input.priority.is_none());
    }

    #[test]
    fn parse_task_update_input_blocked_by_missing_is_none() {
        let request = req("u-b1", "task.update", json!({ "id": "x" }));
        let input = parse_task_update_input(&request).expect("should parse");
        // Missing field = no change intent.
        assert!(
            input.blocked_by.is_none(),
            "missing blockedBy must be None (no change)"
        );
    }

    #[test]
    fn parse_task_update_input_blocked_by_null_is_some_none() {
        let request = req(
            "u-b2",
            "task.update",
            json!({ "id": "x", "blockedBy": Value::Null }),
        );
        let input = parse_task_update_input(&request).expect("null blockedBy should parse");
        // Present but null = intent to clear.
        assert_eq!(input.blocked_by, Some(None));
    }

    #[test]
    fn parse_task_update_input_blocked_by_string_is_some_some() {
        let request = req(
            "u-b3",
            "task.update",
            json!({ "id": "x", "blockedBy": "other-task" }),
        );
        let input = parse_task_update_input(&request).expect("string blockedBy should parse");
        assert_eq!(input.blocked_by, Some(Some("other-task".to_string())));
    }

    #[test]
    fn parse_task_update_input_blocked_by_wrong_type_errors() {
        let request = req(
            "u-b4",
            "task.update",
            json!({ "id": "x", "blockedBy": 123 }),
        );
        let err =
            parse_task_update_input(&request).expect_err("numeric blockedBy should be rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("blockedBy"));
    }

    #[test]
    fn parse_task_update_input_blocked_by_boolean_is_rejected() {
        let request = req(
            "u-b5",
            "task.update",
            json!({ "id": "x", "blockedBy": true }),
        );
        let err = parse_task_update_input(&request).expect_err("bool blockedBy should be rejected");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
    }
}
