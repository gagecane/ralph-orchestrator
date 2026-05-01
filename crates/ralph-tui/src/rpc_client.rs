//! RPC v1 client for connecting the TUI to a remote ralph-api server.
//!
//! Provides HTTP request/response and WebSocket streaming for consuming
//! the same RPC v1 API that the web dashboard uses. This enables the TUI
//! to attach to a running orchestration loop from any terminal.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Request / response types (mirrors ralph-api protocol)
// ---------------------------------------------------------------------------

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("tui-{}-{:04x}", chrono::Utc::now().timestamp_millis(), n)
}

fn next_idempotency_key(method: &str) -> String {
    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "idem-tui-{}-{}-{:04x}",
        method.replace('.', "-"),
        chrono::Utc::now().timestamp_millis(),
        n
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RpcRequest {
    api_version: String,
    id: String,
    method: String,
    params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<RequestMeta>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestMeta {
    idempotency_key: String,
    request_ts: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcResponse {
    #[allow(dead_code)]
    api_version: String,
    #[allow(dead_code)]
    id: String,
    result: Option<Value>,
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

// ---------------------------------------------------------------------------
// Stream event types
// ---------------------------------------------------------------------------

/// A stream event received over WebSocket from ralph-api.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    #[allow(dead_code)]
    pub api_version: String,
    #[allow(dead_code)]
    pub stream: String,
    pub topic: String,
    pub cursor: String,
    pub sequence: u64,
    #[allow(dead_code)]
    pub ts: String,
    pub resource: StreamResource,
    pub replay: StreamReplay,
    pub payload: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamResource {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamReplay {
    pub mode: String,
    #[allow(dead_code)]
    pub requested_cursor: Option<String>,
    #[allow(dead_code)]
    pub batch: Option<u64>,
}

// ---------------------------------------------------------------------------
// Subscribe result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    pub subscription_id: String,
    pub accepted_topics: Vec<String>,
    pub cursor: String,
}

// ---------------------------------------------------------------------------
// Domain types returned by RPC methods
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopRecord {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResult {
    #[serde(default)]
    pub config: Value,
}

// ---------------------------------------------------------------------------
// RPC client
// ---------------------------------------------------------------------------

/// An RPC v1 client targeting a single ralph-api server.
#[derive(Clone)]
pub struct RpcClient {
    http: reqwest::Client,
    /// Base URL, e.g. `http://127.0.0.1:3000`
    base_url: url::Url,
}

impl RpcClient {
    /// Create a new client pointed at the given base URL.
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = url::Url::parse(base_url)
            .with_context(|| format!("invalid ralph-api base URL: {base_url}"))?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { http, base_url })
    }

    /// Issue an RPC call and return the `result` value.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let is_mutating = is_mutating(method);
        let request = RpcRequest {
            api_version: "v1".to_string(),
            id: next_request_id(),
            method: method.to_string(),
            params,
            meta: if is_mutating {
                Some(RequestMeta {
                    idempotency_key: next_idempotency_key(method),
                    request_ts: chrono::Utc::now().to_rfc3339(),
                })
            } else {
                None
            },
        };

        let url = self
            .base_url
            .join("/rpc/v1")
            .context("failed to build RPC endpoint URL")?;

        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .context("RPC HTTP request failed")?;

        let status = response.status();
        let body: RpcResponse = response
            .json()
            .await
            .context("failed to parse RPC response JSON")?;

        if let Some(err) = body.error {
            anyhow::bail!(
                "RPC error ({status}): [{code}] {msg}",
                code = err.code,
                msg = err.message
            );
        }

        body.result
            .ok_or_else(|| anyhow::anyhow!("RPC response missing result"))
    }

    // -- convenience wrappers ------------------------------------------------

    /// Fetch all tasks.
    pub async fn task_list(&self) -> Result<Vec<TaskRecord>> {
        let result = self.call("task.list", json!({})).await?;
        let tasks: Vec<TaskRecord> =
            serde_json::from_value(result.get("tasks").cloned().unwrap_or(Value::Array(vec![])))
                .context("failed to parse task list")?;
        Ok(tasks)
    }

    /// Fetch all loops.
    pub async fn loop_list(&self) -> Result<Vec<LoopRecord>> {
        let result = self
            .call("loop.list", json!({ "includeTerminal": true }))
            .await?;
        let loops: Vec<LoopRecord> =
            serde_json::from_value(result.get("loops").cloned().unwrap_or(Value::Array(vec![])))
                .context("failed to parse loop list")?;
        Ok(loops)
    }

    /// Fetch config.
    pub async fn config_get(&self) -> Result<Value> {
        self.call("config.get", json!({})).await
    }

    /// Create a stream subscription, returning the subscription ID and cursor.
    pub async fn stream_subscribe(
        &self,
        topics: &[&str],
        cursor: Option<&str>,
    ) -> Result<SubscribeResult> {
        let mut params = json!({
            "topics": topics,
        });
        if let Some(c) = cursor {
            params["cursor"] = Value::String(c.to_string());
        }
        let result = self.call("stream.subscribe", params).await?;
        serde_json::from_value(result).context("failed to parse subscribe result")
    }

    /// Build the WebSocket URL for the given subscription ID.
    pub fn stream_ws_url(&self, subscription_id: &str) -> Result<String> {
        let mut ws_url = self.base_url.clone();
        let scheme = match ws_url.scheme() {
            "https" => "wss",
            _ => "ws",
        };
        ws_url
            .set_scheme(scheme)
            .map_err(|()| anyhow::anyhow!("failed to set WebSocket scheme"))?;
        ws_url.set_path("/rpc/v1/stream");
        ws_url
            .query_pairs_mut()
            .append_pair("subscriptionId", subscription_id);
        Ok(ws_url.to_string())
    }

    /// Send a `stream.ack` to checkpoint the cursor.
    pub async fn stream_ack(&self, subscription_id: &str, cursor: &str) -> Result<()> {
        self.call(
            "stream.ack",
            json!({
                "subscriptionId": subscription_id,
                "cursor": cursor,
            }),
        )
        .await?;
        Ok(())
    }
}

fn is_mutating(method: &str) -> bool {
    matches!(
        method,
        "task.create"
            | "task.update"
            | "task.close"
            | "task.archive"
            | "task.unarchive"
            | "task.delete"
            | "task.clear"
            | "task.run"
            | "task.run_all"
            | "task.retry"
            | "task.cancel"
            | "loop.process"
            | "loop.prune"
            | "loop.retry"
            | "loop.discard"
            | "loop.stop"
            | "loop.merge"
            | "loop.trigger_merge_task"
            | "planning.start"
            | "planning.respond"
            | "planning.resume"
            | "planning.delete"
            | "config.update"
            | "collection.create"
            | "collection.update"
            | "collection.delete"
            | "collection.import"
    )
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as AsyncMutex;

    // =========================================================================
    // Pure-logic helpers: next_request_id / next_idempotency_key
    // =========================================================================

    #[test]
    fn next_request_id_has_expected_format_and_is_monotonic() {
        let a = next_request_id();
        let b = next_request_id();

        // Format: tui-<timestamp-ms>-<4-hex-counter>
        for id in [&a, &b] {
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 3, "expected 3 parts, got {id:?}");
            assert_eq!(parts[0], "tui");
            assert!(
                parts[1].chars().all(|c| c.is_ascii_digit()),
                "timestamp part should be numeric in {id:?}"
            );
            assert_eq!(
                parts[2].len(),
                4,
                "counter part should be 4 hex chars in {id:?}"
            );
            assert!(
                parts[2].chars().all(|c| c.is_ascii_hexdigit()),
                "counter part should be hex in {id:?}"
            );
        }

        // The counter is shared atomically so IDs must differ.
        assert_ne!(a, b, "consecutive IDs should be unique");
    }

    #[test]
    fn next_idempotency_key_replaces_dots_in_method_name() {
        let key = next_idempotency_key("task.create");
        assert!(
            key.starts_with("idem-tui-task-create-"),
            "dots in method should become dashes, got {key:?}"
        );
        // No literal dots remain (the timestamp and counter are digits/hex only).
        assert!(!key.contains('.'), "key should not contain dots: {key:?}");
    }

    #[test]
    fn next_idempotency_key_includes_numeric_timestamp_and_hex_counter() {
        let key = next_idempotency_key("loop.stop");
        // Layout: idem-tui-<method-with-dashes>-<ts>-<hex>
        let parts: Vec<&str> = key.split('-').collect();
        // ["idem", "tui", "loop", "stop", "<ts>", "<hex>"]
        assert_eq!(parts.len(), 6, "unexpected idempotency key shape: {key:?}");
        assert_eq!(parts[0], "idem");
        assert_eq!(parts[1], "tui");
        assert!(parts[4].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts[5].len(), 4);
        assert!(parts[5].chars().all(|c| c.is_ascii_hexdigit()));
    }

    // =========================================================================
    // Pure-logic: is_mutating()
    // =========================================================================

    #[test]
    fn is_mutating_classifies_all_known_write_methods() {
        let write_methods = [
            "task.create",
            "task.update",
            "task.close",
            "task.archive",
            "task.unarchive",
            "task.delete",
            "task.clear",
            "task.run",
            "task.run_all",
            "task.retry",
            "task.cancel",
            "loop.process",
            "loop.prune",
            "loop.retry",
            "loop.discard",
            "loop.stop",
            "loop.merge",
            "loop.trigger_merge_task",
            "planning.start",
            "planning.respond",
            "planning.resume",
            "planning.delete",
            "config.update",
            "collection.create",
            "collection.update",
            "collection.delete",
            "collection.import",
        ];
        for m in write_methods {
            assert!(is_mutating(m), "{m} should be classified as mutating");
        }
    }

    #[test]
    fn is_mutating_rejects_read_and_unknown_methods() {
        let read_methods = [
            "task.list",
            "loop.list",
            "config.get",
            "stream.subscribe",
            "stream.ack",
            "system.health",
            "",
            "task.unknown_future_method",
            "collection.list",
            "planning.list",
        ];
        for m in read_methods {
            assert!(!is_mutating(m), "{m} should NOT be classified as mutating");
        }
    }

    // =========================================================================
    // Pure-logic: RpcClient::new URL validation
    // =========================================================================

    #[test]
    fn client_new_accepts_valid_http_and_https_urls() {
        assert!(RpcClient::new("http://127.0.0.1:3000").is_ok());
        assert!(RpcClient::new("https://api.example.com").is_ok());
        assert!(RpcClient::new("http://localhost:8080/").is_ok());
    }

    #[test]
    fn client_new_rejects_invalid_url() {
        let result = RpcClient::new("not a url at all");
        let err = match result {
            Ok(_) => panic!("expected error for malformed URL"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("invalid ralph-api base URL"),
            "error should mention invalid URL, got: {msg}"
        );
    }

    // =========================================================================
    // Pure-logic: stream_ws_url
    // =========================================================================

    #[test]
    fn stream_ws_url_converts_http_to_ws_scheme() {
        let client = RpcClient::new("http://127.0.0.1:3000").unwrap();
        let url = client.stream_ws_url("sub-abc").unwrap();
        assert!(url.starts_with("ws://"), "expected ws:// scheme, got {url}");
        assert!(url.contains("127.0.0.1:3000"), "missing host/port in {url}");
        assert!(url.contains("/rpc/v1/stream"), "missing path in {url}");
        assert!(
            url.contains("subscriptionId=sub-abc"),
            "missing subscriptionId in {url}"
        );
    }

    #[test]
    fn stream_ws_url_converts_https_to_wss_scheme() {
        let client = RpcClient::new("https://api.example.com").unwrap();
        let url = client.stream_ws_url("sub-xyz").unwrap();
        assert!(
            url.starts_with("wss://"),
            "expected wss:// scheme, got {url}"
        );
        assert!(url.contains("api.example.com"), "missing host in {url}");
        assert!(url.contains("subscriptionId=sub-xyz"), "missing id in {url}");
    }

    #[test]
    fn stream_ws_url_url_encodes_subscription_id() {
        let client = RpcClient::new("http://127.0.0.1:3000").unwrap();
        // Spaces and special chars should be percent-encoded by the url crate.
        let url = client.stream_ws_url("sub with space").unwrap();
        assert!(
            url.contains("subscriptionId=sub+with+space")
                || url.contains("subscriptionId=sub%20with%20space"),
            "subscription id should be URL-encoded, got {url}"
        );
    }

    #[test]
    fn stream_ws_url_overrides_any_existing_path() {
        // Base URL with a stale path should still produce /rpc/v1/stream.
        let client = RpcClient::new("http://127.0.0.1:3000/some/other/path").unwrap();
        let url = client.stream_ws_url("sub-1").unwrap();
        assert!(url.contains("/rpc/v1/stream"), "path not overridden: {url}");
        assert!(!url.contains("/some/other"), "stale path leaked: {url}");
    }

    // =========================================================================
    // Serialization: outgoing RpcRequest shape
    // =========================================================================

    #[test]
    fn rpc_request_serializes_with_camel_case_fields() {
        let req = RpcRequest {
            api_version: "v1".to_string(),
            id: "req-1".to_string(),
            method: "task.list".to_string(),
            params: json!({}),
            meta: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["apiVersion"], "v1");
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["method"], "task.list");
        assert!(v.get("params").is_some());
        // meta should be omitted when None (serde skip_serializing_if).
        assert!(
            v.get("meta").is_none(),
            "meta must be omitted when None, got {v:?}"
        );
    }

    #[test]
    fn rpc_request_meta_serializes_camel_case_when_present() {
        let req = RpcRequest {
            api_version: "v1".to_string(),
            id: "req-2".to_string(),
            method: "task.create".to_string(),
            params: json!({}),
            meta: Some(RequestMeta {
                idempotency_key: "idem-1".to_string(),
                request_ts: "2026-05-01T00:00:00Z".to_string(),
            }),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["meta"]["idempotencyKey"], "idem-1");
        assert_eq!(v["meta"]["requestTs"], "2026-05-01T00:00:00Z");
    }

    // =========================================================================
    // Deserialization: RpcResponse / RpcErrorBody
    // =========================================================================

    #[test]
    fn rpc_response_deserializes_success_result() {
        let raw = json!({
            "apiVersion": "v1",
            "id": "req-1",
            "result": { "ok": true }
        });
        let resp: RpcResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn rpc_response_deserializes_error_body() {
        let raw = json!({
            "apiVersion": "v1",
            "id": "req-1",
            "error": {
                "code": "NOT_FOUND",
                "message": "task does not exist",
                "retryable": false
            }
        });
        let resp: RpcResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "task does not exist");
        assert!(!err.retryable);
    }

    #[test]
    fn rpc_response_deserializes_retryable_error_body() {
        let raw = json!({
            "apiVersion": "v1",
            "id": "req-2",
            "error": {
                "code": "BUSY",
                "message": "try again",
                "retryable": true
            }
        });
        let resp: RpcResponse = serde_json::from_value(raw).unwrap();
        let err = resp.error.unwrap();
        assert!(err.retryable);
    }

    // =========================================================================
    // Deserialization: domain records
    // =========================================================================

    #[test]
    fn task_record_deserializes_from_camel_case() {
        let raw = json!({ "id": "t-1", "title": "Do thing", "status": "open" });
        let t: TaskRecord = serde_json::from_value(raw).unwrap();
        assert_eq!(t.id, "t-1");
        assert_eq!(t.title, "Do thing");
        assert_eq!(t.status, "open");
    }

    #[test]
    fn loop_record_defaults_prompt_to_none_when_missing() {
        let raw = json!({ "id": "L-1", "status": "running" });
        let l: LoopRecord = serde_json::from_value(raw).unwrap();
        assert_eq!(l.id, "L-1");
        assert_eq!(l.status, "running");
        assert!(l.prompt.is_none());
    }

    #[test]
    fn loop_record_preserves_prompt_when_present() {
        let raw = json!({ "id": "L-2", "status": "queued", "prompt": "hello" });
        let l: LoopRecord = serde_json::from_value(raw).unwrap();
        assert_eq!(l.prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn config_result_defaults_config_to_null_when_missing() {
        let raw = json!({});
        let c: ConfigResult = serde_json::from_value(raw).unwrap();
        // serde_json::Value::default() is Value::Null.
        assert!(c.config.is_null());
    }

    #[test]
    fn subscribe_result_deserializes_camel_case() {
        let raw = json!({
            "subscriptionId": "sub-1",
            "acceptedTopics": ["task.*"],
            "cursor": "42-0"
        });
        let r: SubscribeResult = serde_json::from_value(raw).unwrap();
        assert_eq!(r.subscription_id, "sub-1");
        assert_eq!(r.accepted_topics, vec!["task.*".to_string()]);
        assert_eq!(r.cursor, "42-0");
    }

    #[test]
    fn stream_event_deserializes_nested_camel_case_fields() {
        let raw = json!({
            "apiVersion": "v1",
            "stream": "events.v1",
            "topic": "task.status",
            "cursor": "100-0",
            "sequence": 7,
            "ts": "2026-05-01T00:00:00Z",
            "resource": { "type": "task", "id": "t-1" },
            "replay": { "mode": "live", "requestedCursor": null, "batch": null },
            "payload": { "status": "running" }
        });
        let e: StreamEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(e.topic, "task.status");
        assert_eq!(e.cursor, "100-0");
        assert_eq!(e.sequence, 7);
        assert_eq!(e.resource.kind, "task");
        assert_eq!(e.resource.id, "t-1");
        assert_eq!(e.replay.mode, "live");
        assert_eq!(e.payload["status"], "running");
    }

    #[test]
    fn stream_event_replay_accepts_populated_fields() {
        let raw = json!({
            "apiVersion": "v1",
            "stream": "events.v1",
            "topic": "task.log",
            "cursor": "1-0",
            "sequence": 0,
            "ts": "2026-05-01T00:00:00Z",
            "resource": { "type": "task", "id": "t-1" },
            "replay": {
                "mode": "replay",
                "requestedCursor": "50-0",
                "batch": 10
            },
            "payload": {}
        });
        let e: StreamEvent = serde_json::from_value(raw).unwrap();
        assert_eq!(e.replay.mode, "replay");
    }

    // =========================================================================
    // HTTP integration tests via a minimal in-process mock server
    // =========================================================================

    /// Captured HTTP request from the mock server.
    #[derive(Debug, Clone)]
    struct CapturedRequest {
        method: String,
        path: String,
        body: Value,
    }

    /// A tiny single-shot HTTP/1.1 server that answers one request with a
    /// fixed JSON body and captures the incoming request for assertions.
    struct MockServer {
        base_url: String,
        captured: Arc<AsyncMutex<Vec<CapturedRequest>>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        /// Spin up a server that answers `expected_count` requests, each
        /// replying with the supplied JSON body (status 200).
        async fn start(expected_count: usize, response_body: Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("mock listener bind");
            let addr = listener.local_addr().expect("mock local addr");
            let captured: Arc<AsyncMutex<Vec<CapturedRequest>>> =
                Arc::new(AsyncMutex::new(Vec::new()));
            let captured_clone = captured.clone();
            let body_bytes = serde_json::to_vec(&response_body).expect("response body to bytes");

            let handle = tokio::spawn(async move {
                for _ in 0..expected_count {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(pair) => pair,
                        Err(_) => return,
                    };

                    // Read until we've consumed headers + content-length body.
                    let mut buf = Vec::with_capacity(2048);
                    let mut tmp = [0u8; 1024];
                    let mut headers_end: Option<usize> = None;
                    let mut content_length: usize = 0;

                    // Read headers.
                    loop {
                        let n = match stream.read(&mut tmp).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = find_double_crlf(&buf) {
                            headers_end = Some(pos);
                            content_length = parse_content_length(&buf[..pos]).unwrap_or(0);
                            break;
                        }
                    }

                    let Some(end) = headers_end else {
                        continue;
                    };

                    // Read the rest of the body if not already buffered.
                    let body_start = end + 4;
                    while buf.len() < body_start + content_length {
                        let n = match stream.read(&mut tmp).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                    }

                    let headers_str = String::from_utf8_lossy(&buf[..end]);
                    let mut lines = headers_str.split("\r\n");
                    let request_line = lines.next().unwrap_or("");
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();

                    let body_bytes_in = &buf[body_start..body_start + content_length.min(buf.len().saturating_sub(body_start))];
                    let body_json: Value =
                        serde_json::from_slice(body_bytes_in).unwrap_or(Value::Null);

                    captured_clone.lock().await.push(CapturedRequest {
                        method,
                        path,
                        body: body_json,
                    });

                    // Write HTTP/1.1 response.
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\
                         \r\n",
                        body_bytes.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&body_bytes).await;
                    let _ = stream.flush().await;
                    let _ = stream.shutdown().await;
                }
            });

            MockServer {
                base_url: format!("http://{addr}"),
                captured,
                handle,
            }
        }

        async fn captured(&self) -> Vec<CapturedRequest> {
            self.captured.lock().await.clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn parse_content_length(headers: &[u8]) -> Option<usize> {
        let s = std::str::from_utf8(headers).ok()?;
        for line in s.split("\r\n") {
            let mut parts = line.splitn(2, ':');
            let name = parts.next()?.trim();
            let value = parts.next().unwrap_or("").trim();
            if name.eq_ignore_ascii_case("content-length") {
                return value.parse().ok();
            }
        }
        None
    }

    /// Build a response envelope wrapping an arbitrary `result` value.
    fn ok_envelope(result: Value) -> Value {
        json!({
            "apiVersion": "v1",
            "id": "resp-1",
            "result": result
        })
    }

    /// Build an error-response envelope.
    fn err_envelope(code: &str, message: &str, retryable: bool) -> Value {
        json!({
            "apiVersion": "v1",
            "id": "resp-1",
            "error": {
                "code": code,
                "message": message,
                "retryable": retryable
            }
        })
    }

    #[tokio::test]
    async fn call_sends_post_to_rpc_v1_path_with_camel_case_envelope() {
        let server = MockServer::start(1, ok_envelope(json!({ "ok": true }))).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let result = client
            .call("task.list", json!({ "filter": "all" }))
            .await
            .unwrap();
        assert_eq!(result["ok"], true);

        let captured = server.captured().await;
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/rpc/v1");
        assert_eq!(req.body["apiVersion"], "v1");
        assert_eq!(req.body["method"], "task.list");
        assert_eq!(req.body["params"]["filter"], "all");
        assert!(
            req.body["id"].as_str().unwrap().starts_with("tui-"),
            "id should carry tui- prefix"
        );
    }

    #[tokio::test]
    async fn call_omits_meta_for_read_methods() {
        let server = MockServer::start(1, ok_envelope(json!({}))).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        client.call("task.list", json!({})).await.unwrap();

        let captured = server.captured().await;
        let body = &captured[0].body;
        assert!(
            body.get("meta").is_none() || body["meta"].is_null(),
            "read methods must not send meta, got {body}"
        );
    }

    #[tokio::test]
    async fn call_includes_meta_with_idempotency_key_for_write_methods() {
        let server = MockServer::start(1, ok_envelope(json!({}))).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        client.call("task.create", json!({ "title": "x" })).await.unwrap();

        let captured = server.captured().await;
        let body = &captured[0].body;
        let meta = body.get("meta").expect("meta required for mutating method");
        let idem = meta["idempotencyKey"].as_str().expect("idempotencyKey string");
        assert!(
            idem.starts_with("idem-tui-task-create-"),
            "idempotency key format wrong: {idem}"
        );
        assert!(
            meta["requestTs"].as_str().is_some(),
            "requestTs must be a string"
        );
    }

    #[tokio::test]
    async fn call_bubbles_up_rpc_error_body() {
        let server = MockServer::start(1, err_envelope("NOT_FOUND", "no such task", false)).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let err = client.call("task.list", json!({})).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("NOT_FOUND"), "error missing code: {msg}");
        assert!(msg.contains("no such task"), "error missing message: {msg}");
    }

    #[tokio::test]
    async fn call_errors_when_response_has_neither_result_nor_error() {
        // Envelope with no `result` and no `error` fields.
        let server = MockServer::start(
            1,
            json!({ "apiVersion": "v1", "id": "resp-1" }),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let err = client.call("task.list", json!({})).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing result"),
            "expected missing-result error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn task_list_parses_records_from_tasks_field() {
        let server = MockServer::start(
            1,
            ok_envelope(json!({
                "tasks": [
                    { "id": "t-1", "title": "Alpha", "status": "open" },
                    { "id": "t-2", "title": "Beta", "status": "closed" }
                ]
            })),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let tasks = client.task_list().await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "t-1");
        assert_eq!(tasks[1].status, "closed");
    }

    #[tokio::test]
    async fn task_list_returns_empty_vec_when_tasks_field_missing() {
        let server = MockServer::start(1, ok_envelope(json!({}))).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let tasks = client.task_list().await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn loop_list_sends_include_terminal_true_and_parses_loops() {
        let server = MockServer::start(
            1,
            ok_envelope(json!({
                "loops": [
                    { "id": "L-1", "status": "running", "prompt": "hi" },
                    { "id": "L-2", "status": "done" }
                ]
            })),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let loops = client.loop_list().await.unwrap();
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].prompt.as_deref(), Some("hi"));
        assert!(loops[1].prompt.is_none());

        let captured = server.captured().await;
        let params = &captured[0].body["params"];
        assert_eq!(params["includeTerminal"], true);
    }

    #[tokio::test]
    async fn config_get_returns_raw_result_value() {
        let server = MockServer::start(
            1,
            ok_envelope(json!({ "config": { "model": "claude" } })),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let v = client.config_get().await.unwrap();
        assert_eq!(v["config"]["model"], "claude");
    }

    #[tokio::test]
    async fn stream_subscribe_sends_topics_and_parses_response() {
        let server = MockServer::start(
            1,
            ok_envelope(json!({
                "subscriptionId": "sub-1",
                "acceptedTopics": ["task.*", "loop.*"],
                "cursor": "50-0"
            })),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        let result = client
            .stream_subscribe(&["task.*", "loop.*"], None)
            .await
            .unwrap();
        assert_eq!(result.subscription_id, "sub-1");
        assert_eq!(result.cursor, "50-0");
        assert_eq!(
            result.accepted_topics,
            vec!["task.*".to_string(), "loop.*".to_string()]
        );

        let captured = server.captured().await;
        let params = &captured[0].body["params"];
        assert_eq!(params["topics"][0], "task.*");
        assert_eq!(params["topics"][1], "loop.*");
        assert!(
            params.get("cursor").is_none() || params["cursor"].is_null(),
            "cursor should not be set when None"
        );
    }

    #[tokio::test]
    async fn stream_subscribe_includes_cursor_when_provided() {
        let server = MockServer::start(
            1,
            ok_envelope(json!({
                "subscriptionId": "sub-2",
                "acceptedTopics": [],
                "cursor": "100-0"
            })),
        )
        .await;
        let client = RpcClient::new(&server.base_url).unwrap();

        client
            .stream_subscribe(&["task.*"], Some("42-0"))
            .await
            .unwrap();

        let captured = server.captured().await;
        let params = &captured[0].body["params"];
        assert_eq!(params["cursor"], "42-0");
    }

    #[tokio::test]
    async fn stream_ack_sends_subscription_id_and_cursor() {
        let server = MockServer::start(1, ok_envelope(json!({}))).await;
        let client = RpcClient::new(&server.base_url).unwrap();

        client.stream_ack("sub-1", "99-0").await.unwrap();

        let captured = server.captured().await;
        let body = &captured[0].body;
        assert_eq!(body["method"], "stream.ack");
        assert_eq!(body["params"]["subscriptionId"], "sub-1");
        assert_eq!(body["params"]["cursor"], "99-0");
        // stream.ack is a read method so no idempotency meta.
        assert!(body.get("meta").is_none() || body["meta"].is_null());
    }

    #[tokio::test]
    async fn call_errors_with_context_when_server_returns_invalid_json() {
        // Start a listener that replies with non-JSON body.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut tmp = [0u8; 2048];
                // Drain request.
                let _ = stream.read(&mut tmp).await;
                let body = b"not json at all";
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(body).await;
                let _ = stream.shutdown().await;
            }
        });

        let client = RpcClient::new(&format!("http://{addr}")).unwrap();
        let err = client.call("task.list", json!({})).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to parse RPC response JSON"),
            "expected JSON parse context, got: {msg}"
        );
        handle.abort();
    }
}
