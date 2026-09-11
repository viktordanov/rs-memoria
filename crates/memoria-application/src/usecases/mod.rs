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
pub mod state_diff;
pub mod state_inspect;
pub mod status;

use memoria_domain::{DocumentId, DocumentStatus, InputChange, ManifestDiff, PendingCause};

use crate::error::{AppError, Detail, DetailMap};
use crate::ports::{LockFailure, Services, WriteGuard};

/// Acquire the project write lock or fail with `state_busy`.
pub(crate) fn acquire_lock<'a>(
    services: &Services<'a>,
) -> Result<Box<dyn WriteGuard + 'a>, AppError> {
    services.locks.lock().map_err(|failure| match failure {
        LockFailure::Busy => AppError::conflict(
            "state_busy",
            "another Memoria mutation holds the write lock; retry shortly",
        ),
        LockFailure::Io(err) => AppError::io("io_error", err.to_string()),
    })
}

/// Parse a CLI document path (project-root-relative).
pub(crate) fn parse_document(raw: &str) -> Result<DocumentId, AppError> {
    DocumentId::parse(raw).map_err(|err| AppError::usage("document_invalid", err.to_string()))
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
            identity: "README".into(),
            before: Some(before),
            after: Some(after),
        });
    }
    if let Some(before) = diff.document_removed {
        out.push(ChangeRecord {
            kind: "document",
            change: "removed",
            identity: "README".into(),
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
