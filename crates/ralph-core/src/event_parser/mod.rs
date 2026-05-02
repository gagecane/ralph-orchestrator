//! Event parsing from CLI output.
//!
//! Parses XML-style event tags from agent output:
//! ```text
//! <event topic="impl.done">payload</event>
//! <event topic="handoff" target="reviewer">payload</event>
//! ```
//!
//! Submodule layout:
//! - [`ansi`] — strip ANSI escape sequences from colorized CLI output.
//! - [`evidence`] — data types ([`BackpressureEvidence`], [`ReviewEvidence`],
//!   [`QualityReport`], [`MutationEvidence`], [`MutationStatus`]) and their
//!   validation logic.
//! - [`extraction`] — low-level text extraction helpers shared across parsers.
//! - [`parser`] — [`EventParser`], the public entry point.

mod ansi;
mod evidence;
mod extraction;
mod parser;

pub use evidence::{BackpressureEvidence, MutationEvidence, MutationStatus};
// Re-exported for API completeness; currently only referenced via their
// parser methods (`parse_quality_report`, `parse_review_evidence`) inside
// this module, which is why we silence the unused-imports warning.
#[allow(unused_imports)]
pub use evidence::{QualityReport, ReviewEvidence};
pub use parser::EventParser;

#[cfg(test)]
mod tests {
    use super::*;
    use super::extraction::extract_first_number;
    use super::parser::strip_event_tags;

    #[test]
    fn test_parse_single_event() {
        let output = r#"
Some preamble text.
<event topic="impl.done">
Implemented the authentication module.
</event>
Some trailing text.
"#;
        let parser = EventParser::new();
        let events = parser.parse(output);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].topic.as_str(), "impl.done");
        assert!(events[0].payload.contains("authentication module"));
    }

    #[test]
    fn test_parse_event_with_target() {
        let output = r#"<event topic="handoff" target="reviewer">Please review</event>"#;
        let parser = EventParser::new();
        let events = parser.parse(output);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].target.as_ref().unwrap().as_str(), "reviewer");
    }

    #[test]
    fn test_parse_multiple_events() {
        let output = r#"
<event topic="impl.started">Starting work</event>
Working on implementation...
<event topic="impl.done">Finished</event>
"#;
        let parser = EventParser::new();
        let events = parser.parse(output);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].topic.as_str(), "impl.started");
        assert_eq!(events[1].topic.as_str(), "impl.done");
    }

    #[test]
    fn test_parse_with_source() {
        let output = r#"<event topic="impl.done">Done</event>"#;
        let parser = EventParser::new().with_source("implementer");
        let events = parser.parse(output);

        assert_eq!(events[0].source.as_ref().unwrap().as_str(), "implementer");
    }

    #[test]
    fn test_no_events() {
        let output = "Just regular output with no events.";
        let parser = EventParser::new();
        let events = parser.parse(output);

        assert!(events.is_empty());
    }

    #[test]
    fn test_contains_promise_requires_last_line() {
        assert!(EventParser::contains_promise(
            "LOOP_COMPLETE",
            "LOOP_COMPLETE"
        ));
        assert!(EventParser::contains_promise(
            "All done!\nLOOP_COMPLETE",
            "LOOP_COMPLETE"
        ));
        assert!(EventParser::contains_promise(
            "LOOP_COMPLETE   \n\n",
            "LOOP_COMPLETE"
        ));
        assert!(!EventParser::contains_promise(
            "prefix LOOP_COMPLETE suffix",
            "LOOP_COMPLETE"
        ));
        assert!(!EventParser::contains_promise(
            "LOOP_COMPLETE\nMore text",
            "LOOP_COMPLETE"
        ));
        assert!(!EventParser::contains_promise("Any output", "   "));
        assert!(!EventParser::contains_promise(
            "No promise here",
            "LOOP_COMPLETE"
        ));
    }

    #[test]
    fn test_contains_promise_ignores_event_payloads() {
        // Promise inside event payload should NOT be detected
        let output = r#"<event topic="build.task">Fix LOOP_COMPLETE detection</event>"#;
        assert!(!EventParser::contains_promise(output, "LOOP_COMPLETE"));

        // Promise inside event with acceptance criteria mentioning LOOP_COMPLETE
        let output = r#"<event topic="build.task">
## Task: Fix completion promise detection
- Given LOOP_COMPLETE appears inside an event tag
- Then it should be ignored
</event>"#;
        assert!(!EventParser::contains_promise(output, "LOOP_COMPLETE"));
    }

    #[test]
    fn test_contains_promise_detects_outside_events() {
        // Promise outside event tags should be detected
        let output = r#"<event topic="build.done">Task complete</event>
All done!
LOOP_COMPLETE"#;
        assert!(EventParser::contains_promise(output, "LOOP_COMPLETE"));

        // Promise before event tags
        let output = r#"LOOP_COMPLETE
<event topic="summary">Final summary</event>"#;
        assert!(EventParser::contains_promise(output, "LOOP_COMPLETE"));
    }

    #[test]
    fn test_contains_promise_mixed_content() {
        // Promise only in event payload, not in surrounding text
        let output = r#"Working on task...
<event topic="build.task">Fix LOOP_COMPLETE bug</event>
Still working..."#;
        assert!(!EventParser::contains_promise(output, "LOOP_COMPLETE"));

        // Promise in both event and surrounding text - should NOT complete
        // because promise appears inside an event tag (safety mechanism)
        let output = r#"All tasks done. LOOP_COMPLETE
<event topic="summary">Completed LOOP_COMPLETE task</event>"#;
        assert!(!EventParser::contains_promise(output, "LOOP_COMPLETE"));
    }

    #[test]
    fn test_promise_in_event_tags() {
        // Promise inside event payload
        let output = r#"<event topic="build.task">Fix LOOP_COMPLETE bug</event>"#;
        assert!(EventParser::promise_in_event_tags(output, "LOOP_COMPLETE"));

        // Promise not in any event
        let output = r#"<event topic="build.done">Task complete</event>"#;
        assert!(!EventParser::promise_in_event_tags(output, "LOOP_COMPLETE"));

        // No events at all
        let output = "Just regular text with LOOP_COMPLETE";
        assert!(!EventParser::promise_in_event_tags(output, "LOOP_COMPLETE"));

        // Multiple events, promise in second
        let output = r#"<event topic="a">first</event>
<event topic="b">contains LOOP_COMPLETE</event>"#;
        assert!(EventParser::promise_in_event_tags(output, "LOOP_COMPLETE"));
    }

    #[test]
    fn test_strip_event_tags() {
        // Single event
        let output = r#"before <event topic="test">payload</event> after"#;
        let stripped = strip_event_tags(output);
        assert_eq!(stripped, "before  after");
        assert!(!stripped.contains("payload"));

        // Multiple events
        let output =
            r#"start <event topic="a">one</event> middle <event topic="b">two</event> end"#;
        let stripped = strip_event_tags(output);
        assert_eq!(stripped, "start  middle  end");

        // No events
        let output = "just plain text";
        let stripped = strip_event_tags(output);
        assert_eq!(stripped, "just plain text");
    }

    #[test]
    fn test_parse_backpressure_evidence_all_pass() {
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(evidence.lint_passed);
        assert!(evidence.typecheck_passed);
        assert!(evidence.audit_passed);
        assert!(evidence.coverage_passed);
        assert_eq!(evidence.complexity_score, Some(7.0));
        assert!(evidence.duplication_passed);
        assert_eq!(evidence.performance_regression, Some(false));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_some_fail() {
        let payload = "tests: pass\nlint: fail\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(!evidence.lint_passed);
        assert!(evidence.typecheck_passed);
        assert!(evidence.audit_passed);
        assert!(evidence.coverage_passed);
        assert_eq!(evidence.complexity_score, Some(7.0));
        assert!(evidence.duplication_passed);
        assert_eq!(evidence.performance_regression, Some(false));
        assert!(!evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_missing() {
        let payload = "Task completed successfully";
        let evidence = EventParser::parse_backpressure_evidence(payload);
        assert!(evidence.is_none());
    }

    #[test]
    fn test_parse_backpressure_evidence_partial() {
        let payload = "tests: pass\nSome other text";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(!evidence.lint_passed);
        assert!(!evidence.typecheck_passed);
        assert!(!evidence.audit_passed);
        assert!(!evidence.coverage_passed);
        assert!(evidence.complexity_score.is_none());
        assert!(!evidence.duplication_passed);
        assert!(evidence.performance_regression.is_none());
        assert!(!evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_with_ansi_codes() {
        let payload = "\x1b[0mtests: pass\x1b[0m\n\x1b[32mlint: pass\x1b[0m\ntypecheck: pass\n\x1b[34maudit: pass\x1b[0m\n\x1b[35mcoverage: pass\x1b[0m\n\x1b[36mcomplexity: 7\x1b[0m\n\x1b[31mduplication: pass\x1b[0m\n\x1b[33mperformance: pass\x1b[0m";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(evidence.lint_passed);
        assert!(evidence.typecheck_passed);
        assert!(evidence.audit_passed);
        assert!(evidence.coverage_passed);
        assert_eq!(evidence.complexity_score, Some(7.0));
        assert!(evidence.duplication_passed);
        assert_eq!(evidence.performance_regression, Some(false));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_with_mutants_pass() {
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass\nmutants: pass (82%)";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        let mutants = evidence
            .mutants
            .as_ref()
            .expect("mutants evidence should parse");
        assert_eq!(mutants.status, MutationStatus::Pass);
        assert_eq!(mutants.score_percent, Some(82.0));
        assert_eq!(evidence.performance_regression, Some(false));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_with_mutants_warn() {
        let payload = "tests: pass, lint: pass, typecheck: pass, audit: pass, coverage: pass, complexity: 7, duplication: pass, performance: pass, mutants: warn (65%)";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        let mutants = evidence
            .mutants
            .as_ref()
            .expect("mutants evidence should parse");
        assert_eq!(mutants.status, MutationStatus::Warn);
        assert_eq!(mutants.score_percent, Some(65.0));
        assert_eq!(evidence.performance_regression, Some(false));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_with_performance_regression() {
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: regression";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.performance_regression, Some(true));
        assert!(!evidence.all_passed());
    }

    #[test]
    fn test_parse_review_evidence_all_pass() {
        let payload = "tests: pass\nbuild: pass";
        let evidence = EventParser::parse_review_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(evidence.build_passed);
        assert!(evidence.is_verified());
    }

    #[test]
    fn test_parse_review_evidence_tests_fail() {
        let payload = "tests: fail\nbuild: pass";
        let evidence = EventParser::parse_review_evidence(payload).unwrap();
        assert!(!evidence.tests_passed);
        assert!(evidence.build_passed);
        assert!(!evidence.is_verified());
    }

    #[test]
    fn test_parse_review_evidence_build_fail() {
        let payload = "tests: pass\nbuild: fail";
        let evidence = EventParser::parse_review_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(!evidence.build_passed);
        assert!(!evidence.is_verified());
    }

    #[test]
    fn test_parse_review_evidence_missing() {
        let payload = "Looks good, approved!";
        let evidence = EventParser::parse_review_evidence(payload);
        assert!(evidence.is_none());
    }

    #[test]
    fn test_parse_review_evidence_partial() {
        let payload = "tests: pass\nLGTM";
        let evidence = EventParser::parse_review_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(!evidence.build_passed);
        assert!(!evidence.is_verified());
    }

    #[test]
    fn test_parse_review_evidence_with_ansi_codes() {
        let payload = "\x1b[32mtests: pass\x1b[0m\n\x1b[32mbuild: pass\x1b[0m";
        let evidence = EventParser::parse_review_evidence(payload).unwrap();
        assert!(evidence.tests_passed);
        assert!(evidence.build_passed);
        assert!(evidence.is_verified());
    }

    #[test]
    fn test_parse_quality_report_passes_thresholds() {
        let payload = "quality.tests: pass\nquality.coverage: 82% (>=80%)\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 71% (>=70%)\nquality.complexity: 7 (<=10)";
        let report = EventParser::parse_quality_report(payload).unwrap();
        assert_eq!(report.tests_passed, Some(true));
        assert_eq!(report.lint_passed, Some(true));
        assert_eq!(report.audit_passed, Some(true));
        assert_eq!(report.coverage_percent, Some(82.0));
        assert_eq!(report.mutation_percent, Some(71.0));
        assert_eq!(report.complexity_score, Some(7.0));
        assert!(report.meets_thresholds());
    }

    #[test]
    fn test_parse_quality_report_fails_thresholds() {
        let payload = "quality.tests: pass\nquality.coverage: 60%\nquality.lint: fail\nquality.audit: pass\nquality.mutation: 50%\nquality.complexity: 12";
        let report = EventParser::parse_quality_report(payload).unwrap();
        assert!(!report.meets_thresholds());
    }

    #[test]
    fn test_parse_quality_report_missing() {
        let payload = "Looks good, approved!";
        let report = EventParser::parse_quality_report(payload);
        assert!(report.is_none());
    }

    #[test]
    fn test_extract_first_number_quality_line() {
        let value = extract_first_number("quality.complexity: 7 (<=10)");
        assert_eq!(value, Some(7.0));
    }

    #[test]
    fn test_parse_backpressure_evidence_with_specs_pass() {
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass\nspecs: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.specs_verified, Some(true));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_backpressure_evidence_with_specs_fail() {
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass\nspecs: fail";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.specs_verified, Some(false));
        assert!(
            !evidence.all_passed(),
            "specs: fail should block build.done"
        );
    }

    #[test]
    fn test_parse_backpressure_evidence_specs_omitted_does_not_block() {
        // When specs evidence is not included, it should not block
        let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.specs_verified, None);
        assert!(
            evidence.all_passed(),
            "missing specs should not block build.done"
        );
    }

    #[test]
    fn test_parse_backpressure_evidence_specs_comma_separated() {
        let payload = "tests: pass, lint: pass, typecheck: pass, audit: pass, coverage: pass, complexity: 7, duplication: pass, performance: pass, specs: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.specs_verified, Some(true));
        assert!(evidence.all_passed());
    }

    #[test]
    fn test_parse_specs_evidence_only() {
        // specs: alone should be recognized as evidence
        let payload = "specs: pass";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert_eq!(evidence.specs_verified, Some(true));
    }

    #[test]
    fn test_quality_report_with_specs_pass() {
        let payload = "quality.tests: pass\nquality.coverage: 82%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 71%\nquality.complexity: 7\nquality.specs: pass";
        let report = EventParser::parse_quality_report(payload).unwrap();
        assert_eq!(report.specs_verified, Some(true));
        assert!(report.meets_thresholds());
    }

    #[test]
    fn test_quality_report_with_specs_fail() {
        let payload = "quality.tests: pass\nquality.coverage: 82%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 71%\nquality.complexity: 7\nquality.specs: fail";
        let report = EventParser::parse_quality_report(payload).unwrap();
        assert_eq!(report.specs_verified, Some(false));
        assert!(
            !report.meets_thresholds(),
            "specs: fail should fail quality thresholds"
        );
        assert!(report.failed_dimensions().contains(&"specs"));
    }

    #[test]
    fn test_quality_report_specs_omitted_passes() {
        let payload = "quality.tests: pass\nquality.coverage: 82%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 71%\nquality.complexity: 7";
        let report = EventParser::parse_quality_report(payload).unwrap();
        assert_eq!(report.specs_verified, None);
        assert!(
            report.meets_thresholds(),
            "missing specs should not fail quality thresholds"
        );
        assert!(!report.failed_dimensions().contains(&"specs"));
    }

    #[test]
    fn test_strip_ansi_function() {
        // Test the internal strip_ansi function via parse_backpressure_evidence
        // Simple CSI reset sequence
        let payload = "\x1b[0mtests: pass\x1b[0m";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);

        // Bold green text
        let payload = "\x1b[1m\x1b[32mtests: pass\x1b[0m";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(evidence.tests_passed);

        // Multiple sequences mixed with content
        let payload = "\x1b[31mtests: fail\x1b[0m\n\x1b[32mlint: pass\x1b[0m";
        let evidence = EventParser::parse_backpressure_evidence(payload).unwrap();
        assert!(!evidence.tests_passed); // "tests: fail" not "tests: pass"
        assert!(evidence.lint_passed);
        assert!(!evidence.coverage_passed);
    }
}
