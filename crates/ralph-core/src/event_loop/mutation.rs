//! File-modification auditing and mutation-testing backpressure helpers.

use super::EventLoop;
use crate::event_parser::{MutationEvidence, MutationStatus};
use ralph_proto::{Event, HatId};
use tracing::{debug, warn};

impl EventLoop {
    /// Audits file modifications when a hat has Edit/Write restrictions.
    ///
    /// If the hat has `Edit` or `Write` in its `disallowed_tools`, checks whether
    /// files were modified (via `git diff --stat HEAD`). If so, emits a
    /// `<hat_id>.scope_violation` event.
    pub(super) fn audit_file_modifications(&mut self, hat_id: &HatId) {
        let config = match self.registry.get_config(hat_id) {
            Some(c) => c,
            None => return,
        };

        let has_write_restriction = config
            .disallowed_tools
            .iter()
            .any(|t| t == "Edit" || t == "Write");

        if !has_write_restriction {
            return;
        }

        let workspace = &self.config.core.workspace_root;
        let diff_output = std::process::Command::new("git")
            .args(["diff", "--stat", "HEAD"])
            .current_dir(workspace)
            .output();

        match diff_output {
            Ok(output) if !output.stdout.is_empty() => {
                let diff_stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
                warn!(
                    hat = %hat_id.as_str(),
                    diff = %diff_stat,
                    "Hat modified files despite tool restrictions (scope violation)"
                );

                let violation_topic = format!("{}.scope_violation", hat_id.as_str());
                let violation = Event::new(
                    violation_topic.as_str(),
                    format!(
                        "Hat '{}' modified files with Edit/Write disallowed:\n{}",
                        hat_id.as_str(),
                        diff_stat
                    ),
                );
                self.bus.publish(violation);
            }
            Err(e) => {
                debug!(error = %e, "Could not run git diff for file-modification audit");
            }
            _ => {} // No modifications — all good
        }
    }

    pub(super) fn warn_on_mutation_evidence(
        &self,
        evidence: &crate::event_parser::BackpressureEvidence,
    ) {
        let threshold = self.config.event_loop.mutation_score_warn_threshold;

        match &evidence.mutants {
            Some(mutants) => {
                if let Some(reason) = Self::mutation_warning_reason(mutants, threshold) {
                    warn!(
                        reason = %reason,
                        mutants_status = ?mutants.status,
                        mutants_score = mutants.score_percent,
                        mutants_threshold = threshold,
                        "Mutation testing warning"
                    );
                }
            }
            None => {
                if let Some(threshold) = threshold {
                    warn!(
                        mutants_threshold = threshold,
                        "Mutation testing warning: missing mutation evidence in build.done payload"
                    );
                }
            }
        }
    }

    pub(super) fn mutation_warning_reason(
        mutants: &MutationEvidence,
        threshold: Option<f64>,
    ) -> Option<String> {
        match mutants.status {
            MutationStatus::Fail => Some("mutation testing failed".to_string()),
            MutationStatus::Warn => Some(Self::format_mutation_message(
                "mutation score below threshold",
                mutants.score_percent,
            )),
            MutationStatus::Unknown => Some("mutation testing status unknown".to_string()),
            MutationStatus::Pass => {
                let threshold = threshold?;

                match mutants.score_percent {
                    Some(score) if score < threshold => Some(format!(
                        "mutation score {:.2}% below threshold {:.2}%",
                        score, threshold
                    )),
                    Some(_) => None,
                    None => Some(format!(
                        "mutation score missing (threshold {:.2}%)",
                        threshold
                    )),
                }
            }
        }
    }

    pub(super) fn format_mutation_message(message: &str, score: Option<f64>) -> String {
        match score {
            Some(score) => format!("{message} ({score:.2}%)"),
            None => message.to_string(),
        }
    }
}
