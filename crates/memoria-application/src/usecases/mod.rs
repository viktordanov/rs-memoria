//! Use cases, one per command. Each shares the snapshot pipeline.

pub mod ack;
pub mod agent;
pub mod agent_hooks;
pub mod check;
mod evidence;
pub mod explain;
pub mod github_workflow;
pub mod graph;
pub mod guidance;
mod history;
pub mod init;
pub mod invalidate;
pub mod lint;
pub mod packet_view;
pub mod plan;
pub mod prepare_review;
pub mod render;
pub mod requirements;
pub mod save_artifact;
pub mod state_diff;
pub mod state_inspect;
pub mod status;

use memoria_domain::{DocumentId, DocumentStatus, InputChange, ManifestDiff, PendingCause};

use crate::error::{AppError, Detail, DetailMap};
use crate::ports::{LockFailure, Services, WriteGuard};
use crate::snapshot::Snapshot;

/// Acquire the project write lock or fail with `state_busy`.
///
/// The coordinator already waited for its budget before it reports a busy
/// lock. The request itself is still valid, so the message says to repeat it
/// unchanged: a review artifact does not need a fresh capture.
pub(crate) fn acquire_lock<'a>(
    services: &Services<'a>,
) -> Result<Box<dyn WriteGuard + 'a>, AppError> {
    services.locks.lock().map_err(|failure| match failure {
        LockFailure::Busy => AppError::conflict(
            "state_busy",
            "another Memoria mutation still holds the write lock after the wait; repeat the same command. A review artifact stays valid, so do not capture a new one for this error",
        ),
        LockFailure::Io(err) => AppError::io("io_error", err.to_string()),
    })
}

/// Parse a CLI document path (project-root-relative): `README.md`, `*.md`,
/// or `*.markdown`.
pub(crate) fn parse_document(raw: &str) -> Result<DocumentId, AppError> {
    DocumentId::parse(raw).map_err(|err| AppError::usage("document_invalid", err.to_string()))
}

/// The error for a CLI path that names no tracked document. An unmarked
/// Markdown source says which documents cover it and how to track it.
pub(crate) fn document_not_found(snapshot: &Snapshot, document: &DocumentId) -> AppError {
    if snapshot.collected.file_bytes.contains_key(document.path()) {
        let covered_by: Vec<String> = snapshot
            .scopes
            .covering(document.path())
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        let owners = if covered_by.is_empty() {
            "no document".to_string()
        } else {
            covered_by.join(", ")
        };
        return AppError::new(
            crate::error::ExitClass::Validation,
            crate::error::Diagnostic::error(
                "document_not_found",
                format!(
                    "{document} is ordinary Markdown with no Memoria marker, so it is a source covered by {owners}, not a tracked document; add an import, export, or section marker to track it"
                ),
            )
            .at_path(document.as_str())
            .with_details(
                DetailMap::default()
                    .with("covered_by", Detail::texts(covered_by))
                    .build(),
            ),
        );
    }
    AppError::validation(
        "document_not_found",
        format!("{document} is not a tracked document"),
    )
}

pub(crate) fn status_label(status: &DocumentStatus) -> &'static str {
    if status.pending()
        && status
            .causes
            .iter()
            .all(|c| matches!(c, PendingCause::NeverReviewed))
    {
        "never_reviewed"
    } else if status.pending() {
        "pending"
    } else {
        "current"
    }
}

pub(crate) fn cause_detail(cause: &PendingCause) -> Detail {
    let base = DetailMap::default().text("code", cause.code());
    match cause {
        PendingCause::NeverReviewed => base.build(),
        PendingCause::InputChanged(diff) => base.with("changes", diff_detail(diff)).build(),
        PendingCause::DocumentChanged { before, after } => base
            .number("before_bytes", before.0)
            .text("before_hash", before.1.to_hex())
            .number("after_bytes", after.0)
            .text("after_hash", after.1.to_hex())
            .build(),
        PendingCause::ExplicitInvalidation { id, reason } => base
            .number("id", *id)
            .text("reason", reason.clone())
            .build(),
    }
}

/// Every difference in a manifest diff as a list of change records.
pub(crate) fn diff_detail(diff: &ManifestDiff) -> Detail {
    Detail::list(diff_changes(diff).into_iter().map(|c| c.to_detail()))
}

/// A flattened change record shared by plans, packets, and conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRecord {
    pub kind: &'static str,
    pub change: &'static str,
    pub identity: String,
    pub before: Option<(u64, memoria_domain::Hash64)>,
    pub after: Option<(u64, memoria_domain::Hash64)>,
}

impl ChangeRecord {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", self.kind)
            .text("change", self.change)
            .text("identity", self.identity.clone())
            .with(
                "before_bytes",
                self.before
                    .map(|b| Detail::Number(b.0))
                    .unwrap_or(Detail::Null),
            )
            .with(
                "before_hash",
                Detail::option_text(self.before.map(|b| b.1.to_hex())),
            )
            .with(
                "after_bytes",
                self.after
                    .map(|a| Detail::Number(a.0))
                    .unwrap_or(Detail::Null),
            )
            .with(
                "after_hash",
                Detail::option_text(self.after.map(|a| a.1.to_hex())),
            )
            .build()
    }
}

pub(crate) fn diff_changes(diff: &ManifestDiff) -> Vec<ChangeRecord> {
    let mut out = Vec::new();
    if let Some((before, after)) = diff.policy {
        out.push(ChangeRecord {
            kind: "policy",
            change: "changed",
            identity: "policy".into(),
            before: Some((0, before)),
            after: Some((0, after)),
        });
    }
    if let Some((before, after)) = diff.document {
        out.push(ChangeRecord {
            kind: "document",
            change: "changed",
            identity: "document".into(),
            before: Some(before),
            after: Some(after),
        });
    }
    if let Some(before) = diff.document_removed {
        out.push(ChangeRecord {
            kind: "document",
            change: "removed",
            identity: "document".into(),
            before: Some(before),
            after: None,
        });
    }
    for change in &diff.files {
        out.push(match change {
            InputChange::Added(f) => ChangeRecord {
                kind: "file",
                change: "added",
                identity: f.path.as_str().into(),
                before: None,
                after: Some((f.bytes, f.hash)),
            },
            InputChange::Removed(f) => ChangeRecord {
                kind: "file",
                change: "removed",
                identity: f.path.as_str().into(),
                before: Some((f.bytes, f.hash)),
                after: None,
            },
            InputChange::Changed { before, after } => ChangeRecord {
                kind: "file",
                change: "changed",
                identity: after.path.as_str().into(),
                before: Some((before.bytes, before.hash)),
                after: Some((after.bytes, after.hash)),
            },
        });
    }
    for change in &diff.imports {
        let identity = |i: &memoria_domain::ImportInput| format!("{}#{}", i.document, i.export_id);
        out.push(match change {
            InputChange::Added(i) => ChangeRecord {
                kind: "import",
                change: "added",
                identity: identity(i),
                before: None,
                after: Some((i.bytes, i.hash)),
            },
            InputChange::Removed(i) => ChangeRecord {
                kind: "import",
                change: "removed",
                identity: identity(i),
                before: Some((i.bytes, i.hash)),
                after: None,
            },
            InputChange::Changed { before, after } => ChangeRecord {
                kind: "import",
                change: "changed",
                identity: identity(after),
                before: Some((before.bytes, before.hash)),
                after: Some((after.bytes, after.hash)),
            },
        });
    }
    out
}

pub(crate) fn document_status_detail(status: &DocumentStatus) -> Detail {
    DetailMap::default()
        .text("document", status.document.as_str())
        .text("status", status_label(status))
        .number("revision", status.revision)
        .bool("pending", status.pending())
        .bool("ready", status.ready())
        .bool("waiting", status.waiting())
        .with(
            "waiting_on",
            Detail::texts(status.waiting_on.iter().map(|d| d.as_str().to_string())),
        )
        .with(
            "causes",
            Detail::list(status.causes.iter().map(cause_detail)),
        )
        .build()
}
