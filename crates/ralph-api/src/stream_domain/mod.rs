mod filters;
mod rpc_side_effects;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::errors::ApiError;
use crate::loop_support::now_ts;
use crate::protocol::{API_VERSION, STREAM_NAME, STREAM_TOPICS};

use self::filters::{
    SubscriptionFilters, cursor_is_older, cursor_sequence, normalize_topics, validate_cursor,
};

pub const KEEPALIVE_INTERVAL_MS: u64 = 15_000;

const DEFAULT_REPLAY_LIMIT: usize = 200;
const HISTORY_LIMIT: usize = 2_048;
const LIVE_BUFFER_CAPACITY: usize = 256;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSubscribeParams {
    pub topics: Vec<String>,
    pub cursor: Option<String>,
    pub replay_limit: Option<u16>,
    pub filters: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamUnsubscribeParams {
    pub subscription_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamAckParams {
    pub subscription_id: String,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSubscribeResult {
    pub subscription_id: String,
    pub accepted_topics: Vec<String>,
    pub cursor: String,
}

#[derive(Debug, Clone)]
pub struct ReplayBatch {
    pub events: Vec<StreamEventEnvelope>,
    pub dropped_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEventEnvelope {
    pub api_version: String,
    pub stream: String,
    pub topic: String,
    pub cursor: String,
    pub sequence: u64,
    pub ts: String,
    pub resource: StreamResource,
    pub replay: StreamReplay,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamResource {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamReplay {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch: Option<u64>,
}

#[derive(Clone)]
pub struct StreamDomain {
    state: Arc<Mutex<StreamState>>,
    live_tx: broadcast::Sender<StreamEventEnvelope>,
}

#[derive(Debug, Clone)]
struct SubscriptionRecord {
    topics: HashSet<String>,
    filters: SubscriptionFilters,
    cursor: String,
    replay_limit: usize,
    explicit_cursor: bool,
    principal: String,
}

struct StreamState {
    sequence: u64,
    subscription_counter: u64,
    history: VecDeque<StreamEventEnvelope>,
    subscriptions: HashMap<String, SubscriptionRecord>,
}

impl StreamDomain {
    pub fn new() -> Self {
        let (live_tx, _) = broadcast::channel(LIVE_BUFFER_CAPACITY);
        Self {
            state: Arc::new(Mutex::new(StreamState {
                sequence: 1,
                subscription_counter: 0,
                history: VecDeque::with_capacity(HISTORY_LIMIT),
                subscriptions: HashMap::new(),
            })),
            live_tx,
        }
    }

    pub fn subscribe(
        &self,
        params: StreamSubscribeParams,
        principal: &str,
    ) -> Result<StreamSubscribeResult, ApiError> {
        let accepted_topics = normalize_topics(&params.topics, STREAM_TOPICS)?;
        let cursor = if let Some(cursor) = &params.cursor {
            validate_cursor(cursor)?;
            cursor.clone()
        } else {
            self.latest_cursor_or_now()?
        };

        let replay_limit = usize::from(params.replay_limit.unwrap_or(DEFAULT_REPLAY_LIMIT as u16));
        let filters = SubscriptionFilters::from_json(params.filters)?;

        let mut state = self.lock_state()?;
        state.subscription_counter = state.subscription_counter.saturating_add(1);
        let subscription_id = format!(
            "sub-{}-{:04x}",
            Utc::now().timestamp_millis(),
            state.subscription_counter
        );

        let topics = accepted_topics.iter().cloned().collect::<HashSet<_>>();
        state.subscriptions.insert(
            subscription_id.clone(),
            SubscriptionRecord {
                topics,
                filters,
                cursor: cursor.clone(),
                replay_limit,
                explicit_cursor: params.cursor.is_some(),
                principal: principal.to_string(),
            },
        );

        Ok(StreamSubscribeResult {
            subscription_id,
            accepted_topics,
            cursor,
        })
    }

    pub fn get_subscription_principal(&self, subscription_id: &str) -> Option<String> {
        let state = self.lock_state().ok()?;
        state
            .subscriptions
            .get(subscription_id)
            .map(|s| s.principal.clone())
    }

    pub fn unsubscribe(&self, params: StreamUnsubscribeParams) -> Result<(), ApiError> {
        let mut state = self.lock_state()?;
        let removed = state.subscriptions.remove(&params.subscription_id);
        if removed.is_none() {
            return Err(ApiError::not_found(format!(
                "subscription '{}' not found",
                params.subscription_id
            ))
            .with_details(json!({ "subscriptionId": params.subscription_id })));
        }

        Ok(())
    }

    pub fn ack(&self, params: StreamAckParams) -> Result<(), ApiError> {
        validate_cursor(&params.cursor)?;

        let mut state = self.lock_state()?;
        let Some(subscription) = state.subscriptions.get_mut(&params.subscription_id) else {
            return Err(ApiError::not_found(format!(
                "subscription '{}' not found",
                params.subscription_id
            ))
            .with_details(json!({ "subscriptionId": params.subscription_id })));
        };

        if cursor_is_older(&params.cursor, &subscription.cursor)? {
            return Err(ApiError::precondition_failed(
                "stream.ack cursor is older than the subscription checkpoint",
            )
            .with_details(json!({
                "subscriptionId": params.subscription_id,
                "cursor": params.cursor,
                "currentCursor": subscription.cursor
            })));
        }

        subscription.cursor = params.cursor;
        subscription.explicit_cursor = true;
        Ok(())
    }

    pub fn live_receiver(&self) -> broadcast::Receiver<StreamEventEnvelope> {
        self.live_tx.subscribe()
    }

    pub fn has_subscription(&self, subscription_id: &str) -> bool {
        self.state
            .lock()
            .ok()
            .is_some_and(|state| state.subscriptions.contains_key(subscription_id))
    }

    pub fn matches_subscription(&self, subscription_id: &str, event: &StreamEventEnvelope) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };

        let Some(subscription) = state.subscriptions.get(subscription_id) else {
            return false;
        };

        subscription.matches(event)
    }

    pub fn subscription_cursor_sequence(&self, subscription_id: &str) -> Result<u64, ApiError> {
        let state = self.lock_state()?;
        let Some(subscription) = state.subscriptions.get(subscription_id) else {
            return Err(ApiError::not_found(format!(
                "subscription '{}' not found",
                subscription_id
            ))
            .with_details(json!({ "subscriptionId": subscription_id })));
        };

        cursor_sequence(&subscription.cursor)
    }

    pub fn subscription_cursor(&self, subscription_id: &str) -> Result<String, ApiError> {
        let state = self.lock_state()?;
        let Some(subscription) = state.subscriptions.get(subscription_id) else {
            return Err(ApiError::not_found(format!(
                "subscription '{}' not found",
                subscription_id
            ))
            .with_details(json!({ "subscriptionId": subscription_id })));
        };

        Ok(subscription.cursor.clone())
    }

    pub fn replay_for_subscription(&self, subscription_id: &str) -> Result<ReplayBatch, ApiError> {
        let state = self.lock_state()?;
        let Some(subscription) = state.subscriptions.get(subscription_id) else {
            return Err(ApiError::not_found(format!(
                "subscription '{}' not found",
                subscription_id
            ))
            .with_details(json!({ "subscriptionId": subscription_id })));
        };

        let cursor_sequence = cursor_sequence(&subscription.cursor)?;
        let current_cursor = subscription.cursor.clone();
        let mut events = state
            .history
            .iter()
            .filter(|event| {
                event.sequence > cursor_sequence
                    || (event.sequence == cursor_sequence && event.cursor != current_cursor)
            })
            .filter(|event| subscription.matches(event))
            .cloned()
            .collect::<Vec<_>>();

        let dropped_count = events.len().saturating_sub(subscription.replay_limit);
        if dropped_count > 0 {
            events = events.split_off(dropped_count);
        }

        if !events.is_empty() {
            let replay_mode = if subscription.explicit_cursor {
                "resume"
            } else {
                "replay"
            };

            let batch = u64::try_from(events.len()).unwrap_or(u64::MAX);
            for event in &mut events {
                event.replay.mode = replay_mode.to_string();
                event.replay.requested_cursor = Some(subscription.cursor.clone());
                event.replay.batch = Some(batch);
            }
        }

        Ok(ReplayBatch {
            events,
            dropped_count,
        })
    }

    pub fn keepalive_event(&self, subscription_id: &str, interval_ms: u64) -> StreamEventEnvelope {
        self.ephemeral_event(
            "stream.keepalive",
            "stream",
            subscription_id,
            json!({ "intervalMs": interval_ms }),
            "live",
            None,
            None,
        )
    }

    pub fn backpressure_event(
        &self,
        subscription_id: &str,
        dropped_count: usize,
    ) -> StreamEventEnvelope {
        self.ephemeral_event(
            "error.raised",
            "stream",
            subscription_id,
            json!({
                "code": "BACKPRESSURE_DROPPED",
                "message": format!(
                    "subscription '{}' dropped {} event(s) due to backpressure",
                    subscription_id,
                    dropped_count
                ),
                "retryable": true
            }),
            "live",
            None,
            None,
        )
    }

    pub fn publish(&self, topic: &str, resource_type: &str, resource_id: &str, payload: Value) {
        if !STREAM_TOPICS.contains(&topic) {
            return;
        }

        let Ok(mut state) = self.lock_state() else {
            return;
        };

        let event = next_event(
            &mut state,
            topic,
            resource_type,
            resource_id,
            payload,
            "live",
            None,
            None,
        );

        if state.history.len() >= HISTORY_LIMIT {
            state.history.pop_front();
        }
        state.history.push_back(event.clone());
        let _ = self.live_tx.send(event);
    }

    pub fn publish_rpc_side_effect(&self, method: &str, params: &Value, result: &Value) {
        rpc_side_effects::publish_rpc_side_effect(self, method, params, result);
    }

    fn latest_cursor_or_now(&self) -> Result<String, ApiError> {
        let state = self.lock_state()?;
        Ok(state
            .history
            .back()
            .map(|event| event.cursor.clone())
            .unwrap_or_else(|| format!("{}-0", Utc::now().timestamp_millis())))
    }

    fn ephemeral_event(
        &self,
        topic: &str,
        resource_type: &str,
        resource_id: &str,
        payload: Value,
        mode: &str,
        requested_cursor: Option<String>,
        batch: Option<u64>,
    ) -> StreamEventEnvelope {
        let Ok(mut state) = self.lock_state() else {
            return StreamEventEnvelope {
                api_version: API_VERSION.to_string(),
                stream: STREAM_NAME.to_string(),
                topic: topic.to_string(),
                cursor: format!("{}-0", Utc::now().timestamp_millis()),
                sequence: 0,
                ts: now_ts(),
                resource: StreamResource {
                    kind: resource_type.to_string(),
                    id: resource_id.to_string(),
                },
                replay: StreamReplay {
                    mode: mode.to_string(),
                    requested_cursor,
                    batch,
                },
                payload,
            };
        };

        next_event(
            &mut state,
            topic,
            resource_type,
            resource_id,
            payload,
            mode,
            requested_cursor,
            batch,
        )
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, StreamState>, ApiError> {
        self.state
            .lock()
            .map_err(|_| ApiError::internal("stream state lock poisoned"))
    }
}

impl Default for StreamDomain {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionRecord {
    fn matches(&self, event: &StreamEventEnvelope) -> bool {
        self.topics.contains(&event.topic) && self.filters.matches(event)
    }
}

fn next_event(
    state: &mut StreamState,
    topic: &str,
    resource_type: &str,
    resource_id: &str,
    payload: Value,
    mode: &str,
    requested_cursor: Option<String>,
    batch: Option<u64>,
) -> StreamEventEnvelope {
    let sequence = state.sequence;
    state.sequence = state.sequence.saturating_add(1);

    StreamEventEnvelope {
        api_version: API_VERSION.to_string(),
        stream: STREAM_NAME.to_string(),
        topic: topic.to_string(),
        cursor: format!("{}-{sequence}", Utc::now().timestamp_millis()),
        sequence,
        ts: now_ts(),
        resource: StreamResource {
            kind: resource_type.to_string(),
            id: resource_id.to_string(),
        },
        replay: StreamReplay {
            mode: mode.to_string(),
            requested_cursor,
            batch,
        },
        payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscribe_all(domain: &StreamDomain) -> String {
        domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: None,
                },
                "tester",
            )
            .expect("subscribe should succeed")
            .subscription_id
    }

    #[test]
    fn subscribe_accepts_known_topics_and_returns_cursor() {
        let domain = StreamDomain::new();

        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec![
                        "task.status.changed".to_string(),
                        "task.log.line".to_string(),
                    ],
                    cursor: None,
                    replay_limit: Some(50),
                    filters: None,
                },
                "alice",
            )
            .expect("subscribe should succeed");

        assert!(result.subscription_id.starts_with("sub-"));
        assert_eq!(
            result.accepted_topics,
            vec!["task.status.changed".to_string(), "task.log.line".to_string()]
        );
        assert!(
            result.cursor.contains('-'),
            "cursor should be epochMillis-sequence format, got {}",
            result.cursor
        );
        assert_eq!(
            domain.get_subscription_principal(&result.subscription_id),
            Some("alice".to_string())
        );
        assert!(domain.has_subscription(&result.subscription_id));
    }

    #[test]
    fn subscribe_deduplicates_repeated_topics() {
        let domain = StreamDomain::new();

        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec![
                        "task.status.changed".to_string(),
                        "task.status.changed".to_string(),
                    ],
                    cursor: None,
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect("subscribe should succeed");

        assert_eq!(result.accepted_topics, vec!["task.status.changed".to_string()]);
    }

    #[test]
    fn subscribe_rejects_unknown_topic() {
        let domain = StreamDomain::new();

        let err = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["totally.unknown".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect_err("unknown topic should be rejected");

        assert!(err.message.contains("unknown stream topic"), "got: {}", err.message);
    }

    #[test]
    fn subscribe_rejects_empty_topics() {
        let domain = StreamDomain::new();

        let err = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec![],
                    cursor: None,
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect_err("empty topic list should be rejected");

        assert!(
            err.message.contains("at least one topic"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn subscribe_validates_supplied_cursor() {
        let domain = StreamDomain::new();

        let err = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: Some("not-a-cursor".to_string()),
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect_err("malformed cursor should be rejected");

        assert!(
            err.message.contains("cursor must match"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn subscribe_rejects_non_object_filters() {
        let domain = StreamDomain::new();

        let err = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: Some(json!("nope")),
                },
                "bob",
            )
            .expect_err("non-object filters should be rejected");

        assert!(
            err.message.contains("filters must be an object"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn unsubscribe_removes_known_subscription() {
        let domain = StreamDomain::new();
        let subscription_id = subscribe_all(&domain);

        domain
            .unsubscribe(StreamUnsubscribeParams {
                subscription_id: subscription_id.clone(),
            })
            .expect("unsubscribe should succeed");

        assert!(!domain.has_subscription(&subscription_id));
        assert!(domain.get_subscription_principal(&subscription_id).is_none());
    }

    #[test]
    fn unsubscribe_errors_on_unknown_subscription() {
        let domain = StreamDomain::new();

        let err = domain
            .unsubscribe(StreamUnsubscribeParams {
                subscription_id: "sub-missing".to_string(),
            })
            .expect_err("unknown subscription should error");

        assert!(err.message.contains("not found"), "got: {}", err.message);
    }

    #[test]
    fn ack_advances_cursor_on_valid_input() {
        let domain = StreamDomain::new();
        domain.publish(
            "task.status.changed",
            "task",
            "task-1",
            json!({ "from": "open", "to": "done" }),
        );
        let subscription_id = subscribe_all(&domain);
        let original_cursor = domain
            .subscription_cursor(&subscription_id)
            .expect("cursor should exist");
        let original_sequence = cursor_sequence(&original_cursor).unwrap();

        // Build a cursor that's newer than the current one.
        let newer_cursor = format!("{}-{}", Utc::now().timestamp_millis(), original_sequence + 10);

        domain
            .ack(StreamAckParams {
                subscription_id: subscription_id.clone(),
                cursor: newer_cursor.clone(),
            })
            .expect("ack should succeed");

        assert_eq!(
            domain
                .subscription_cursor(&subscription_id)
                .expect("cursor"),
            newer_cursor
        );
        assert_eq!(
            domain
                .subscription_cursor_sequence(&subscription_id)
                .expect("sequence"),
            original_sequence + 10
        );
    }

    #[test]
    fn ack_rejects_older_cursor() {
        let domain = StreamDomain::new();
        domain.publish(
            "task.status.changed",
            "task",
            "task-1",
            json!({ "from": "open", "to": "done" }),
        );
        domain.publish(
            "task.status.changed",
            "task",
            "task-2",
            json!({ "from": "open", "to": "done" }),
        );
        let subscription_id = subscribe_all(&domain);
        let current = domain.subscription_cursor(&subscription_id).unwrap();
        let current_sequence = cursor_sequence(&current).unwrap();
        // Guard against subtracting past zero in case a test seeds with sequence 0.
        let older_sequence = current_sequence.saturating_sub(1);
        let older = format!("0-{older_sequence}");

        let err = domain
            .ack(StreamAckParams {
                subscription_id,
                cursor: older,
            })
            .expect_err("older cursor should be rejected");

        assert!(
            err.message.contains("older"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn ack_rejects_invalid_cursor() {
        let domain = StreamDomain::new();
        let subscription_id = subscribe_all(&domain);

        let err = domain
            .ack(StreamAckParams {
                subscription_id,
                cursor: "bogus".to_string(),
            })
            .expect_err("invalid cursor should be rejected");

        assert!(err.message.contains("cursor must match"), "got: {}", err.message);
    }

    #[test]
    fn ack_errors_on_unknown_subscription() {
        let domain = StreamDomain::new();

        let err = domain
            .ack(StreamAckParams {
                subscription_id: "sub-missing".to_string(),
                cursor: "100-1".to_string(),
            })
            .expect_err("unknown subscription should error");

        assert!(err.message.contains("not found"), "got: {}", err.message);
    }

    #[test]
    fn publish_appends_to_history_and_broadcasts_live() {
        let domain = StreamDomain::new();
        let mut live_rx = domain.live_receiver();

        domain.publish(
            "task.status.changed",
            "task",
            "task-42",
            json!({ "from": "none", "to": "open" }),
        );

        let event = live_rx
            .try_recv()
            .expect("publish should push to live receiver");
        assert_eq!(event.topic, "task.status.changed");
        assert_eq!(event.resource.kind, "task");
        assert_eq!(event.resource.id, "task-42");
        assert_eq!(event.stream, STREAM_NAME);
        assert_eq!(event.api_version, API_VERSION);
        assert_eq!(event.replay.mode, "live");
        assert!(event.replay.requested_cursor.is_none());
        assert!(event.replay.batch.is_none());
        assert!(event.sequence >= 1);
    }

    #[test]
    fn publish_ignores_unknown_topic() {
        let domain = StreamDomain::new();
        let mut live_rx = domain.live_receiver();

        domain.publish("not.a.topic", "task", "task-x", json!({}));

        // No event should have been broadcast.
        assert!(live_rx.try_recv().is_err());
    }

    #[test]
    fn publish_assigns_monotonic_sequences() {
        let domain = StreamDomain::new();
        let mut live_rx = domain.live_receiver();

        domain.publish("task.status.changed", "task", "t1", json!({}));
        domain.publish("task.status.changed", "task", "t2", json!({}));
        domain.publish("task.status.changed", "task", "t3", json!({}));

        let seq1 = live_rx.try_recv().unwrap().sequence;
        let seq2 = live_rx.try_recv().unwrap().sequence;
        let seq3 = live_rx.try_recv().unwrap().sequence;

        assert!(seq1 < seq2 && seq2 < seq3, "sequences should be monotonic, got {seq1} {seq2} {seq3}");
    }

    #[test]
    fn replay_returns_events_after_cursor_in_replay_mode() {
        let domain = StreamDomain::new();
        let subscription_id = subscribe_all(&domain);

        domain.publish(
            "task.status.changed",
            "task",
            "task-1",
            json!({ "from": "none", "to": "open" }),
        );
        domain.publish(
            "task.status.changed",
            "task",
            "task-2",
            json!({ "from": "open", "to": "done" }),
        );

        let batch = domain
            .replay_for_subscription(&subscription_id)
            .expect("replay should succeed");

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.dropped_count, 0);
        for event in &batch.events {
            // Initial subscription used no explicit cursor, so mode is "replay".
            assert_eq!(event.replay.mode, "replay");
            assert_eq!(event.replay.batch, Some(2));
            assert!(event.replay.requested_cursor.is_some());
        }
    }

    #[test]
    fn replay_uses_resume_mode_when_cursor_was_explicit() {
        let domain = StreamDomain::new();
        // Publish first so we have a valid cursor to resume from.
        domain.publish("task.status.changed", "task", "task-0", json!({}));
        let first_cursor = domain
            .state
            .lock()
            .unwrap()
            .history
            .back()
            .unwrap()
            .cursor
            .clone();

        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: Some(first_cursor),
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect("subscribe with explicit cursor");

        domain.publish("task.status.changed", "task", "task-1", json!({}));
        domain.publish("task.status.changed", "task", "task-2", json!({}));

        let batch = domain
            .replay_for_subscription(&result.subscription_id)
            .expect("replay");

        assert_eq!(batch.events.len(), 2);
        for event in &batch.events {
            assert_eq!(event.replay.mode, "resume");
        }
    }

    #[test]
    fn replay_honors_replay_limit_and_reports_dropped() {
        let domain = StreamDomain::new();
        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: Some(2),
                    filters: None,
                },
                "bob",
            )
            .expect("subscribe");

        for i in 0..5 {
            domain.publish(
                "task.status.changed",
                "task",
                &format!("task-{i}"),
                json!({ "to": "done" }),
            );
        }

        let batch = domain
            .replay_for_subscription(&result.subscription_id)
            .expect("replay");

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.dropped_count, 3);
        // The kept events should be the most recent two (split_off drops oldest).
        assert_eq!(batch.events[0].resource.id, "task-3");
        assert_eq!(batch.events[1].resource.id, "task-4");
    }

    #[test]
    fn replay_filters_by_resource_id() {
        let domain = StreamDomain::new();
        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: Some(json!({ "resourceId": "task-2" })),
                },
                "bob",
            )
            .expect("subscribe");

        domain.publish("task.status.changed", "task", "task-1", json!({}));
        domain.publish("task.status.changed", "task", "task-2", json!({}));
        domain.publish("task.status.changed", "task", "task-3", json!({}));

        let batch = domain
            .replay_for_subscription(&result.subscription_id)
            .expect("replay");

        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].resource.id, "task-2");
    }

    #[test]
    fn replay_filters_by_resource_type_array() {
        let domain = StreamDomain::new();
        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec![
                        "task.status.changed".to_string(),
                        "loop.status.changed".to_string(),
                    ],
                    cursor: None,
                    replay_limit: None,
                    filters: Some(json!({ "resourceTypes": ["loop"] })),
                },
                "bob",
            )
            .expect("subscribe");

        domain.publish("task.status.changed", "task", "task-1", json!({}));
        domain.publish("loop.status.changed", "loop", "loop-1", json!({}));

        let batch = domain
            .replay_for_subscription(&result.subscription_id)
            .expect("replay");

        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].resource.kind, "loop");
    }

    #[test]
    fn replay_errors_on_unknown_subscription() {
        let domain = StreamDomain::new();

        let err = domain
            .replay_for_subscription("sub-missing")
            .expect_err("unknown subscription should error");

        assert!(err.message.contains("not found"), "got: {}", err.message);
    }

    #[test]
    fn subscription_cursor_and_sequence_error_on_unknown_subscription() {
        let domain = StreamDomain::new();

        assert!(domain.subscription_cursor("sub-missing").is_err());
        assert!(domain.subscription_cursor_sequence("sub-missing").is_err());
    }

    #[test]
    fn matches_subscription_respects_topic_and_filters() {
        let domain = StreamDomain::new();
        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: Some(json!({ "resourceId": "task-keep" })),
                },
                "bob",
            )
            .expect("subscribe");

        let matching = StreamEventEnvelope {
            api_version: API_VERSION.to_string(),
            stream: STREAM_NAME.to_string(),
            topic: "task.status.changed".to_string(),
            cursor: "1-1".to_string(),
            sequence: 1,
            ts: now_ts(),
            resource: StreamResource {
                kind: "task".to_string(),
                id: "task-keep".to_string(),
            },
            replay: StreamReplay {
                mode: "live".to_string(),
                requested_cursor: None,
                batch: None,
            },
            payload: json!({}),
        };
        assert!(domain.matches_subscription(&result.subscription_id, &matching));

        let wrong_topic = StreamEventEnvelope {
            topic: "task.log.line".to_string(),
            ..matching.clone()
        };
        assert!(!domain.matches_subscription(&result.subscription_id, &wrong_topic));

        let wrong_id = StreamEventEnvelope {
            resource: StreamResource {
                kind: "task".to_string(),
                id: "task-other".to_string(),
            },
            ..matching.clone()
        };
        assert!(!domain.matches_subscription(&result.subscription_id, &wrong_id));

        // Unknown subscription: always false.
        assert!(!domain.matches_subscription("sub-missing", &matching));
    }

    #[test]
    fn keepalive_event_has_expected_shape() {
        let domain = StreamDomain::new();

        let event = domain.keepalive_event("sub-123", KEEPALIVE_INTERVAL_MS);

        assert_eq!(event.topic, "stream.keepalive");
        assert_eq!(event.resource.kind, "stream");
        assert_eq!(event.resource.id, "sub-123");
        assert_eq!(
            event.payload,
            json!({ "intervalMs": KEEPALIVE_INTERVAL_MS })
        );
        assert_eq!(event.replay.mode, "live");
    }

    #[test]
    fn backpressure_event_encodes_dropped_count() {
        let domain = StreamDomain::new();

        let event = domain.backpressure_event("sub-456", 7);

        assert_eq!(event.topic, "error.raised");
        assert_eq!(event.resource.id, "sub-456");
        assert_eq!(
            event.payload["code"],
            Value::String("BACKPRESSURE_DROPPED".to_string())
        );
        assert_eq!(event.payload["retryable"], Value::Bool(true));
        let message = event.payload["message"].as_str().expect("message string");
        assert!(message.contains("sub-456"));
        assert!(message.contains('7'));
    }

    #[test]
    fn ephemeral_events_do_not_enter_history() {
        let domain = StreamDomain::new();
        let subscription_id = subscribe_all(&domain);

        let _ = domain.keepalive_event(&subscription_id, KEEPALIVE_INTERVAL_MS);
        let _ = domain.backpressure_event(&subscription_id, 1);

        let batch = domain
            .replay_for_subscription(&subscription_id)
            .expect("replay");
        assert!(batch.events.is_empty());
        assert_eq!(batch.dropped_count, 0);
    }

    #[test]
    fn history_is_capped_at_history_limit() {
        let domain = StreamDomain::new();

        for i in 0..(HISTORY_LIMIT + 32) {
            domain.publish(
                "task.status.changed",
                "task",
                &format!("task-{i}"),
                json!({}),
            );
        }

        let state = domain.state.lock().unwrap();
        assert_eq!(state.history.len(), HISTORY_LIMIT);
        // The oldest retained event should not be task-0.
        let oldest = state.history.front().unwrap();
        assert_ne!(oldest.resource.id, "task-0");
    }

    #[test]
    fn default_matches_new() {
        let a = StreamDomain::default();
        let b = StreamDomain::new();
        assert!(!a.has_subscription("anything"));
        assert!(!b.has_subscription("anything"));
    }

    #[test]
    fn subscribe_without_history_produces_synthetic_cursor() {
        let domain = StreamDomain::new();
        let result = domain
            .subscribe(
                StreamSubscribeParams {
                    topics: vec!["task.status.changed".to_string()],
                    cursor: None,
                    replay_limit: None,
                    filters: None,
                },
                "bob",
            )
            .expect("subscribe");

        // No events published yet; cursor should still parse as <millis>-<seq>.
        assert!(cursor_sequence(&result.cursor).is_ok());
    }

    #[test]
    fn publish_rpc_side_effect_emits_task_status_changed() {
        let domain = StreamDomain::new();
        let subscription_id = subscribe_all(&domain);

        domain.publish_rpc_side_effect(
            "task.create",
            &json!({}),
            &json!({ "task": { "id": "task-99", "status": "open" } }),
        );

        let batch = domain
            .replay_for_subscription(&subscription_id)
            .expect("replay");
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].topic, "task.status.changed");
        assert_eq!(batch.events[0].resource.id, "task-99");
        assert_eq!(batch.events[0].payload["to"], Value::String("open".to_string()));
    }
}
