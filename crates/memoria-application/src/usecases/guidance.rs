//! `memoria guidance [README.md]`: show project documentation guidance
//! without declaring staleness.
//!
//! The command is read-only. It works for current documents, needs no
//! review packet, and changes no state. Guidance informs judgment; it never
//! selects files and never decides byte-based freshness.

use memoria_domain::DocumentId;

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome};
use crate::guidance::entry_detail;
use crate::ports::Services;
use crate::snapshot::{self, Snapshot};

use super::parse_document;

/// One configuration scope that adds guidance, with its inspect command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSummary {
    pub scope: String,
    pub source: String,
    pub entries: u64,
    /// The command that shows the effective guidance of that scope.
    pub inspect_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceReport {
    pub document: String,
    pub digest: String,
    pub entries: Vec<Detail>,
    /// Sources contributing to this boundary, in applied order.
    pub sources: Vec<String>,
    /// Every scope that adds guidance anywhere in the project.
    pub scopes: Vec<ScopeSummary>,
    /// The digest this document's last review recorded, when one exists.
    pub reviewed_digest: Option<String>,
    pub changed_since_review: Option<bool>,
}

impl GuidanceReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("document", self.document.clone())
            .text("digest", self.digest.clone())
            .with("entries", Detail::List(self.entries.clone()))
            .with("sources", Detail::texts(self.sources.clone()))
            .with(
                "scopes",
                Detail::list(self.scopes.iter().map(|s| {
                    DetailMap::default()
                        .text("scope", s.scope.clone())
                        .text("source", s.source.clone())
                        .number("entries", s.entries)
                        .text("inspect_command", s.inspect_command.clone())
                        .build()
                })),
            )
            .with(
                "reviewed_digest",
                Detail::option_text(self.reviewed_digest.clone()),
            )
            .with(
                "changed_since_review",
                self.changed_since_review
                    .map(Detail::Bool)
                    .unwrap_or(Detail::Null),
            )
            .build()
    }
}

/// Build the report for one document of an existing snapshot.
pub fn report_for(snapshot: &Snapshot, document: &DocumentId) -> GuidanceReport {
    let effective = snapshot.guidance_of(document);
    let mut sources: Vec<String> = Vec::new();
    for entry in &effective.entries {
        if !sources.contains(&entry.source) {
            sources.push(entry.source.clone());
        }
    }
    let scopes = snapshot
        .guidance_scopes()
        .into_iter()
        .map(|(dir, source, entries)| ScopeSummary {
            scope: dir.as_str().to_string(),
            source,
            entries: entries as u64,
            inspect_command: format!("memoria guidance {}", dir.readme()),
        })
        .collect();
    let reviewed = snapshot
        .state
        .reviews
        .get(document)
        .map(|record| record.guidance.to_hex());
    GuidanceReport {
        document: document.as_str().to_string(),
        digest: effective.digest.to_hex(),
        entries: effective.entries.iter().map(entry_detail).collect(),
        sources,
        scopes,
        changed_since_review: snapshot.guidance_changed(document),
        reviewed_digest: reviewed,
    }
}

pub fn run(
    services: &Services<'_>,
    raw_document: Option<&str>,
) -> Result<Outcome<GuidanceReport>, AppError> {
    let snapshot = snapshot::build(services)?;
    let document = match raw_document {
        Some(raw) => parse_document(raw)?,
        None => memoria_domain::DirPath::root().readme(),
    };
    // A structural error can hide guidance: a missing, unreadable, or
    // forbidden guidance file contributes no entry. Reporting an empty list
    // would tell the reviewer that no guidance applies, which is false.
    // Every error is reported before any report is returned, for a present
    // document and an absent one alike.
    let errors = snapshot.structural_errors();
    if !errors.is_empty() {
        let report = snapshot
            .collected
            .documents
            .contains(&document)
            .then(|| report_for(&snapshot, &document));
        let failure = AppError::many(ExitClass::Validation, errors);
        return Err(match report {
            Some(report) => failure.with_data(report.to_detail()),
            None => failure,
        });
    }
    if !snapshot.collected.documents.contains(&document) {
        return Err(AppError::validation(
            "document_not_found",
            format!("{document} is not a discovered README"),
        ));
    }
    Ok(Outcome::new(
        report_for(&snapshot, &document),
        snapshot.non_error_diagnostics(),
    ))
}
