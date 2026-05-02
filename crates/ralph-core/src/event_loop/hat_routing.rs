//! Hat routing, activation, and dispatch helpers.

use super::EventLoop;
use ralph_proto::{Event, Hat, HatId};
use tracing::{debug, info, warn};

impl EventLoop {
    /// Returns the backend override for a specific hat, if one is configured.
    ///
    /// If the hat has a backend configured, returns that.
    /// Otherwise, returns None (caller should use global backend).
    pub fn get_hat_backend(&self, hat_id: &HatId) -> Option<&crate::config::HatBackend> {
        self.registry
            .get_config(hat_id)
            .and_then(|config| config.backend.as_ref())
    }

    /// Gets the next hat to execute (if any have pending events).
    ///
    /// Per "Hatless Ralph" architecture: When custom hats are defined, Ralph is
    /// always the executor. Custom hats define topology (pub/sub contracts) that
    /// Ralph uses for coordination context, but Ralph handles all iterations.
    ///
    /// - Solo mode (no custom hats): Returns "ralph" if Ralph has pending events
    /// - Multi-hat mode (custom hats defined): Always returns "ralph" if ANY hat has pending events
    pub fn next_hat(&self) -> Option<&HatId> {
        let next = self.bus.next_hat_with_pending();

        // If no pending hat events but human interactions are pending, route to Ralph.
        if next.is_none() && self.bus.has_human_pending() {
            return self.bus.hat_ids().find(|id| id.as_str() == "ralph");
        }

        // If no pending events, return None
        next.as_ref()?;

        // In multi-hat mode, always route to Ralph (custom hats define topology only)
        // Ralph's prompt includes the ## HATS section for coordination awareness
        if self.registry.is_empty() {
            // Solo mode - return the next hat (which is "ralph")
            next
        } else {
            // Return "ralph" - the constant coordinator
            // Find ralph in the bus's registered hats
            self.bus.hat_ids().find(|id| id.as_str() == "ralph")
        }
    }

    /// Gets the topics a hat is allowed to publish.
    ///
    /// Used to build retry prompts when the LLM forgets to publish an event.
    pub fn get_hat_publishes(&self, hat_id: &HatId) -> Vec<String> {
        self.registry
            .get(hat_id)
            .map(|hat| hat.publishes.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    /// Injects a fallback event to recover from a stalled loop.
    ///
    /// When no hats have pending events (agent failed to publish), this method
    /// injects a `task.resume` event which Ralph will handle to attempt recovery.
    ///
    /// Returns true if a fallback event was injected, false if recovery is not possible.
    pub fn inject_fallback_event(&mut self) -> bool {
        // If a custom hat was last executing, target the fallback back to it
        // This preserves hat context instead of always falling back to Ralph
        let fallback_event = match &self.state.last_hat {
            Some(hat_id) if hat_id.as_str() != "ralph" => {
                let publishes = self.get_hat_publishes(hat_id);
                let payload = if publishes.is_empty() {
                    format!(
                        "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         Emit exactly one valid next event via `ralph emit`, or stop only after \
                         publishing the configured completion event.",
                        hat_id.as_str()
                    )
                } else {
                    format!(
                        "RECOVERY: Previous iteration by hat `{}` did not publish an event. \
                         This failed because no event was emitted. Emit exactly ONE valid next \
                         event via `ralph emit`. Allowed topics: `{}`. Do not only write prose \
                         or update files. Stop immediately after emitting.",
                        hat_id.as_str(),
                        publishes.join("`, `")
                    )
                };

                debug!(
                    hat = %hat_id.as_str(),
                    "Injecting fallback event to recover - targeting last hat with task.resume"
                );
                Event::new("task.resume", payload).with_target(hat_id.clone())
            }
            _ => {
                debug!("Injecting fallback event to recover - triggering Ralph with task.resume");
                Event::new(
                    "task.resume",
                    "RECOVERY: Previous iteration did not publish an event. \
                     Review the scratchpad and either dispatch the next task or complete the loop.",
                )
            }
        };

        self.bus.publish(fallback_event);
        true
    }

    /// Determines which hats should be active based on pending events.
    /// Returns list of Hat references that are triggered by any pending event.
    pub(super) fn determine_active_hats(&self, events: &[Event]) -> Vec<&Hat> {
        let mut active_hats = Vec::new();
        for id in self.determine_active_hat_ids(events) {
            if let Some(hat) = self.registry.get(&id) {
                active_hats.push(hat);
            }
        }
        active_hats
    }

    pub(super) fn determine_active_hat_ids(&self, events: &[Event]) -> Vec<HatId> {
        let mut entrypoint_hat_ids = Vec::new();
        let mut progressed_hat_ids = Vec::new();
        for event in events {
            // Prefer direct event target over topic-based lookup
            let hat_id = if let Some(target) = &event.target
                && self.registry.get(target).is_some()
            {
                target.clone()
            } else if let Some(hat) = self.registry.get_for_topic(event.topic.as_str()) {
                hat.id.clone()
            } else {
                continue;
            };

            let list = if self.is_entrypoint_topic(event.topic.as_str()) {
                &mut entrypoint_hat_ids
            } else {
                &mut progressed_hat_ids
            };
            if !list.iter().any(|id| id == &hat_id) {
                list.push(hat_id);
            }
        }
        // Prefer progressed hats over entrypoint hats. Entrypoint events
        // (starting_event, task.start, task.resume) linger in the bus after
        // the first hat runs. Including them would re-activate the first hat
        // alongside downstream hats, confusing the agent with multiple hat
        // instructions when only the downstream hat should run.
        if progressed_hat_ids.is_empty() {
            entrypoint_hat_ids
        } else {
            progressed_hat_ids
        }
    }

    pub(super) fn effective_regular_events<'a>(&self, events: &'a [Event]) -> Vec<&'a Event> {
        let has_downstream_event = events
            .iter()
            .any(|event| !Self::is_kickoff_or_recovery_event(event.topic.as_str()));
        events
            .iter()
            .filter(|event| {
                !has_downstream_event || !Self::is_kickoff_or_recovery_event(event.topic.as_str())
            })
            .collect()
    }

    pub(super) fn is_kickoff_or_recovery_event(topic: &str) -> bool {
        topic == "task.start" || topic == "task.resume" || topic.strip_suffix(".start").is_some()
    }

    pub(super) fn is_entrypoint_topic(&self, topic: &str) -> bool {
        topic == "task.start"
            || topic == "task.resume"
            || topic.strip_suffix(".start").is_some()
            || self.config.event_loop.starting_event.as_deref() == Some(topic)
    }

    pub(super) fn peek_pending_regular_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for hat_id in self.bus.hat_ids() {
            if let Some(pending) = self.bus.peek_pending(hat_id) {
                events.extend(pending.iter().cloned());
            }
        }
        events
    }

    /// Formats an event for prompt context.
    ///
    /// For top-level prompts (task.start, task.resume), wraps the payload in
    /// `<top-level-prompt>` XML tags to clearly delineate the user's original request.
    pub(super) fn format_event(event: &Event) -> String {
        let topic = &event.topic;
        let payload = &event.payload;

        if topic.as_str() == "task.start" || topic.as_str() == "task.resume" {
            format!(
                "Event: {} - <top-level-prompt>\n{}\n</top-level-prompt>",
                topic, payload
            )
        } else {
            format!("Event: {} - {}", topic, payload)
        }
    }

    pub(super) fn check_hat_exhaustion(
        &mut self,
        hat_id: &HatId,
        dropped: &[Event],
    ) -> (bool, Option<Event>) {
        let Some(config) = self.registry.get_config(hat_id) else {
            return (false, None);
        };
        let Some(max) = config.max_activations else {
            return (false, None);
        };

        let count = *self.state.hat_activation_counts.get(hat_id).unwrap_or(&0);
        if count < max {
            return (false, None);
        }

        // Emit only once per hat per run (avoid flooding).
        let should_emit = self.state.exhausted_hats.insert(hat_id.clone());

        if !should_emit {
            // Hat is already exhausted - drop pending events silently.
            return (true, None);
        }

        let mut dropped_topics: Vec<String> = dropped.iter().map(|e| e.topic.to_string()).collect();
        dropped_topics.sort();

        let payload = format!(
            "Hat '{hat}' exhausted.\n- max_activations: {max}\n- activations: {count}\n- dropped_topics:\n  - {topics}",
            hat = hat_id.as_str(),
            max = max,
            count = count,
            topics = dropped_topics.join("\n  - ")
        );

        warn!(
            hat = %hat_id.as_str(),
            max_activations = max,
            activations = count,
            "Hat exhausted (max_activations reached)"
        );

        (
            true,
            Some(Event::new(
                format!("{}.exhausted", hat_id.as_str()),
                payload,
            )),
        )
    }

    pub(super) fn record_hat_activations(&mut self, active_hat_ids: &[HatId]) {
        for hat_id in active_hat_ids {
            *self
                .state
                .hat_activation_counts
                .entry(hat_id.clone())
                .or_insert(0) += 1;
        }
    }

    /// Returns the primary active hat ID for display purposes.
    /// Returns the first active hat, or "ralph" if no specific hat is active.
    /// BTreeMap iteration is already sorted by key.
    pub fn get_active_hat_id(&self) -> HatId {
        let pending_events = self.peek_pending_regular_events();
        if let Some(active_hat_id) = self
            .determine_active_hat_ids(&pending_events)
            .into_iter()
            .next()
        {
            return active_hat_id;
        }
        HatId::new("ralph")
    }

    /// Injects a default event for a hat when the agent wrote no events.
    ///
    /// Call this after `process_events_from_jsonl` returns `Ok(false)` (no events found).
    /// If the hat has `default_publishes` configured, this injects the default event.
    ///
    /// If the default topic matches the completion promise, `completion_requested` is set
    /// so the loop can terminate. Without this, completion events injected via
    /// `default_publishes` would only be published to the bus (triggering downstream hats)
    /// but never detected by `check_completion_event`, causing an infinite loop.
    pub fn check_default_publishes(&mut self, hat_id: &HatId) {
        if let Some(config) = self.registry.get_config(hat_id)
            && let Some(default_topic) = &config.default_publishes
        {
            let default_event = Event::new(default_topic.as_str(), "").with_source(hat_id.clone());

            debug!(
                hat = %hat_id.as_str(),
                topic = %default_topic,
                "No events written by hat, injecting default_publishes event"
            );

            self.state.record_event(&default_event);

            // If the default topic is the completion promise, set the flag directly.
            // The normal path (process_events_from_jsonl) sets this when reading from
            // JSONL, but default_publishes bypasses JSONL entirely.
            if default_topic.as_str() == self.config.event_loop.completion_promise {
                info!(
                    hat = %hat_id.as_str(),
                    topic = %default_topic,
                    "default_publishes matches completion_promise — requesting termination"
                );
                self.state.completion_requested = true;
            }

            self.bus.publish(default_event);
        }
    }
}
