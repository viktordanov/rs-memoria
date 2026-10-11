//! `memoria lint`: structure, configuration, markers, and link hints.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome, Severity};
use crate::ports::Services;
use crate::snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintReport {
    pub documents: u64,
    pub readmes: u64,
    pub errors: u64,
    pub warnings: u64,
    pub hints: u64,
}

impl LintReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .number("documents", self.documents)
            .number("readmes", self.readmes)
            .number("errors", self.errors)
            .number("warnings", self.warnings)
            .number("hints", self.hints)
            .build()
    }
}

/// A hint for each section token that matches nothing. Only `lint` reports
/// it, so routine commands do not repeat it. `status --explain` shows the
/// same tokens for one document.
fn unmatched_section_hints(snapshot: &snapshot::Snapshot) -> Vec<Diagnostic> {
    let mut hints = Vec::new();
    for (document, map) in &snapshot.sections {
        for section in map.sections() {
            for token in &section.unmatched {
                let message = if token.starts_with('!') {
                    format!(
                        "section {:?}: `{token}` removes no source that the section includes",
                        section.id.as_str()
                    )
                } else {
                    format!(
                        "section {:?}: `{token}` matches no source in this document's scope",
                        section.id.as_str()
                    )
                };
                let mut hint = Diagnostic::hint("section_pattern_empty", message)
                    .at_path(document.as_str())
                    .with_details(
                        DetailMap::default()
                            .text("section", section.id.as_str())
                            .text("token", token.as_str())
                            .build(),
                    );
                if let Some(line) = section.first_line.checked_sub(1) {
                    hint = hint.at(line, 1);
                }
                hints.push(hint);
            }
        }
    }
    hints
}

pub fn run(services: &Services<'_>) -> Result<Outcome<LintReport>, AppError> {
    let mut snapshot = snapshot::build(services)?;
    let hints = unmatched_section_hints(&snapshot);
    snapshot.diagnostics.extend(hints);
    let count = |severity: Severity| {
        snapshot
            .diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count() as u64
    };
    let report = LintReport {
        documents: snapshot.collected.documents.len() as u64,
        readmes: super::status::DocumentCounts::of(&snapshot).readmes,
        errors: count(Severity::Error),
        warnings: count(Severity::Warning),
        hints: count(Severity::Hint),
    };
    if report.errors > 0 {
        return Err(
            AppError::many(ExitClass::Validation, snapshot.diagnostics.clone())
                .with_data(report.to_detail()),
        );
    }
    Ok(Outcome::new(report, snapshot.diagnostics.clone()))
}
