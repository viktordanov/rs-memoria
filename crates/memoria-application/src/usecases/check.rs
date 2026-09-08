//! `memoria check`: read-only CI validation.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::Services;
use crate::snapshot;

use super::{cause_detail, status_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub readmes: u64,
    pub pending: Vec<Detail>,
    pub outdated_imports: Vec<String>,
    pub structural_errors: u64,
    /// Advisory only. Changed guidance never fails `check`.
    pub guidance: super::status::GuidanceCounts,
}

impl CheckReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .number("readmes", self.readmes)
            .with("pending", Detail::List(self.pending.clone()))
            .with(
                "outdated_imports",
                Detail::texts(self.outdated_imports.clone()),
            )
            .number("structural_errors", self.structural_errors)
            .with("guidance", self.guidance.to_detail())
            .bool(
                "ok",
                self.pending.is_empty()
                    && self.outdated_imports.is_empty()
                    && self.structural_errors == 0,
            )
            .build()
    }
}

pub fn run(services: &Services<'_>) -> Result<Outcome<CheckReport>, AppError> {
    let snapshot = snapshot::build(services)?;
    let mut diagnostics: Vec<Diagnostic> = snapshot.diagnostics.clone();
    let mut pending = Vec::new();
    for status in &snapshot.statuses {
        if status.pending() {
            pending.push(
                DetailMap::default()
                    .text("document", status.document.as_str())
                    .text("status", status_label(status))
                    .with(
                        "causes",
                        Detail::list(status.causes.iter().map(cause_detail)),
                    )
                    .with(
                        "waiting_on",
                        Detail::texts(status.waiting_on.iter().map(|d| d.as_str().to_string())),
                    )
                    .build(),
            );
            let reasons: Vec<&str> = status.causes.iter().map(|c| c.code()).collect();
            diagnostics.push(
                Diagnostic::error(
                    "review_pending",
                    format!("README needs review ({})", reasons.join(", ")),
                )
                .at_path(status.document.as_str())
                .with_details(
                    DetailMap::default()
                        .with(
                            "causes",
                            Detail::list(status.causes.iter().map(cause_detail)),
                        )
                        .build(),
                ),
            );
        }
    }
    let outdated: Vec<String> = snapshot
        .outdated_imports
        .iter()
        .map(|o| format!("{}: {}#{}", o.document, o.provider, o.export_id))
        .collect();
    for warning in diagnostics
        .iter_mut()
        .filter(|d| d.code == "imports_outdated")
    {
        warning.severity = crate::error::Severity::Error;
    }
    let structural_errors = snapshot.structural_errors().len() as u64;
    // Changed guidance is advisory context. It is reported as a hint and
    // never turns a byte-current document into a failure.
    let guidance = super::status::guidance_counts(&snapshot);
    if guidance.changed_documents > 0 {
        diagnostics.push(
            Diagnostic::hint(
                "guidance_changed",
                format!(
                    "{} reviewed document(s) show guidance that changed since their review; byte-based freshness is separate. Run `memoria guidance <README.md>`, then `memoria invalidate` the scope the change affects.",
                    guidance.changed_documents
                ),
            )
            .with_details(
                DetailMap::default()
                    .number("changed_documents", guidance.changed_documents)
                    .build(),
            ),
        );
    }
    let report = CheckReport {
        readmes: snapshot.collected.documents.len() as u64,
        pending,
        outdated_imports: outdated,
        structural_errors,
        guidance,
    };
    if diagnostics.iter().any(|d| d.is_error()) {
        return Err(
            AppError::many(ExitClass::Validation, diagnostics).with_data(report.to_detail())
        );
    }
    Ok(Outcome::new(report, diagnostics))
}
