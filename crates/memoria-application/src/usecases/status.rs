//! `memoria status`: footprint, review counts, invalidations, and explanations.

use std::collections::BTreeMap;

use memoria_domain::{Exclusion, ProjectPath, RuleKind};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub path: String,
    /// `selected`, `excluded`, `boundary`, `git-ignored`, `not-eligible`.
    pub outcome: String,
    pub owner: Option<String>,
    pub reason: String,
    pub steps: Vec<String>,
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
    pub readmes: u64,
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
        DetailMap::default()
            .number("readmes", self.readmes)
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
    pub readmes: u64,
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
        DetailMap::default()
            .text("root", self.root.clone())
            .number("readmes", self.readmes)
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
                    Some(e) => DetailMap::default()
                        .text("path", e.path.clone())
                        .text("outcome", e.outcome.clone())
                        .with("owner", Detail::option_text(e.owner.clone()))
                        .text("reason", e.reason.clone())
                        .with("steps", Detail::texts(e.steps.clone()))
                        .build(),
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
        readmes: snapshot.collected.documents.len() as u64,
        selected_files: snapshot.collected.file_bytes.len() as u64,
        selected_bytes: snapshot.selected_bytes(),
        current,
        pending,
        never_reviewed: never,
        waiting,
        guidance: guidance_counts(snapshot),
        unowned: snapshot.ownership.unowned().len() as u64,
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
                    "owned_files".into(),
                    Detail::Number(snapshot.ownership.owned_by(&status.document).len() as u64),
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
        readmes: snapshot.collected.documents.len() as u64,
        selected_files: snapshot.collected.file_bytes.len() as u64,
        selected_bytes: snapshot.selected_bytes(),
        current,
        pending,
        never_reviewed: never,
        waiting,
        guidance: guidance_counts(snapshot),
        invalidations,
        unowned: snapshot
            .ownership
            .unowned()
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
        let owner = snapshot
            .ownership
            .owner_of(&path)
            .map(|d| d.as_str().to_string());
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
            owner,
            reason,
            steps,
        });
    }
    if snapshot.collected.boundaries.iter().any(|b| {
        &path == b
            || path.is_within(&memoria_domain::DirPath::parse(b.as_str()).unwrap_or_default())
    }) {
        return Ok(Explanation {
            path: path.as_str().to_string(),
            outcome: "boundary".into(),
            owner: None,
            reason: "inside a nested repository or submodule; Memoria does not enter it".into(),
            steps: vec![],
        });
    }
    let explanation = services
        .git
        .explain_ignore(path.as_str())
        .map_err(|err| AppError::io("git_unavailable", err.to_string()))?;
    Ok(match explanation {
        Some(rule) => Explanation {
            path: path.as_str().to_string(),
            outcome: "git-ignored".into(),
            owner: None,
            reason: format!("ignored by Git: {rule}"),
            steps: vec![],
        },
        None => Explanation {
            path: path.as_str().to_string(),
            outcome: "not-eligible".into(),
            owner: None,
            reason: "not tracked, not present, or otherwise not listed by Git".into(),
            steps: vec![],
        },
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
