//! Event parsing and promise detection.
//!
//! This module owns [`EventParser`], which extracts `<event>` tags from
//! agent output, parses structured evidence payloads, and determines
//! whether a completion promise was emitted outside of an event tag.

use ralph_proto::{Event, HatId};

use super::ansi::strip_ansi;
use super::evidence::{BackpressureEvidence, QualityReport, ReviewEvidence};
use super::extraction::{
    extract_attr, extract_first_number, extract_percentage, parse_complexity_evidence,
    parse_duplication_evidence, parse_mutation_evidence, parse_performance_regression,
    parse_quality_pass_fail, parse_specs_evidence,
};

/// Parser for extracting events from CLI output.
#[derive(Debug, Default)]
pub struct EventParser {
    /// The source hat ID to attach to parsed events.
    source: Option<HatId>,
}

impl EventParser {
    /// Creates a new event parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the source hat for parsed events.
    pub fn with_source(mut self, source: impl Into<HatId>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Parses events from CLI output text.
    ///
    /// Returns a list of parsed events.
    pub fn parse(&self, output: &str) -> Vec<Event> {
        let mut events = Vec::new();
        let mut remaining = output;

        while let Some(start_idx) = remaining.find("<event ") {
            let after_start = &remaining[start_idx..];

            // Find the end of the opening tag
            let Some(tag_end) = after_start.find('>') else {
                remaining = &remaining[start_idx + 7..];
                continue;
            };

            let opening_tag = &after_start[..tag_end + 1];

            // Parse attributes from opening tag
            let topic = extract_attr(opening_tag, "topic");
            let target = extract_attr(opening_tag, "target");

            let Some(topic) = topic else {
                remaining = &remaining[start_idx + tag_end + 1..];
                continue;
            };

            // Find the closing tag
            let content_start = &after_start[tag_end + 1..];
            let Some(close_idx) = content_start.find("</event>") else {
                remaining = &remaining[start_idx + tag_end + 1..];
                continue;
            };

            let payload = content_start[..close_idx].trim().to_string();

            let mut event = Event::new(topic, payload);

            if let Some(source) = &self.source {
                event = event.with_source(source.clone());
            }

            if let Some(target) = target {
                event = event.with_target(target);
            }

            events.push(event);

            // Move past this event
            let total_consumed = start_idx + tag_end + 1 + close_idx + 8; // 8 = "</event>".len()
            remaining = &remaining[total_consumed..];
        }

        events
    }

    /// Parses backpressure evidence from `build.done` event payload.
    ///
    /// Expected format:
    /// ```text
    /// tests: pass
    /// lint: pass
    /// typecheck: pass
    /// audit: pass
    /// coverage: pass
    /// complexity: 7           # required (<=10)
    /// duplication: pass       # required
    /// performance: pass       # optional (regression blocks)
    /// mutants: pass (82%)     # optional, warning-only
    /// specs: pass             # optional (fail blocks)
    /// ```
    ///
    /// Note: ANSI escape codes are stripped before parsing to handle
    /// colorized CLI output.
    pub fn parse_backpressure_evidence(payload: &str) -> Option<BackpressureEvidence> {
        // Strip ANSI codes before checking for evidence strings
        let clean_payload = strip_ansi(payload);

        let tests_passed = clean_payload.contains("tests: pass");
        let lint_passed = clean_payload.contains("lint: pass");
        let typecheck_passed = clean_payload.contains("typecheck: pass");
        let audit_passed = clean_payload.contains("audit: pass");
        let coverage_passed = clean_payload.contains("coverage: pass");
        let complexity_score = parse_complexity_evidence(&clean_payload);
        let duplication_passed = parse_duplication_evidence(&clean_payload).unwrap_or(false);
        let performance_regression = parse_performance_regression(&clean_payload);
        let mutants = parse_mutation_evidence(&clean_payload);
        let specs_verified = parse_specs_evidence(&clean_payload);

        // Only return evidence if at least one check is mentioned
        if clean_payload.contains("tests:")
            || clean_payload.contains("lint:")
            || clean_payload.contains("typecheck:")
            || clean_payload.contains("audit:")
            || clean_payload.contains("coverage:")
            || clean_payload.contains("complexity:")
            || clean_payload.contains("duplication:")
            || clean_payload.contains("performance:")
            || clean_payload.contains("perf:")
            || clean_payload.contains("mutants:")
            || clean_payload.contains("specs:")
        {
            Some(BackpressureEvidence {
                tests_passed,
                lint_passed,
                typecheck_passed,
                audit_passed,
                coverage_passed,
                complexity_score,
                duplication_passed,
                performance_regression,
                mutants,
                specs_verified,
            })
        } else {
            None
        }
    }

    /// Parses review evidence from `review.done` event payload.
    ///
    /// Expected format (subset of backpressure evidence):
    /// ```text
    /// tests: pass
    /// build: pass
    /// ```
    ///
    /// Note: ANSI escape codes are stripped before parsing.
    pub fn parse_review_evidence(payload: &str) -> Option<ReviewEvidence> {
        let clean_payload = strip_ansi(payload);

        let tests_passed = clean_payload.contains("tests: pass");
        let build_passed = clean_payload.contains("build: pass");

        // Only return evidence if at least one check is mentioned
        if clean_payload.contains("tests:") || clean_payload.contains("build:") {
            Some(ReviewEvidence {
                tests_passed,
                build_passed,
            })
        } else {
            None
        }
    }

    /// Parses quality report evidence from `verify.*` event payloads.
    ///
    /// Expected format:
    /// ```text
    /// quality.tests: pass
    /// quality.coverage: 82%
    /// quality.lint: pass
    /// quality.audit: pass
    /// quality.mutation: 71%
    /// quality.complexity: 7
    /// quality.specs: pass         # optional (fail blocks)
    /// ```
    ///
    /// Note: ANSI escape codes are stripped before parsing.
    pub fn parse_quality_report(payload: &str) -> Option<QualityReport> {
        let clean_payload = strip_ansi(payload);
        let mut report = QualityReport {
            tests_passed: None,
            lint_passed: None,
            audit_passed: None,
            coverage_percent: None,
            mutation_percent: None,
            complexity_score: None,
            specs_verified: None,
        };
        let mut seen = false;

        for segment in clean_payload
            .split(|c| c == '\n' || c == ',')
            .map(str::trim)
        {
            if segment.is_empty() {
                continue;
            }
            let normalized = segment.to_lowercase();

            if normalized.starts_with("quality.tests:") {
                report.tests_passed = parse_quality_pass_fail(&normalized);
                seen = true;
            } else if normalized.starts_with("quality.lint:") {
                report.lint_passed = parse_quality_pass_fail(&normalized);
                seen = true;
            } else if normalized.starts_with("quality.audit:") {
                report.audit_passed = parse_quality_pass_fail(&normalized);
                seen = true;
            } else if normalized.starts_with("quality.coverage:") {
                report.coverage_percent =
                    extract_percentage(segment).or_else(|| extract_first_number(segment));
                seen = true;
            } else if normalized.starts_with("quality.mutation:") {
                report.mutation_percent =
                    extract_percentage(segment).or_else(|| extract_first_number(segment));
                seen = true;
            } else if normalized.starts_with("quality.complexity:") {
                report.complexity_score = extract_first_number(segment);
                seen = true;
            } else if normalized.starts_with("quality.specs:") {
                report.specs_verified = parse_quality_pass_fail(&normalized);
                seen = true;
            }
        }

        if seen { Some(report) } else { None }
    }

    /// Checks if output contains the completion promise.
    ///
    /// Per spec: The promise must appear in the agent's final output,
    /// not inside an `<event>` tag payload. This function:
    /// 1. Returns false if the promise appears inside ANY event tag
    ///    (prevents accidental completion when agents discuss the promise)
    /// 2. Otherwise, checks that the promise is the final non-empty line
    ///    in the stripped output (prevents prompt echo false positives)
    pub fn contains_promise(output: &str, promise: &str) -> bool {
        let promise = promise.trim();
        if promise.is_empty() {
            return false;
        }

        // Safety check: if promise appears inside any event tag, never complete
        if Self::promise_in_event_tags(output, promise) {
            return false;
        }
        let stripped = strip_event_tags(output);

        for line in stripped.lines().rev() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return trimmed == promise;
        }

        false
    }

    /// Checks if the promise appears inside any event tag payload.
    pub fn promise_in_event_tags(output: &str, promise: &str) -> bool {
        let mut remaining = output;

        while let Some(start_idx) = remaining.find("<event ") {
            let after_start = &remaining[start_idx..];

            // Find the end of the opening tag
            let Some(tag_end) = after_start.find('>') else {
                remaining = &remaining[start_idx + 7..];
                continue;
            };

            // Find the closing tag
            let content_start = &after_start[tag_end + 1..];
            let Some(close_idx) = content_start.find("</event>") else {
                remaining = &remaining[start_idx + tag_end + 1..];
                continue;
            };

            let payload = &content_start[..close_idx];
            if payload.contains(promise) {
                return true;
            }

            // Move past this event
            let total_consumed = start_idx + tag_end + 1 + close_idx + 8;
            remaining = &remaining[total_consumed..];
        }

        false
    }
}

/// Strips all `<event ...>...</event>` blocks from output.
///
/// Returns the output with event tags removed, leaving only the "final
/// output" text that should be checked for promises.
pub(super) fn strip_event_tags(output: &str) -> String {
    let mut result = String::with_capacity(output.len());
    let mut remaining = output;

    while let Some(start_idx) = remaining.find("<event ") {
        // Add everything before this event tag
        result.push_str(&remaining[..start_idx]);

        let after_start = &remaining[start_idx..];

        // Find the closing tag
        if let Some(close_idx) = after_start.find("</event>") {
            // Skip past the entire event block
            remaining = &after_start[close_idx + 8..]; // 8 = "</event>".len()
        } else {
            // Malformed: no closing tag, keep the rest and stop
            result.push_str(after_start);
            remaining = "";
            break;
        }
    }

    // Add any remaining content after the last event
    result.push_str(remaining);
    result
}
