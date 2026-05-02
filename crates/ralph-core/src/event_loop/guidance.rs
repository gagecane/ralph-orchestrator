//! Human-in-the-loop guidance handling (caching, scratchpad persistence, prompt injection).

use super::EventLoop;
use ralph_proto::Event;
use serde_json::{Map, Value};
use tracing::{debug, info, warn};

impl EventLoop {
    /// Injects human guidance messages so the next prompt includes them.
    ///
    /// Records a `human.guidance` event on the bus and loop state for each message.
    ///
    /// This is used for local TUI/RPC guidance so the next prompt boundary
    /// sees the message immediately without waiting for a JSONL reread.
    pub fn inject_human_guidance<I, S>(&mut self, messages: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for message in messages {
            let event = Event::new("human.guidance", message.into());
            self.state.record_event(&event);
            self.bus.publish(event);
        }
    }

    /// Stores guidance payloads, persists them to scratchpad, and prepares them for prompt injection.
    ///
    /// Guidance events are ephemeral in the event bus (consumed by `take_pending`).
    /// This method both caches them in memory for prompt injection and appends
    /// them to the scratchpad file so they survive across process restarts.
    pub(super) fn update_robot_guidance(&mut self, guidance_events: Vec<Event>) {
        if guidance_events.is_empty() {
            return;
        }

        // Persist new guidance to scratchpad before caching
        self.persist_guidance_to_scratchpad(&guidance_events);

        self.robot_guidance
            .extend(guidance_events.into_iter().map(|e| e.payload));
    }

    /// Appends human guidance entries to the scratchpad file for durability.
    ///
    /// Each guidance message is written as a timestamped markdown entry so it
    /// appears alongside the agent's own thinking and survives process restarts.
    ///
    /// When scratchpad is disabled for the current hat, persists to the global
    /// scratchpad path (guidance is cross-hat state). If global is also disabled,
    /// skips persistence.
    pub(super) fn persist_guidance_to_scratchpad(&self, guidance_events: &[Event]) {
        use std::io::Write;

        // When hat scratchpad is disabled, fall back to global scratchpad
        let scratchpad_path = if self.ralph.active_scratchpad().enabled {
            self.scratchpad_path()
        } else {
            if !self.config.core.scratchpad.enabled {
                debug!("Both hat and global scratchpad disabled, skipping guidance persistence");
                return;
            }
            self.global_scratchpad_path()
        };
        let resolved_path = if scratchpad_path.is_relative() {
            self.config.core.workspace_root.join(&scratchpad_path)
        } else {
            scratchpad_path
        };

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            warn!("Failed to create scratchpad directory: {}", e);
            return;
        }

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open scratchpad for guidance persistence: {}", e);
                return;
            }
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        for event in guidance_events {
            let entry = format!(
                "\n### HUMAN GUIDANCE ({})\n\n{}\n",
                timestamp, event.payload
            );
            if let Err(e) = file.write_all(entry.as_bytes()) {
                warn!("Failed to write guidance to scratchpad: {}", e);
            }
        }

        info!(
            count = guidance_events.len(),
            "Persisted human guidance to scratchpad"
        );
    }

    /// Injects cached guidance into the next prompt build.
    pub(super) fn apply_robot_guidance(&mut self) {
        if self.robot_guidance.is_empty() {
            return;
        }

        self.ralph.set_robot_guidance(self.robot_guidance.clone());
    }

    pub(super) fn parse_human_interact_context(payload: &str) -> Value {
        let mut context = match serde_json::from_str::<Value>(payload) {
            Ok(Value::Object(map)) => map,
            Ok(value) => {
                let mut map = Map::new();
                map.insert("question".to_string(), value);
                map
            }
            Err(_) => {
                let mut map = Map::new();
                map.insert("question".to_string(), Value::String(payload.to_string()));
                map
            }
        };

        if !context.contains_key("question") {
            context.insert("question".to_string(), Value::String(payload.to_string()));
        }

        Value::Object(context)
    }
}
