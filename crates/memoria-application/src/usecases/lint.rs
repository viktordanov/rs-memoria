//! `memoria lint`: structure, configuration, markers, and link hints.

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome, Severity};
use crate::ports::Services;
use crate::snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintReport {
    pub readmes: u64,
    pub errors: u64,
    pub warnings: u64,
    pub hints: u64,
}

impl LintReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .number("readmes", self.readmes)
            .number("errors", self.errors)
            .number("warnings", self.warnings)
            .number("hints", self.hints)
            .build()
    }
}

pub fn run(services: &Services<'_>) -> Result<Outcome<LintReport>, AppError> {
    let snapshot = snapshot::build(services)?;
    let count = |severity: Severity| {
        snapshot
            .diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count() as u64
    };
    let report = LintReport {
        readmes: snapshot.collected.documents.len() as u64,
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
