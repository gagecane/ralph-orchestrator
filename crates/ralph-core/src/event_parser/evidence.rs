//! Evidence data types produced by parsing event payloads.
//!
//! These structs represent the backpressure gates the orchestrator uses
//! to decide whether an agent's completion claim is trustworthy. The
//! parser lives in [`super::parser`]; this module only holds the data
//! types and their validation logic.

/// Evidence of backpressure checks for `build.done` events.
#[derive(Debug, Clone, PartialEq)]
pub struct BackpressureEvidence {
    pub tests_passed: bool,
    pub lint_passed: bool,
    pub typecheck_passed: bool,
    pub audit_passed: bool,
    pub coverage_passed: bool,
    pub complexity_score: Option<f64>,
    pub duplication_passed: bool,
    pub performance_regression: Option<bool>,
    pub mutants: Option<MutationEvidence>,
    /// Whether spec acceptance criteria have been verified against passing tests.
    ///
    /// `None` means specs evidence was not included in the payload (optional gate).
    /// `Some(true)` means all spec criteria are satisfied.
    /// `Some(false)` means some spec criteria are unsatisfied — blocks `build.done`.
    pub specs_verified: Option<bool>,
}

impl BackpressureEvidence {
    /// Returns true if all required checks passed.
    ///
    /// Mutation testing evidence is warning-only and does not affect this result.
    /// Spec verification blocks when explicitly reported as failed (`Some(false)`),
    /// but is optional — omitting it (`None`) does not block.
    pub fn all_passed(&self) -> bool {
        self.tests_passed
            && self.lint_passed
            && self.typecheck_passed
            && self.audit_passed
            && self.coverage_passed
            && self
                .complexity_score
                .is_some_and(|value| value <= QualityReport::COMPLEXITY_THRESHOLD)
            && self.duplication_passed
            && !matches!(self.performance_regression, Some(true))
            && !matches!(self.specs_verified, Some(false))
    }
}

/// Status of mutation testing evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
}

/// Evidence of mutation testing for `build.done` payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct MutationEvidence {
    pub status: MutationStatus,
    pub score_percent: Option<f64>,
}

/// Evidence of verification for `review.done` events.
///
/// Enforces that review hats actually ran verification commands rather
/// than just asserting "looks good". At minimum, tests must have been run.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewEvidence {
    pub tests_passed: bool,
    pub build_passed: bool,
}

impl ReviewEvidence {
    /// Returns true if the review has sufficient verification.
    ///
    /// Both tests and build must pass to constitute a verified review.
    pub fn is_verified(&self) -> bool {
        self.tests_passed && self.build_passed
    }
}

/// Structured quality report for verifier events.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityReport {
    pub tests_passed: Option<bool>,
    pub lint_passed: Option<bool>,
    pub audit_passed: Option<bool>,
    pub coverage_percent: Option<f64>,
    pub mutation_percent: Option<f64>,
    pub complexity_score: Option<f64>,
    /// Whether spec acceptance criteria are satisfied by passing tests.
    ///
    /// `None` means not reported (optional — does not fail thresholds).
    /// `Some(false)` means spec criteria are unsatisfied — fails thresholds.
    pub specs_verified: Option<bool>,
}

impl QualityReport {
    pub const COVERAGE_THRESHOLD: f64 = 80.0;
    pub const MUTATION_THRESHOLD: f64 = 70.0;
    pub const COMPLEXITY_THRESHOLD: f64 = 10.0;

    pub fn meets_thresholds(&self) -> bool {
        self.tests_passed == Some(true)
            && self.lint_passed == Some(true)
            && self.audit_passed == Some(true)
            && self
                .coverage_percent
                .is_some_and(|value| value >= Self::COVERAGE_THRESHOLD)
            && self
                .mutation_percent
                .is_some_and(|value| value >= Self::MUTATION_THRESHOLD)
            && self
                .complexity_score
                .is_some_and(|value| value <= Self::COMPLEXITY_THRESHOLD)
            && !matches!(self.specs_verified, Some(false))
    }

    pub fn failed_dimensions(&self) -> Vec<&'static str> {
        let mut failed = Vec::new();

        if self.tests_passed != Some(true) {
            failed.push("tests");
        }
        if self.lint_passed != Some(true) {
            failed.push("lint");
        }
        if self.audit_passed != Some(true) {
            failed.push("audit");
        }
        if self
            .coverage_percent
            .is_none_or(|value| value < Self::COVERAGE_THRESHOLD)
        {
            failed.push("coverage");
        }
        if self
            .mutation_percent
            .is_none_or(|value| value < Self::MUTATION_THRESHOLD)
        {
            failed.push("mutation");
        }
        if self
            .complexity_score
            .is_none_or(|value| value > Self::COMPLEXITY_THRESHOLD)
        {
            failed.push("complexity");
        }
        if matches!(self.specs_verified, Some(false)) {
            failed.push("specs");
        }

        failed
    }
}
