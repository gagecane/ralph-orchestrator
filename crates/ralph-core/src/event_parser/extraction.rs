//! Low-level text extraction helpers shared across parsers.
//!
//! These helpers pull numeric values and pass/fail signals out of the
//! free-form strings that agents emit. They are intentionally forgiving:
//! the evidence DSL is a convention, not a grammar.

use super::evidence::{MutationEvidence, MutationStatus};

/// Extracts an attribute value from an XML-like opening tag.
///
/// Returns `None` if the attribute is not present or is unterminated.
pub(super) fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let pattern = format!("{attr}=\"");
    let start = tag.find(&pattern)?;
    let value_start = start + pattern.len();
    let rest = &tag[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extracts the first percentage value from a segment (e.g. `82` from
/// `mutants: pass (82%)`).
pub(super) fn extract_percentage(segment: &str) -> Option<f64> {
    let percent_idx = segment.find('%')?;
    let bytes = segment.as_bytes();
    let mut start = percent_idx;

    while start > 0 {
        let prev = bytes[start - 1];
        if prev.is_ascii_digit() || prev == b'.' {
            start -= 1;
        } else {
            break;
        }
    }

    if start == percent_idx {
        return None;
    }

    segment[start..percent_idx].trim().parse::<f64>().ok()
}

/// Extracts the first decimal number from a segment (e.g. `7` from
/// `complexity: 7 (<=10)`).
pub(super) fn extract_first_number(segment: &str) -> Option<f64> {
    let bytes = segment.as_bytes();
    let mut start = None;
    let mut end = None;

    for (idx, &byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() {
            if start.is_none() {
                start = Some(idx);
            }
            end = Some(idx + 1);
        } else if byte == b'.' && start.is_some() {
            end = Some(idx + 1);
        } else if start.is_some() {
            break;
        }
    }

    let start = start?;
    let end = end?;
    segment[start..end].trim().parse::<f64>().ok()
}

/// Parses a `pass`/`fail` token out of a segment.
pub(super) fn parse_quality_pass_fail(segment: &str) -> Option<bool> {
    if segment.contains("pass") {
        Some(true)
    } else if segment.contains("fail") {
        Some(false)
    } else {
        None
    }
}

/// Parses mutation testing evidence from a clean (ANSI-stripped) payload.
pub(super) fn parse_mutation_evidence(clean_payload: &str) -> Option<MutationEvidence> {
    let segment = clean_payload
        .split(|c| c == '\n' || c == ',')
        .map(str::trim)
        .find(|segment| segment.contains("mutants:"))?;

    let normalized = segment.to_lowercase();
    let status = if normalized.contains("mutants: pass") {
        MutationStatus::Pass
    } else if normalized.contains("mutants: warn") {
        MutationStatus::Warn
    } else if normalized.contains("mutants: fail") {
        MutationStatus::Fail
    } else {
        MutationStatus::Unknown
    };

    let score_percent = extract_percentage(segment);

    Some(MutationEvidence {
        status,
        score_percent,
    })
}

/// Parses `complexity: <number>` out of a clean payload.
pub(super) fn parse_complexity_evidence(clean_payload: &str) -> Option<f64> {
    let segment = clean_payload
        .split(|c| c == '\n' || c == ',')
        .map(str::trim)
        .find(|segment| segment.to_lowercase().starts_with("complexity:"))?;

    extract_first_number(segment)
}

/// Parses `duplication: pass|fail` out of a clean payload.
pub(super) fn parse_duplication_evidence(clean_payload: &str) -> Option<bool> {
    let segment = clean_payload
        .split(|c| c == '\n' || c == ',')
        .map(str::trim)
        .find(|segment| segment.to_lowercase().starts_with("duplication:"))?;

    let normalized = segment.to_lowercase();
    if normalized.contains("duplication: pass") {
        Some(true)
    } else if normalized.contains("duplication: fail") {
        Some(false)
    } else {
        None
    }
}

/// Parses `performance:` / `perf:` segments, returning `Some(true)` if a
/// regression is reported.
pub(super) fn parse_performance_regression(clean_payload: &str) -> Option<bool> {
    let segment = clean_payload
        .split(|c| c == '\n' || c == ',')
        .map(str::trim)
        .find(|segment| {
            let normalized = segment.to_lowercase();
            normalized.starts_with("performance:") || normalized.starts_with("perf:")
        })?;

    let normalized = segment.to_lowercase();
    if normalized.contains("regression") || normalized.contains("fail") {
        Some(true)
    } else if normalized.contains("pass")
        || normalized.contains("ok")
        || normalized.contains("improved")
    {
        Some(false)
    } else {
        None
    }
}

/// Parses spec acceptance criteria verification evidence.
///
/// Returns `Some(true)` for `specs: pass`, `Some(false)` for `specs: fail`,
/// and `None` if no specs evidence is present.
pub(super) fn parse_specs_evidence(clean_payload: &str) -> Option<bool> {
    let segment = clean_payload
        .split(|c| c == '\n' || c == ',')
        .map(str::trim)
        .find(|segment| segment.to_lowercase().starts_with("specs:"))?;

    let normalized = segment.to_lowercase();
    if normalized.contains("specs: pass") {
        Some(true)
    } else if normalized.contains("specs: fail") {
        Some(false)
    } else {
        None
    }
}
