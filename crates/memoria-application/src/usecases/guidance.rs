//! `memoria guidance [DOCUMENT]`: show project documentation guidance
//! without declaring staleness.
//!
//! The command is read-only. It works for current documents, needs no review
//! artifact, and changes no state. Guidance informs judgment. It never
//! selects files and never decides byte-based freshness.
//!
//! `memoria guidance --changed` is the guidance assessment: it lists the
//! reviewed documents whose effective guidance differs from the guidance
//! their last review saw. It records nothing. The reviewer decides which
//! documents the change affects and requests their review explicitly.

use memoria_domain::{DocumentId, GuidanceKind};

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

/// One registered section guide and how many sections name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionGuideSummary {
    pub source: String,
    /// Documents with at least one section that names the guide.
    pub documents: u64,
    /// Sections, across all documents, that name the guide.
    pub sections: u64,
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
    /// Every registered section guide, for `memoria guidance` without a
    /// document. Empty for a named document and for a project that
    /// registers none.
    pub section_guides: Vec<SectionGuideSummary>,
}

impl GuidanceReport {
    /// Whether any section guide applies to this document.
    pub fn has_section_guides(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.get("kind") == Some(&Detail::text(GuidanceKind::Section.as_str())))
    }
}

impl GuidanceReport {
    pub fn to_detail(&self) -> Detail {
        let detail = DetailMap::default()
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
            );
        // Added only when a section guide is registered, so a project that
        // registers none keeps its exact 0.8 output.
        if self.section_guides.is_empty() {
            return detail.build();
        }
        detail
            .with(
                "section_guides",
                Detail::list(self.section_guides.iter().map(|guide| {
                    DetailMap::default()
                        .text("source", guide.source.clone())
                        .number("documents", guide.documents)
                        .number("sections", guide.sections)
                        .build()
                })),
            )
            .build()
    }
}

/// Every registered section guide with its use across the project.
pub fn section_guide_summary(snapshot: &Snapshot) -> Vec<SectionGuideSummary> {
    snapshot
        .collected
        .section_guides
        .iter()
        .map(|path| {
            let mut documents = 0;
            let mut sections = 0;
            for effective in snapshot.guidance.values() {
                if let Some(entry) = effective
                    .section_guides()
                    .find(|entry| entry.source == path.as_str())
                {
                    documents += 1;
                    sections += entry.sections.len() as u64;
                }
            }
            SectionGuideSummary {
                source: path.as_str().to_string(),
                documents,
                sections,
            }
        })
        .collect()
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
        section_guides: Vec::new(),
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
        return Err(super::document_not_found(&snapshot, &document));
    }
    let mut report = report_for(&snapshot, &document);
    if raw_document.is_none() {
        report.section_guides = section_guide_summary(&snapshot);
    }
    Ok(Outcome::new(report, snapshot.non_error_diagnostics()))
}

/// Reviewed documents that saw the same guidance change: the same digest at
/// review time and the same digest now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceChangeGroup {
    pub reviewed_digest: String,
    pub current_digest: String,
    pub documents: Vec<String>,
    /// The current guidance sources of these documents, in applied order.
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceAssessment {
    pub changed_documents: u64,
    pub groups: Vec<GuidanceChangeGroup>,
}

impl GuidanceAssessment {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", "guidance_assessment")
            .number("changed_documents", self.changed_documents)
            .with(
                "groups",
                Detail::list(self.groups.iter().map(|group| {
                    DetailMap::default()
                        .text("reviewed_digest", group.reviewed_digest.clone())
                        .text("current_digest", group.current_digest.clone())
                        .with("documents", Detail::texts(group.documents.clone()))
                        .with("sources", Detail::texts(group.sources.clone()))
                        .build()
                })),
            )
            .build()
    }
}

/// Group the reviewed documents whose guidance changed since their review.
pub fn assessment_for(snapshot: &Snapshot) -> GuidanceAssessment {
    let mut groups: Vec<GuidanceChangeGroup> = Vec::new();
    let mut changed = 0;
    for document in &snapshot.collected.documents {
        if snapshot.guidance_changed(document) != Some(true) {
            continue;
        }
        let Some(record) = snapshot.state.reviews.get(document) else {
            continue;
        };
        changed += 1;
        let effective = snapshot.guidance_of(document);
        let reviewed = record.guidance.to_hex();
        let current = effective.digest.to_hex();
        let index = match groups
            .iter()
            .position(|g| g.reviewed_digest == reviewed && g.current_digest == current)
        {
            Some(index) => index,
            None => {
                groups.push(GuidanceChangeGroup {
                    reviewed_digest: reviewed,
                    current_digest: current,
                    documents: Vec::new(),
                    sources: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        group.documents.push(document.as_str().to_string());
        for entry in &effective.entries {
            if !group.sources.contains(&entry.source) {
                group.sources.push(entry.source.clone());
            }
        }
    }
    for group in &mut groups {
        group.documents.sort();
    }
    groups.sort_by(|a, b| {
        b.documents
            .len()
            .cmp(&a.documents.len())
            .then_with(|| a.documents.cmp(&b.documents))
    });
    GuidanceAssessment {
        changed_documents: changed,
        groups,
    }
}

pub fn run_changed(services: &Services<'_>) -> Result<Outcome<GuidanceAssessment>, AppError> {
    let snapshot = snapshot::build(services)?;
    // A structural error can hide guidance, so a partial assessment would
    // understate the change. Report the errors instead.
    let errors = snapshot.structural_errors();
    if !errors.is_empty() {
        return Err(AppError::many(ExitClass::Validation, errors));
    }
    Ok(Outcome::new(
        assessment_for(&snapshot),
        snapshot.non_error_diagnostics(),
    ))
}
