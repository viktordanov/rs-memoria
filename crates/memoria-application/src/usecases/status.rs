//! `memoria status`: footprint, review counts, invalidations, and explanations.

use std::collections::BTreeMap;

use memoria_domain::{DocumentId, DocumentKind, Exclusion, Handoff, ProjectPath, RuleKind};

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome};
use crate::ports::{FileKind, Services};
use crate::snapshot::{self, Snapshot};

use super::{document_status_detail, status_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidationSummary {
    pub id: u64,
    pub scope: String,
    pub reason: String,
    pub created_at: String,
    pub targets: Vec<String>,
    pub pending_existing: Vec<String>,
    pub pending_missing: Vec<String>,
}

/// One handoff as `status --explain` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffNote {
    /// The parent document that hands the subtree off.
    pub by: String,
    /// The tracked document that covers the subtree instead.
    pub to: String,
    pub subtree: String,
    /// `link`, `import`, or `both`.
    pub via: String,
    pub line: u64,
}

impl HandoffNote {
    pub fn from_handoff(handoff: &Handoff) -> HandoffNote {
        HandoffNote {
            by: handoff.parent.as_str().to_string(),
            to: handoff.target.as_str().to_string(),
            subtree: handoff.subtree.as_str().to_string(),
            via: handoff.via.as_str().to_string(),
            line: handoff.line as u64,
        }
    }

    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("by", self.by.clone())
            .text("to", self.to.clone())
            .text("subtree", self.subtree.clone())
            .text("via", self.via.clone())
            .number("line", self.line)
            .build()
    }
}

/// Scope facts about a tracked document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFacts {
    /// `readme` or `opted_in`.
    pub kind: String,
    pub scope_files: u64,
    pub handoffs: Vec<HandoffNote>,
    pub handed_off_by: Vec<HandoffNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub path: String,
    /// `document`, `selected`, `excluded`, `deleted`, `boundary`,
    /// `git-ignored`, or `not-eligible`.
    pub outcome: String,
    pub reason: String,
    pub steps: Vec<String>,
    /// For a selected source: every document whose scope contains it.
    pub covered_by: Vec<String>,
    /// For a selected source: every handoff whose subtree contains it.
    pub handed_off: Vec<HandoffNote>,
    /// For a tracked document: its kind, scope size, and handoffs.
    pub document: Option<DocumentFacts>,
}

impl Explanation {
    fn plain(path: &ProjectPath, outcome: &str, reason: String) -> Explanation {
        Explanation {
            path: path.as_str().to_string(),
            outcome: outcome.to_string(),
            reason,
            steps: vec![],
            covered_by: vec![],
            handed_off: vec![],
            document: None,
        }
    }

    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("path", self.path.clone())
            .text("outcome", self.outcome.clone())
            .text("reason", self.reason.clone())
            .with("steps", Detail::texts(self.steps.clone()))
            .with("covered_by", Detail::texts(self.covered_by.clone()))
            .with(
                "handed_off",
                Detail::list(self.handed_off.iter().map(HandoffNote::to_detail)),
            )
            .with(
                "document",
                match &self.document {
                    None => Detail::Null,
                    Some(facts) => DetailMap::default()
                        .text("kind", facts.kind.clone())
                        .number("scope_files", facts.scope_files)
                        .with(
                            "handoffs",
                            Detail::list(facts.handoffs.iter().map(HandoffNote::to_detail)),
                        )
                        .with(
                            "handed_off_by",
                            Detail::list(facts.handed_off_by.iter().map(HandoffNote::to_detail)),
                        )
                        .build(),
                },
            )
            .build()
    }
}

/// Document and scope counters shared by `status` and `status --summary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocumentCounts {
    pub documents: u64,
    pub readmes: u64,
    pub opted_in: u64,
    pub handoffs: u64,
    pub overlapping_sources: u64,
}

impl DocumentCounts {
    pub fn of(snapshot: &Snapshot) -> DocumentCounts {
        let readmes = snapshot
            .collected
            .documents
            .iter()
            .filter(|d| d.kind() == DocumentKind::Readme)
            .count() as u64;
        let documents = snapshot.collected.documents.len() as u64;
        DocumentCounts {
            documents,
            readmes,
            opted_in: documents - readmes,
            handoffs: snapshot.scopes.handoff_count() as u64,
            overlapping_sources: snapshot.scopes.overlapping().len() as u64,
        }
    }

    fn insert(self, map: DetailMap) -> DetailMap {
        map.number("documents", self.documents)
            .number("readmes", self.readmes)
            .number("opted_in_documents", self.opted_in)
            .number("handoffs", self.handoffs)
            .number("overlapping_sources", self.overlapping_sources)
    }
}

/// Advisory guidance counters. Guidance never makes a document stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuidanceCounts {
    /// Documents whose effective guidance has at least one entry.
    pub documents_with_guidance: u64,
    /// Reviewed documents whose guidance differs from the reviewed digest.
    pub changed_documents: u64,
    /// Documents that have no review to compare guidance against.
    pub unreviewed_documents: u64,
}

impl GuidanceCounts {
    pub fn to_detail(self) -> Detail {
        DetailMap::default()
            .number("documents_with_guidance", self.documents_with_guidance)
            .number("changed_documents", self.changed_documents)
            .number("unreviewed_documents", self.unreviewed_documents)
            .build()
    }
}

/// The bounded counters that `status --summary` publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSummary {
    pub counts: DocumentCounts,
    pub selected_files: u64,
    pub selected_bytes: u64,
    pub current: u64,
    pub pending: u64,
    pub never_reviewed: u64,
    /// Waiting can overlap current, pending, or never-reviewed status.
    pub waiting: u64,
    pub guidance: GuidanceCounts,
    pub unowned: u64,
    pub disconnected: u64,
    pub open_invalidations: u64,
    pub missing_invalidation_targets: u64,
    pub error_diagnostics: u64,
    pub warning_diagnostics: u64,
}

impl StatusSummary {
    pub fn to_detail(&self) -> Detail {
        self.counts
            .insert(DetailMap::default())
            .number("selected_files", self.selected_files)
            .number("selected_bytes", self.selected_bytes)
            .with(
                "reviews",
                DetailMap::default()
                    .number("current", self.current)
                    .number("pending", self.pending)
                    .number("never_reviewed", self.never_reviewed)
                    .number("waiting", self.waiting)
                    .build(),
            )
            .with("guidance", self.guidance.to_detail())
            .number("unowned", self.unowned)
            .number("disconnected", self.disconnected)
            .number("open_invalidations", self.open_invalidations)
            .number(
                "missing_invalidation_targets",
                self.missing_invalidation_targets,
            )
            .with(
                "diagnostics",
                DetailMap::default()
                    .number("errors", self.error_diagnostics)
                    .number("warnings", self.warning_diagnostics)
                    .build(),
            )
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    pub root: String,
    pub counts: DocumentCounts,
    pub selected_files: u64,
    pub selected_bytes: u64,
    pub current: u64,
    pub pending: u64,
    pub never_reviewed: u64,
    pub waiting: u64,
    pub guidance: GuidanceCounts,
    pub invalidations: Vec<InvalidationSummary>,
    pub unowned: Vec<String>,
    pub disconnected: Vec<String>,
    pub boundaries: Vec<String>,
    pub exclusions: BTreeMap<String, u64>,
    pub documents: Vec<Detail>,
    pub explanation: Option<Explanation>,
}

impl StatusReport {
    pub fn to_detail(&self) -> Detail {
        self.counts
            .insert(DetailMap::default().text("root", self.root.clone()))
            .number("selected_files", self.selected_files)
            .number("selected_bytes", self.selected_bytes)
            .with(
                "reviews",
                DetailMap::default()
                    .number("current", self.current)
                    .number("pending", self.pending)
                    .number("never_reviewed", self.never_reviewed)
                    .number("waiting", self.waiting)
                    .build(),
            )
            .with("guidance", self.guidance.to_detail())
            .with(
                "invalidations",
                Detail::list(self.invalidations.iter().map(|i| {
                    DetailMap::default()
                        .number("id", i.id)
                        .text("scope", i.scope.clone())
                        .text("reason", i.reason.clone())
                        .text("created_at", i.created_at.clone())
                        .with("targets", Detail::texts(i.targets.clone()))
                        .with(
                            "pending_existing",
                            Detail::texts(i.pending_existing.clone()),
                        )
                        .with("pending_missing", Detail::texts(i.pending_missing.clone()))
                        .build()
                })),
            )
            .with("unowned", Detail::texts(self.unowned.clone()))
            .with("disconnected", Detail::texts(self.disconnected.clone()))
            .with("boundaries", Detail::texts(self.boundaries.clone()))
            .with(
                "exclusions",
                Detail::Map(
                    self.exclusions
                        .iter()
                        .map(|(k, v)| (k.clone(), Detail::Number(*v)))
                        .collect(),
                ),
            )
            .with("documents", Detail::List(self.documents.clone()))
            .with(
                "explanation",
                match &self.explanation {
                    None => Detail::Null,
                    Some(e) => e.to_detail(),
                },
            )
            .build()
    }
}

/// Count the advisory guidance state of every discovered document.
pub fn guidance_counts(snapshot: &Snapshot) -> GuidanceCounts {
    let mut counts = GuidanceCounts::default();
    for document in &snapshot.collected.documents {
        if !snapshot.guidance_of(document).is_empty() {
            counts.documents_with_guidance += 1;
        }
        match snapshot.guidance_changed(document) {
            None => counts.unreviewed_documents += 1,
            Some(true) => counts.changed_documents += 1,
            Some(false) => {}
        }
    }
    counts
}

/// The bounded summary. It uses the same snapshot and freshness logic as
/// ordinary status and emits counts only: no per-document manifests, source
/// contents, full guidance, or exclusion explanations.
pub fn summary(snapshot: &Snapshot) -> StatusSummary {
    let mut current = 0;
    let mut pending = 0;
    let mut never = 0;
    let mut waiting = 0;
    for status in &snapshot.statuses {
        match status_label(status) {
            "current" => current += 1,
            "pending" => pending += 1,
            _ => never += 1,
        }
        if status.waiting() {
            waiting += 1;
        }
    }
    let missing_targets: u64 = snapshot
        .state
        .invalidations
        .iter()
        .map(|inv| {
            inv.pending
                .iter()
                .filter(|d| !snapshot.collected.documents.contains(d))
                .count() as u64
        })
        .sum();
    StatusSummary {
        counts: DocumentCounts::of(snapshot),
        selected_files: snapshot.collected.file_bytes.len() as u64,
        selected_bytes: snapshot.selected_bytes(),
        current,
        pending,
        never_reviewed: never,
        waiting,
        guidance: guidance_counts(snapshot),
        unowned: snapshot.scopes.uncovered().len() as u64,
        disconnected: snapshot.disconnected.len() as u64,
        open_invalidations: snapshot.state.invalidations.len() as u64,
        missing_invalidation_targets: missing_targets,
        error_diagnostics: snapshot.diagnostics.iter().filter(|d| d.is_error()).count() as u64,
        warning_diagnostics: snapshot
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::error::Severity::Warning)
            .count() as u64,
    }
}

pub fn run_summary(services: &Services<'_>) -> Result<Outcome<StatusSummary>, AppError> {
    let snapshot = snapshot::build(services)?;
    let report = summary(&snapshot);
    let errors = snapshot.structural_errors();
    if !errors.is_empty() {
        return Err(
            AppError::many(ExitClass::Validation, snapshot.diagnostics.clone())
                .with_data(report.to_detail()),
        );
    }
    Ok(Outcome::new(report, snapshot.non_error_diagnostics()))
}

pub fn summarize(services: &Services<'_>, snapshot: &Snapshot) -> StatusReport {
    let mut current = 0;
    let mut pending = 0;
    let mut never = 0;
    let mut waiting = 0;
    for status in &snapshot.statuses {
        match status_label(status) {
            "current" => current += 1,
            "pending" => pending += 1,
            _ => never += 1,
        }
        if status.waiting() {
            waiting += 1;
        }
    }
    let invalidations = snapshot
        .state
        .invalidations
        .iter()
        .map(|inv| InvalidationSummary {
            id: inv.id,
            scope: inv.scope.to_string(),
            reason: inv.reason.as_str().to_string(),
            created_at: inv.created_at.0.clone(),
            targets: inv.targets.iter().map(|d| d.as_str().to_string()).collect(),
            pending_existing: inv
                .pending
                .iter()
                .filter(|d| snapshot.collected.documents.contains(d))
                .map(|d| d.as_str().to_string())
                .collect(),
            pending_missing: inv
                .pending
                .iter()
                .filter(|d| !snapshot.collected.documents.contains(d))
                .map(|d| d.as_str().to_string())
                .collect(),
        })
        .collect();
    let mut documents: Vec<Detail> = snapshot
        .statuses
        .iter()
        .map(|status| {
            let mut detail = document_status_detail(status);
            if let Detail::Map(map) = &mut detail {
                let effective = snapshot.guidance_of(&status.document);
                let reviewed = snapshot
                    .state
                    .reviews
                    .get(&status.document)
                    .map(|r| r.guidance.to_hex());
                map.insert(
                    "guidance".into(),
                    DetailMap::default()
                        .bool("present", !effective.is_empty())
                        .with(
                            "changed_since_review",
                            snapshot
                                .guidance_changed(&status.document)
                                .map(Detail::Bool)
                                .unwrap_or(Detail::Null),
                        )
                        .text("current_digest", effective.digest.to_hex())
                        .with("reviewed_digest", Detail::option_text(reviewed))
                        .build(),
                );
                map.insert(
                    "document_kind".into(),
                    Detail::Text(status.document.kind().as_str().to_string()),
                );
                map.insert(
                    "scope_files".into(),
                    Detail::Number(snapshot.scopes.scope_of(&status.document).len() as u64),
                );
                map.insert(
                    "handoffs".into(),
                    Detail::list(
                        snapshot
                            .scopes
                            .handoffs_of(&status.document)
                            .iter()
                            .map(|h| HandoffNote::from_handoff(h).to_detail()),
                    ),
                );
                map.insert(
                    "disconnected".into(),
                    Detail::Bool(snapshot.disconnected.contains(&status.document)),
                );
                map.insert(
                    "render_required".into(),
                    Detail::Bool(!snapshot.outdated_imports_of(&status.document).is_empty()),
                );
            }
            detail
        })
        .collect();
    if snapshot.statuses.is_empty() {
        // Structural errors prevented scheduling; still list documents.
        documents = snapshot
            .collected
            .documents
            .iter()
            .map(|d| {
                DetailMap::default()
                    .text("document", d.as_str())
                    .text("status", "unknown")
                    .build()
            })
            .collect();
    }
    StatusReport {
        root: services.files.root_display(),
        counts: DocumentCounts::of(snapshot),
        selected_files: snapshot.collected.file_bytes.len() as u64,
        selected_bytes: snapshot.selected_bytes(),
        current,
        pending,
        never_reviewed: never,
        waiting,
        guidance: guidance_counts(snapshot),
        invalidations,
        unowned: snapshot
            .scopes
            .uncovered()
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        disconnected: snapshot
            .disconnected
            .iter()
            .map(|d| d.as_str().to_string())
            .collect(),
        boundaries: snapshot
            .collected
            .boundaries
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        exclusions: snapshot.selection_exclusion_summary(),
        documents,
        explanation: None,
    }
}

fn explain(
    services: &Services<'_>,
    snapshot: &Snapshot,
    raw: &str,
) -> Result<Explanation, AppError> {
    let path =
        ProjectPath::parse(raw).map_err(|err| AppError::usage("path_invalid", err.to_string()))?;
    if let Ok(document) = DocumentId::from_path(path.clone())
        && snapshot.collected.documents.contains(&document)
    {
        let kind = document.kind();
        let reason = match kind {
            DocumentKind::Readme => "a README.md: always a tracked document".to_string(),
            DocumentKind::OptedIn => format!(
                "Markdown with a Memoria marker (first at line {}): an opted-in document",
                snapshot
                    .collected
                    .opted_in
                    .get(&document)
                    .copied()
                    .unwrap_or(1)
            ),
        };
        return Ok(Explanation {
            document: Some(DocumentFacts {
                kind: kind.as_str().to_string(),
                scope_files: snapshot.scopes.scope_of(&document).len() as u64,
                handoffs: snapshot
                    .scopes
                    .handoffs_of(&document)
                    .iter()
                    .map(HandoffNote::from_handoff)
                    .collect(),
                handed_off_by: snapshot
                    .scopes
                    .handed_off_by(&document)
                    .into_iter()
                    .map(HandoffNote::from_handoff)
                    .collect(),
            }),
            ..Explanation::plain(&path, "document", reason)
        });
    }
    if let Some(decision) = snapshot.collected.decisions.get(&path) {
        let steps: Vec<String> = decision
            .steps
            .iter()
            .map(|step| {
                let scope = if step.scope.is_root() {
                    snapshot::ROOT_CONFIG_PATH.to_string()
                } else {
                    format!("{}/{}", step.scope.as_str(), snapshot::SIDECAR_FILE_NAME)
                };
                let kind = match step.kind {
                    RuleKind::Ignore => "ignore",
                    RuleKind::Include => "include",
                };
                format!("{scope}: {kind} {:?}", step.pattern)
            })
            .collect();
        let covered_by: Vec<String> = snapshot
            .scopes
            .covering(&path)
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        let handed_off: Vec<HandoffNote> = if covered_by.is_empty() {
            Vec::new()
        } else {
            snapshot
                .scopes
                .documents()
                .flat_map(|d| snapshot.scopes.handoffs_of(d))
                .filter(|h| path.is_within(&h.subtree))
                .map(HandoffNote::from_handoff)
                .collect()
        };
        let (outcome, reason) = match &decision.exclusion {
            None => {
                let kind = snapshot
                    .collected
                    .kinds
                    .get(&path)
                    .copied()
                    .unwrap_or(FileKind::Missing);
                if kind == FileKind::Missing {
                    (
                        "deleted".to_string(),
                        "tracked by Git but absent from the worktree".to_string(),
                    )
                } else if steps.is_empty() {
                    (
                        "selected".to_string(),
                        "eligible in Git and matched by no Memoria rule".to_string(),
                    )
                } else {
                    (
                        "selected".to_string(),
                        "eligible in Git; the last matching Memoria rule is an include".to_string(),
                    )
                }
            }
            Some(Exclusion::Reserved(category)) => (
                "excluded".to_string(),
                format!("reserved {category} file; never a source input"),
            ),
            Some(Exclusion::Rule { scope, pattern }) => {
                let source = if scope.is_root() {
                    snapshot::ROOT_CONFIG_PATH.to_string()
                } else {
                    format!("{}/{}", scope.as_str(), snapshot::SIDECAR_FILE_NAME)
                };
                (
                    "excluded".to_string(),
                    format!("ignored by {source} pattern {pattern:?}"),
                )
            }
        };
        return Ok(Explanation {
            path: path.as_str().to_string(),
            outcome,
            reason,
            steps,
            covered_by,
            handed_off,
            document: None,
        });
    }
    if snapshot.collected.boundaries.iter().any(|b| {
        &path == b
            || path.is_within(&memoria_domain::DirPath::parse(b.as_str()).unwrap_or_default())
    }) {
        return Ok(Explanation::plain(
            &path,
            "boundary",
            "inside a nested repository or submodule; Memoria does not enter it".into(),
        ));
    }
    let explanation = services
        .git
        .explain_ignore(path.as_str())
        .map_err(|err| AppError::io("git_unavailable", err.to_string()))?;
    Ok(match explanation {
        Some(rule) => Explanation::plain(&path, "git-ignored", format!("ignored by Git: {rule}")),
        None => Explanation::plain(
            &path,
            "not-eligible",
            "not tracked, not present, or otherwise not listed by Git".into(),
        ),
    })
}

pub fn run(
    services: &Services<'_>,
    explain_path: Option<&str>,
) -> Result<Outcome<StatusReport>, AppError> {
    let snapshot = snapshot::build(services)?;
    let mut report = summarize(services, &snapshot);
    if let Some(raw) = explain_path {
        report.explanation = Some(explain(services, &snapshot, raw)?);
    }
    let errors = snapshot.structural_errors();
    if !errors.is_empty() {
        return Err(
            AppError::many(ExitClass::Validation, snapshot.diagnostics.clone())
                .with_data(report.to_detail()),
        );
    }
    Ok(Outcome::new(report, snapshot.non_error_diagnostics()))
}
