//! `memoria state inspect`: read-only inspection of committed state.
//!
//! Inspection decodes bytes. It never compares archived records with current
//! source files and never claims that they are current. Ordinary `status`
//! remains the next action for worktree freshness. Inspection offers no
//! editable export, import, reset, or migration.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::packet::manifest_detail;
use crate::ports::{InspectedState, Services, StateInspector};
use crate::snapshot::STATE_PATH;

/// Buffered rendering limit before stdout: 256 MiB.
pub const MAX_RENDERED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectReport {
    pub inspected: InspectedState,
}

pub(crate) fn state_detail(inspected: &InspectedState) -> Detail {
    let state = &inspected.state;
    let guidance: std::collections::BTreeMap<&str, &str> = inspected
        .guidance
        .iter()
        .map(|(document, digest)| (document.as_str(), digest.as_str()))
        .collect();
    let reviews = Detail::Map(
        state
            .reviews
            .iter()
            .map(|(document, record)| {
                let digest = guidance
                    .get(document.as_str())
                    .copied()
                    .unwrap_or("0000000000000000");
                (
                    document.as_str().to_string(),
                    DetailMap::default()
                        .number("revision", record.revision)
                        .with("input_manifest", manifest_detail(&record.manifest))
                        .text("input_fingerprint", record.input_fingerprint.to_hex())
                        .text("token_digest", record.token_digest.to_hex())
                        .text("guidance_digest", digest)
                        .text("reviewed_at", record.reviewed_at.0.clone())
                        .text("reviewer", record.reviewer.as_str())
                        .text("result", record.result.as_str())
                        .text("note", record.note.as_str())
                        .with(
                            "git",
                            DetailMap::default()
                                .with(
                                    "base_commit",
                                    Detail::option_text(record.git.base_commit.clone()),
                                )
                                .bool("worktree_dirty", record.git.worktree_dirty)
                                .build(),
                        )
                        .with(
                            "acknowledged_invalidations",
                            Detail::list(
                                record
                                    .acknowledged_invalidations
                                    .iter()
                                    .map(|id| Detail::Number(*id)),
                            ),
                        )
                        .build(),
                )
            })
            .collect(),
    );
    let invalidations = Detail::list(state.invalidations.iter().map(|inv| {
        DetailMap::default()
            .number("id", inv.id)
            .text("scope", inv.scope.to_string())
            .text("reason", inv.reason.as_str())
            .text("created_at", inv.created_at.0.clone())
            .with(
                "targets",
                Detail::texts(inv.targets.iter().map(|d| d.as_str().to_string())),
            )
            .with(
                "pending_documents",
                Detail::texts(inv.pending.iter().map(|d| d.as_str().to_string())),
            )
            .build()
    }));
    DetailMap::default()
        .number("schema_version", 2)
        .number("revision", state.revision)
        .number("next_invalidation_id", state.next_invalidation_id)
        .with("reviews", reviews)
        .with("invalidations", invalidations)
        .build()
}

impl InspectReport {
    pub fn to_detail(&self) -> Detail {
        let i = &self.inspected;
        DetailMap::default()
            .text("path", i.path.clone())
            .number("file_bytes", i.file_bytes)
            .number("payload_bytes", i.payload_bytes)
            .number("format_version", i.format_version)
            .text("codec", i.codec)
            .text("checksum", i.checksum.clone())
            .with("state", state_detail(i))
            .build()
    }
}

fn failure(failure: crate::ports::StateFailure, path: &str) -> AppError {
    match failure {
        crate::ports::StateFailure::Io(err) => AppError::new(
            ExitClass::Io,
            Diagnostic::error("state_unreadable", err.to_string()).at_path(path),
        ),
        other => AppError::new(
            ExitClass::Io,
            Diagnostic::error(other.code(), other.message()).at_path(path),
        ),
    }
}

/// Inspect the project's `memoria.lock`.
pub fn run(services: &Services<'_>) -> Result<Outcome<InspectReport>, AppError> {
    let inspected = services
        .inspector
        .inspect_project()
        .map_err(|e| failure(e, STATE_PATH))?;
    finish(inspected)
}

/// Inspect an explicit file, without project discovery or a worktree scan.
pub fn run_file(
    inspector: &dyn StateInspector,
    path: &str,
) -> Result<Outcome<InspectReport>, AppError> {
    let inspected = inspector.inspect_file(path).map_err(|e| failure(e, path))?;
    finish(inspected)
}

fn finish(inspected: InspectedState) -> Result<Outcome<InspectReport>, AppError> {
    let report = InspectReport { inspected };
    // Rendering is buffered before stdout, so an output-limit failure never
    // emits a partial JSON object.
    let rendered = report.to_detail().approximate_bytes();
    if rendered > MAX_RENDERED_BYTES {
        return Err(AppError::new(
            ExitClass::Io,
            Diagnostic::error(
                "state_inspection_limit_exceeded",
                format!(
                    "the rendered inspection would be about {rendered} bytes, above the limit of {MAX_RENDERED_BYTES}"
                ),
            )
            .at_path(&report.inspected.path),
        ));
    }
    Ok(Outcome::new(report, vec![]))
}
