//! Event-processing pipeline: JSONL reading, orphan routing, backpressure,
//! human interact handling, wave partitioning.

use super::{EventLoop, TerminationReason};
use crate::event_parser::EventParser;
use ralph_proto::{Event, HatId};
use serde_json::{Map, Value};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Result of processing events from JSONL.
#[derive(Debug, Clone)]
pub struct ProcessedEvents {
    /// Whether any valid events were found and published.
    pub had_events: bool,
    /// Whether any published events matched the semantic `plan.*` topic family.
    pub had_plan_events: bool,
    /// Structured context for the first processed `human.interact` event,
    /// including the question payload and post-dispatch outcome metadata.
    pub human_interact_context: Option<Value>,
    /// Whether any events lacked specific hat subscribers (orphans handled by Ralph).
    pub has_orphans: bool,
}

/// Result of processing events from JSONL with wave events partitioned out.
#[derive(Debug)]
pub struct ProcessedEventsWithWaves {
    /// Normal event processing results.
    pub processed: ProcessedEvents,
    /// Wave events extracted before normal processing (have wave_id set).
    pub wave_events: Vec<crate::event_reader::Event>,
}

impl EventLoop {
    /// Advances the event reader to the current end of the events file.
    ///
    /// Call this after writing observability records (e.g. start event) to the
    /// events JSONL file so they are not re-read by `process_events_from_jsonl`.
    /// The start event is already published to the bus via `initialize()`, so
    /// re-reading it from the file would cause double-delivery.
    pub fn sync_event_reader_to_file_end(&mut self) {
        let path = self.event_reader.path();
        if let Ok(metadata) = std::fs::metadata(path) {
            self.event_reader.set_position(metadata.len());
        }
    }

    /// Checks if any hats have pending events.
    ///
    /// Use this after `process_output` to detect if the LLM failed to publish an event.
    /// If false after processing, the loop will terminate on the next iteration.
    pub fn has_pending_events(&self) -> bool {
        self.bus.next_hat_with_pending().is_some() || self.bus.has_human_pending()
    }

    /// Checks if any pending events are human-related (human.response, human.guidance).
    ///
    /// Used to skip cooldown delays when a human event is next, since we don't
    /// want to artificially delay the response to a human interaction.
    pub fn has_pending_human_events(&self) -> bool {
        self.bus.has_human_pending()
    }

    /// Returns whether unread JSONL events include any semantic `plan.*` topics.
    ///
    /// This allows callers to dispatch `pre.plan.created` hooks before
    /// event publication handling without consuming unread events.
    pub fn has_pending_plan_events_in_jsonl(&self) -> std::io::Result<bool> {
        let result = self.event_reader.peek_new_events()?;
        Ok(result
            .events
            .iter()
            .any(|event| event.topic.starts_with("plan.")))
    }

    /// Returns structured context for the first unread `human.interact` event,
    /// if one is present in JSONL without consuming reader state.
    pub fn pending_human_interact_context_in_jsonl(&self) -> std::io::Result<Option<Value>> {
        let result = self.event_reader.peek_new_events()?;
        Ok(result
            .events
            .iter()
            .find(|event| event.topic == "human.interact")
            .map(|event| {
                Self::parse_human_interact_context(event.payload.as_deref().unwrap_or_default())
            }))
    }

    /// Processes output from a hat execution.
    ///
    /// Returns the termination reason if the loop should stop.
    pub fn process_output(
        &mut self,
        hat_id: &HatId,
        output: &str,
        success: bool,
    ) -> Option<TerminationReason> {
        self.state.iteration += 1;
        self.state.last_hat = Some(hat_id.clone());

        // Periodic robot check-in
        if let Some(interval_secs) = self.config.robot.checkin_interval_seconds
            && let Some(ref robot_service) = self.robot_service
        {
            let elapsed = self.state.elapsed();
            let interval = std::time::Duration::from_secs(interval_secs);
            let last = self
                .state
                .last_checkin_at
                .map(|t| t.elapsed())
                .unwrap_or(elapsed);

            if last >= interval {
                let context = self.build_checkin_context(hat_id);
                match robot_service.send_checkin(self.state.iteration, elapsed, Some(&context)) {
                    Ok(_) => {
                        self.state.last_checkin_at = Some(std::time::Instant::now());
                        debug!(iteration = self.state.iteration, "Sent robot check-in");
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to send robot check-in");
                    }
                }
            }
        }

        // Log iteration started
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::IterationStarted,
        );

        // Log hat selected
        self.diagnostics.log_orchestration(
            self.state.iteration,
            "loop",
            crate::diagnostics::OrchestrationEvent::HatSelected {
                hat: hat_id.to_string(),
                reason: "process_output".to_string(),
            },
        );

        // Track failures
        if success {
            self.state.consecutive_failures = 0;
        } else {
            self.state.consecutive_failures += 1;
        }

        let _ = output;

        // File-modification audit: detect when a hat with disallowed Edit/Write tools
        // modified files. This is hard enforcement — emits a scope_violation event.
        self.audit_file_modifications(hat_id);

        // Events are ONLY read from the JSONL file written by `ralph emit`.
        // This enforces tool use and prevents confabulation (agent claiming to emit without actually doing so).
        // See process_events_from_jsonl() for event processing.

        // Check termination conditions
        self.check_termination()
    }

    /// Processes events from JSONL and routes orphaned events to Ralph.
    ///
    /// Also handles backpressure for malformed JSONL lines by:
    /// 1. Emitting `event.malformed` system events for each parse failure
    /// 2. Tracking consecutive failures for termination check
    /// 3. Resetting counter when valid events are parsed
    ///
    /// Returns [`ProcessedEvents`] indicating whether events were found, whether
    /// semantic `plan.*` topics were published, structured `human.interact`
    /// context/outcome metadata, and whether any were orphans that Ralph should
    /// handle.
    pub fn process_events_from_jsonl(&mut self) -> std::io::Result<ProcessedEvents> {
        let result = self.event_reader.read_new_events()?;
        self.process_parse_result(result)
    }

    /// Inner event processing that operates on an already-parsed `ParseResult`.
    ///
    /// This is the single source of truth for event validation, backpressure,
    /// scope enforcement, and bus publishing. Both `process_events_from_jsonl`
    /// and `process_events_from_jsonl_with_waves` delegate to this method.
    pub(super) fn process_parse_result(
        &mut self,
        result: crate::event_reader::ParseResult,
    ) -> std::io::Result<ProcessedEvents> {
        // Handle malformed lines with backpressure
        for malformed in &result.malformed {
            let payload = format!(
                "Line {}: {}\nContent: {}",
                malformed.line_number, malformed.error, &malformed.content
            );
            let event = Event::new("event.malformed", &payload);
            self.bus.publish(event);
            self.state.consecutive_malformed_events += 1;
            warn!(
                line = malformed.line_number,
                consecutive = self.state.consecutive_malformed_events,
                "Malformed event line detected"
            );
        }

        // Reset counter when valid events are parsed
        if !result.events.is_empty() {
            self.state.consecutive_malformed_events = 0;
        }

        if result.events.is_empty() && result.malformed.is_empty() {
            return Ok(ProcessedEvents {
                had_events: false,
                had_plan_events: false,
                human_interact_context: None,
                has_orphans: false,
            });
        }

        // --- Scope enforcement: filter events against active hat's publishes ---
        // Only active when enforce_hat_scope is true in config (opt-in).
        let events = if self.config.event_loop.enforce_hat_scope {
            let active_hats = self.state.last_active_hat_ids.clone();
            let (in_scope, out_of_scope): (Vec<_>, Vec<_>) =
                result.events.into_iter().partition(|event| {
                    if active_hats.is_empty() {
                        return true; // Ralph coordinating — no scope restriction
                    }
                    active_hats
                        .iter()
                        .any(|hat_id| self.registry.can_publish(hat_id, event.topic.as_str()))
                });

            for event in &out_of_scope {
                let violation_hat = active_hats.first().map(|h| h.as_str()).unwrap_or("unknown");
                warn!(
                    active_hats = ?active_hats,
                    topic = %event.topic,
                    "Scope violation: active hat(s) cannot publish this topic — dropping event"
                );
                let violation_topic = format!("{}.scope_violation", violation_hat);
                let violation_payload = format!(
                    "Attempted to publish '{}': {}",
                    event.topic,
                    event.payload.clone().unwrap_or_default()
                );
                let violation = Event::new(violation_topic, violation_payload);
                self.bus.publish(violation);
            }

            in_scope
        } else {
            result.events
        };
        // --- End scope enforcement ---

        let mut has_orphans = false;

        // Validate and transform events (apply backpressure for build.done)
        let mut validated_events = Vec::new();
        let completion_topic = self.config.event_loop.completion_promise.as_str();
        let cancellation_topic = self.config.event_loop.cancellation_promise.clone();
        let total_events = events.len();
        for (index, event) in events.into_iter().enumerate() {
            let payload = event.payload.clone().unwrap_or_default();

            // Detect loop.cancel — unconditional graceful termination
            if !cancellation_topic.is_empty() && event.topic.as_str() == cancellation_topic {
                info!(
                    payload = %payload,
                    "loop.cancel event detected — scheduling graceful termination"
                );
                self.state.cancellation_requested = true;
                // Continue processing remaining events (they may contain cleanup info)
                continue;
            }

            if event.topic == completion_topic {
                if index + 1 == total_events {
                    self.state.completion_requested = true;
                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::EventPublished {
                            topic: event.topic.clone(),
                        },
                    );
                    info!(
                        topic = %event.topic,
                        "Completion event detected in JSONL"
                    );
                } else {
                    warn!(
                        topic = %event.topic,
                        index = index,
                        total_events = total_events,
                        "Completion event ignored because it was not the last event"
                    );
                }
                continue;
            }

            if event.topic == "build.done" {
                // Validate build.done events have backpressure evidence
                if let Some(evidence) = EventParser::parse_backpressure_evidence(&payload) {
                    if evidence.all_passed() {
                        self.warn_on_mutation_evidence(&evidence);
                        validated_events.push(Event::new(event.topic.as_str(), &payload));
                    } else {
                        // Evidence present but checks failed - synthesize build.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            lint = evidence.lint_passed,
                            typecheck = evidence.typecheck_passed,
                            audit = evidence.audit_passed,
                            coverage = evidence.coverage_passed,
                            complexity = evidence.complexity_score,
                            duplication = evidence.duplication_passed,
                            performance = evidence.performance_regression,
                            specs = evidence.specs_verified,
                            "build.done rejected: backpressure checks failed"
                        );

                        let complexity = evidence
                            .complexity_score
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_else(|| "missing".to_string());
                        let performance = match evidence.performance_regression {
                            Some(true) => "regression".to_string(),
                            Some(false) => "pass".to_string(),
                            None => "missing".to_string(),
                        };
                        let specs = match evidence.specs_verified {
                            Some(true) => "pass".to_string(),
                            Some(false) => "fail".to_string(),
                            None => "not reported".to_string(),
                        };

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "backpressure checks failed: tests={}, lint={}, typecheck={}, audit={}, coverage={}, complexity={}, duplication={}, performance={}, specs={}",
                                    evidence.tests_passed,
                                    evidence.lint_passed,
                                    evidence.typecheck_passed,
                                    evidence.audit_passed,
                                    evidence.coverage_passed,
                                    complexity,
                                    evidence.duplication_passed,
                                    performance,
                                    specs
                                ),
                            },
                        );

                        validated_events.push(Event::new(
                            "build.blocked",
                            "Backpressure checks failed. Fix tests/lint/typecheck/audit/coverage/complexity/duplication/specs before emitting build.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize build.blocked
                    warn!("build.done rejected: missing backpressure evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing backpressure evidence".to_string(),
                        },
                    );

                    validated_events.push(Event::new(
                        "build.blocked",
                        "Missing backpressure evidence. Include 'tests: pass', 'lint: pass', 'typecheck: pass', 'audit: pass', 'coverage: pass', 'complexity: <score>', 'duplication: pass', 'performance: pass' (optional), 'specs: pass' (optional) in build.done payload.",
                    ));
                }
            } else if event.topic == "review.done" && !event.is_wave_event() {
                // Validate review.done events have verification evidence.
                // Wave worker events skip this — wave reviews are read-only
                // and don't run tests/builds.
                if let Some(evidence) = EventParser::parse_review_evidence(&payload) {
                    if evidence.is_verified() {
                        validated_events.push(Event::new(event.topic.as_str(), &payload));
                    } else {
                        // Evidence present but checks failed - synthesize review.blocked
                        warn!(
                            tests = evidence.tests_passed,
                            build = evidence.build_passed,
                            "review.done rejected: verification checks failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason: format!(
                                    "review verification failed: tests={}, build={}",
                                    evidence.tests_passed, evidence.build_passed
                                ),
                            },
                        );

                        validated_events.push(Event::new(
                            "review.blocked",
                            "Review verification failed. Run tests and build before emitting review.done.",
                        ));
                    }
                } else {
                    // No evidence found - synthesize review.blocked
                    warn!("review.done rejected: missing verification evidence");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing review verification evidence".to_string(),
                        },
                    );

                    validated_events.push(Event::new(
                        "review.blocked",
                        "Missing verification evidence. Include 'tests: pass' and 'build: pass' in review.done payload.",
                    ));
                }
            } else if event.topic == "verify.passed" {
                if let Some(report) = EventParser::parse_quality_report(&payload) {
                    if report.meets_thresholds() {
                        validated_events.push(Event::new(event.topic.as_str(), &payload));
                    } else {
                        let failed = report.failed_dimensions();
                        let reason = if failed.is_empty() {
                            "quality thresholds failed".to_string()
                        } else {
                            format!("quality thresholds failed: {}", failed.join(", "))
                        };

                        warn!(
                            failed_dimensions = ?failed,
                            "verify.passed rejected: quality thresholds failed"
                        );

                        self.diagnostics.log_orchestration(
                            self.state.iteration,
                            "jsonl",
                            crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                                reason,
                            },
                        );

                        validated_events.push(Event::new(
                            "verify.failed",
                            "Quality thresholds failed. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity with thresholds in verify.passed payload.",
                        ));
                    }
                } else {
                    // No quality report found - synthesize verify.failed
                    warn!("verify.passed rejected: missing quality report");

                    self.diagnostics.log_orchestration(
                        self.state.iteration,
                        "jsonl",
                        crate::diagnostics::OrchestrationEvent::BackpressureTriggered {
                            reason: "missing quality report".to_string(),
                        },
                    );

                    validated_events.push(Event::new(
                        "verify.failed",
                        "Missing quality report. Include quality.tests, quality.coverage, quality.lint, quality.audit, quality.mutation, quality.complexity in verify.passed payload.",
                    ));
                }
            } else if event.topic == "verify.failed" {
                if EventParser::parse_quality_report(&payload).is_none() {
                    warn!("verify.failed missing quality report");
                }
                validated_events.push(Event::new(event.topic.as_str(), &payload));
            } else {
                // Non-backpressure events pass through unchanged
                validated_events.push(Event::new(event.topic.as_str(), &payload));
            }
        }

        // Track build.blocked events for thrashing detection
        let blocked_events: Vec<_> = validated_events
            .iter()
            .filter(|e| e.topic == "build.blocked".into())
            .collect();

        for blocked_event in &blocked_events {
            let task_id = Self::extract_task_id(&blocked_event.payload);

            let count = self
                .state
                .task_block_counts
                .entry(task_id.clone())
                .or_insert(0);
            *count += 1;

            debug!(
                task_id = %task_id,
                block_count = *count,
                "Task blocked"
            );

            // After 3 blocks on same task, emit build.task.abandoned
            if *count >= 3 && !self.state.abandoned_tasks.contains(&task_id) {
                warn!(
                    task_id = %task_id,
                    "Task abandoned after 3 consecutive blocks"
                );

                self.state.abandoned_tasks.push(task_id.clone());

                self.diagnostics.log_orchestration(
                    self.state.iteration,
                    "jsonl",
                    crate::diagnostics::OrchestrationEvent::TaskAbandoned {
                        reason: format!(
                            "3 consecutive build.blocked events for task '{}'",
                            task_id
                        ),
                    },
                );

                let abandoned_event = Event::new(
                    "build.task.abandoned",
                    format!(
                        "Task '{}' abandoned after 3 consecutive build.blocked events",
                        task_id
                    ),
                );

                self.bus.publish(abandoned_event);
            }
        }

        // Track hat-level blocking for legacy thrashing detection
        let has_blocked_event = !blocked_events.is_empty();

        if has_blocked_event {
            self.state.consecutive_blocked += 1;
        } else {
            self.state.consecutive_blocked = 0;
            self.state.last_blocked_hat = None;
        }

        // Handle human.interact blocking behavior:
        // When a human.interact event is detected and robot service is active,
        // send the question and block until human.response or timeout.
        let mut response_event = None;
        let mut human_interact_context = None;
        let ask_human_idx = validated_events
            .iter()
            .position(|e| e.topic == "human.interact".into());

        if let Some(idx) = ask_human_idx {
            let ask_event = &validated_events[idx];
            let payload = ask_event.payload.clone();

            let mut context = match Self::parse_human_interact_context(&payload) {
                Value::Object(map) => map,
                _ => Map::new(),
            };

            if let Some(ref robot_service) = self.robot_service {
                info!(
                    payload = %payload,
                    "human.interact event detected — sending question via robot service"
                );

                // Send the question (includes retry with exponential backoff)
                let send_ok = match robot_service.send_question(&payload) {
                    Ok(_message_id) => true,
                    Err(e) => {
                        warn!(
                            error = %e,
                            "Failed to send human.interact question after retries — treating as timeout"
                        );
                        // Log to diagnostics
                        self.diagnostics.log_error(
                            self.state.iteration,
                            "telegram",
                            crate::diagnostics::DiagnosticError::TelegramSendError {
                                operation: "send_question".to_string(),
                                error: e.to_string(),
                                retry_count: 3,
                            },
                        );
                        context.insert(
                            "outcome".to_string(),
                            Value::String("send_failure".to_string()),
                        );
                        context.insert("error".to_string(), Value::String(e.to_string()));
                        false
                    }
                };

                // Block: poll events file for human.response
                // Per spec, even on send failure we treat as timeout (continue without blocking)
                if send_ok {
                    // Read the active events path from the current-events marker,
                    // falling back to the default events.jsonl if not available.
                    let events_path = self
                        .loop_context
                        .as_ref()
                        .and_then(|ctx| {
                            std::fs::read_to_string(ctx.current_events_marker())
                                .ok()
                                .map(|s| ctx.workspace().join(s.trim()))
                        })
                        .or_else(|| {
                            std::fs::read_to_string(".ralph/current-events")
                                .ok()
                                .map(|s| PathBuf::from(s.trim()))
                        })
                        .unwrap_or_else(|| {
                            self.loop_context
                                .as_ref()
                                .map(|ctx| ctx.events_path())
                                .unwrap_or_else(|| PathBuf::from(".ralph/events.jsonl"))
                        });

                    match robot_service.wait_for_response(&events_path) {
                        Ok(Some(response)) => {
                            info!(
                                response = %response,
                                "Received human.response — continuing loop"
                            );
                            context.insert(
                                "outcome".to_string(),
                                Value::String("response".to_string()),
                            );
                            context.insert("response".to_string(), Value::String(response.clone()));
                            // Create a human.response event to inject into the bus
                            response_event = Some(Event::new("human.response", &response));
                        }
                        Ok(None) => {
                            warn!(
                                timeout_secs = robot_service.timeout_secs(),
                                "Human response timeout — injecting human.timeout event"
                            );
                            context.insert(
                                "outcome".to_string(),
                                Value::String("timeout".to_string()),
                            );
                            context.insert(
                                "timeout_seconds".to_string(),
                                Value::from(robot_service.timeout_secs()),
                            );
                            let timeout_event = Event::new(
                                "human.timeout",
                                format!(
                                    "No response after {}s. Original question: {}",
                                    robot_service.timeout_secs(),
                                    payload
                                ),
                            );
                            response_event = Some(timeout_event);
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                "Error waiting for human response — injecting human.timeout event"
                            );
                            context.insert(
                                "outcome".to_string(),
                                Value::String("wait_error".to_string()),
                            );
                            context.insert("error".to_string(), Value::String(e.to_string()));
                            let timeout_event = Event::new(
                                "human.timeout",
                                format!(
                                    "Error waiting for response: {}. Original question: {}",
                                    e, payload
                                ),
                            );
                            response_event = Some(timeout_event);
                        }
                    }
                }
            } else {
                debug!(
                    "human.interact event detected but no robot service active — passing through"
                );
                context.insert(
                    "outcome".to_string(),
                    Value::String("no_robot_service".to_string()),
                );
            }

            human_interact_context = Some(Value::Object(context));
        }

        let restart_requested = validated_events.iter().any(Self::is_restart_request_event)
            || response_event
                .as_ref()
                .is_some_and(Self::is_restart_request_event);
        if restart_requested {
            self.mark_restart_requested("human_text");
        }

        // Track whether any events will be published (before the loop consumes them).
        let had_events = !validated_events.is_empty();
        let had_plan_events = validated_events
            .iter()
            .any(|event| event.topic.as_str().starts_with("plan."));

        // Publish validated events to the bus.
        // Ralph is always registered with subscribe("*"), so every event has at least
        // one subscriber. Events without a specific hat subscriber are "orphaned" —
        // Ralph handles them as the universal fallback.
        for event in validated_events {
            // Record topic for event chain validation
            self.state.record_event(&event);

            self.diagnostics.log_orchestration(
                self.state.iteration,
                "jsonl",
                crate::diagnostics::OrchestrationEvent::EventPublished {
                    topic: event.topic.to_string(),
                },
            );

            if !self.registry.has_subscriber(event.topic.as_str()) {
                has_orphans = true;
            }

            debug!(
                topic = %event.topic,
                "Publishing event from JSONL"
            );
            self.bus.publish(event);
        }

        // Publish human.response event if one was received during blocking
        if let Some(response) = response_event {
            self.state.record_event(&response);
            info!(
                topic = %response.topic,
                "Publishing human.response event from robot service"
            );
            self.bus.publish(response);
        }

        Ok(ProcessedEvents {
            had_events,
            had_plan_events,
            human_interact_context,
            has_orphans,
        })
    }

    /// Process events from JSONL, partitioning wave events from regular events.
    ///
    /// Wave events (those with `wave_id` set and targeting a concurrent hat) are
    /// extracted and returned separately. Regular events go through the full
    /// backpressure pipeline via `process_parse_result`.
    pub fn process_events_from_jsonl_with_waves(
        &mut self,
    ) -> std::io::Result<ProcessedEventsWithWaves> {
        let result = self.event_reader.read_new_events()?;

        // Partition: wave dispatch events vs regular events.
        // Only events that target a concurrent hat (concurrency > 1) are wave dispatches.
        // Wave *results* (e.g. review.done) have wave_id set but should be treated as
        // regular events so they reach the bus and trigger downstream hats (e.g. aggregator).
        //
        // Uses find_by_trigger + get_config — the same resolution path as
        // detect_wave_events — to ensure partition and detection agree.
        let (wave_events, regular_events): (Vec<_>, Vec<_>) =
            result.events.into_iter().partition(|e| {
                e.wave_id.is_some()
                    && self
                        .registry
                        .find_by_trigger(e.topic.as_str())
                        .and_then(|hat_id| self.registry.get_config(hat_id))
                        .is_some_and(|hat_config| hat_config.concurrency > 1)
            });

        if !wave_events.is_empty() {
            debug!(
                wave_count = wave_events.len(),
                regular_count = regular_events.len(),
                "Partitioned wave events from regular events"
            );
        }

        // Delegate regular events to the full pipeline (backpressure, scope
        // enforcement, human.interact, plan detection, etc.)
        let regular_result = crate::event_reader::ParseResult {
            events: regular_events,
            malformed: result.malformed,
        };
        let processed = self.process_parse_result(regular_result)?;

        Ok(ProcessedEventsWithWaves {
            processed,
            wave_events,
        })
    }
}
