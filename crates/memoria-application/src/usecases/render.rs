//! `memoria render`: refresh declared import blocks only.

use memoria_domain::DocumentId;

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::Services;
use crate::snapshot::{self, Snapshot};

use super::{acquire_lock, parse_document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub provider: String,
    pub export_id: String,
    pub line: usize,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRender {
    pub document: DocumentId,
    pub patches: Vec<Patch>,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderReport {
    pub dry_run: bool,
    pub documents: Vec<DocumentRender>,
}

impl RenderReport {
    pub fn changed(&self) -> impl Iterator<Item = &DocumentRender> {
        self.documents.iter().filter(|d| !d.patches.is_empty())
    }

    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .bool("dry_run", self.dry_run)
            .number("documents_checked", self.documents.len() as u64)
            .number("documents_changed", self.changed().count() as u64)
            .with(
                "changes",
                Detail::list(self.changed().map(|d| {
                    DetailMap::default()
                        .text("document", d.document.as_str())
                        .bool("applied", d.applied)
                        .number("before_bytes", d.before.len() as u64)
                        .number("after_bytes", d.after.len() as u64)
                        .with(
                            "patches",
                            Detail::list(d.patches.iter().map(|p| {
                                DetailMap::default()
                                    .text("provider", p.provider.clone())
                                    .text("export_id", p.export_id.clone())
                                    .number("line", p.line as u64)
                                    .number("before_bytes", p.before_bytes)
                                    .number("after_bytes", p.after_bytes)
                                    .build()
                            })),
                        )
                        .build()
                })),
            )
            .build()
    }
}

/// Compute the rendered bytes of one document without writing.
pub fn render_document(snapshot: &Snapshot, document: &DocumentId) -> DocumentRender {
    let before = snapshot.document_bytes(document).to_vec();
    let parsed = &snapshot.documents[document];
    let mut replacements: Vec<(memoria_domain::ByteRange, Vec<u8>, Patch)> = Vec::new();
    for import in &parsed.imports {
        let Some(export) = snapshot.export_body(&import.provider, &import.export_id) else {
            continue;
        };
        let current = &before[import.body.start..import.body.end];
        if current != export {
            replacements.push((
                import.body,
                export.to_vec(),
                Patch {
                    provider: import.provider.as_str().to_string(),
                    export_id: import.export_id.as_str().to_string(),
                    line: import.location.line,
                    before_bytes: current.len() as u64,
                    after_bytes: export.len() as u64,
                },
            ));
        }
    }
    // Apply in descending offset order so earlier offsets stay valid.
    replacements.sort_by_key(|entry| std::cmp::Reverse(entry.0.start));
    let mut after = before.clone();
    let mut patches = Vec::new();
    for (range, body, patch) in replacements {
        after.splice(range.start..range.end, body);
        patches.push(patch);
    }
    patches.sort_by_key(|p| p.line);
    DocumentRender {
        document: document.clone(),
        patches,
        before,
        after,
        applied: false,
    }
}

pub fn run(
    services: &Services<'_>,
    raw_document: Option<&str>,
    dry_run: bool,
) -> Result<Outcome<RenderReport>, AppError> {
    let selected = raw_document.map(parse_document).transpose()?;
    let snapshot = snapshot::build(services)?;
    snapshot.require_valid()?;
    let targets: Vec<DocumentId> = match &selected {
        Some(document) => {
            if !snapshot.collected.documents.contains(document) {
                return Err(AppError::validation(
                    "document_not_found",
                    format!("{document} is not a discovered README"),
                ));
            }
            vec![document.clone()]
        }
        None => snapshot.collected.documents.iter().cloned().collect(),
    };
    let mut report = RenderReport {
        dry_run,
        documents: targets
            .iter()
            .map(|d| render_document(&snapshot, d))
            .collect(),
    };
    if dry_run || report.changed().count() == 0 {
        return Ok(Outcome::new(report, snapshot.non_error_diagnostics()));
    }
    let _guard = acquire_lock(services)?;
    for document in report.changed() {
        services.progress.note(&format!(
            "render: {} ({} import block(s))",
            document.document,
            document.patches.len()
        ));
    }
    let mut failures = Vec::new();
    for entry in report
        .documents
        .iter_mut()
        .filter(|d| !d.patches.is_empty())
    {
        if !failures.is_empty() {
            break;
        }
        match services
            .writer
            .replace(entry.document.as_str(), &entry.before, &entry.after)
        {
            Ok(()) => entry.applied = true,
            Err(err) => failures.push(
                Diagnostic::error("io_error", format!("render stopped: {err}"))
                    .at_path(entry.document.as_str()),
            ),
        }
    }
    if !failures.is_empty() {
        let unapplied: Vec<String> = report
            .changed()
            .filter(|d| !d.applied)
            .map(|d| d.document.as_str().to_string())
            .collect();
        failures.push(Diagnostic::error(
            "render_incomplete",
            format!(
                "unapplied documents: {}; rerun `memoria render`",
                unapplied.join(", ")
            ),
        ));
        return Err(AppError::many(ExitClass::Io, failures).with_data(report.to_detail()));
    }
    Ok(Outcome::new(report, snapshot.non_error_diagnostics()))
}
