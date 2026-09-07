//! `memoria graph`: ownership, imports, navigation, and review status.

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome};
use crate::ports::Services;
use crate::snapshot;

use super::{cause_detail, status_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphReport {
    pub nodes: Vec<Detail>,
    pub edges: Vec<Detail>,
}

impl GraphReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .with("nodes", Detail::List(self.nodes.clone()))
            .with("edges", Detail::List(self.edges.clone()))
            .build()
    }
}

pub fn run(services: &Services<'_>) -> Result<Outcome<GraphReport>, AppError> {
    let snapshot = snapshot::build(services)?;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for document in &snapshot.collected.documents {
        let status = snapshot.status_of(document);
        let node = DetailMap::default()
            .text("document", document.as_str())
            .number(
                "owned_files",
                snapshot.ownership.owned_by(document).len() as u64,
            )
            .number(
                "owned_bytes",
                snapshot
                    .ownership
                    .owned_by(document)
                    .iter()
                    .map(|p| snapshot.file_bytes(p).len() as u64)
                    .sum(),
            )
            .text("status", status.map(status_label).unwrap_or("unknown"))
            .bool("pending", status.is_some_and(|s| s.pending()))
            .bool("ready", status.is_some_and(|s| s.ready()))
            .bool("waiting", status.is_some_and(|s| s.waiting()))
            .with(
                "waiting_on",
                Detail::texts(
                    status
                        .map(|s| {
                            s.waiting_on
                                .iter()
                                .map(|d| d.as_str().to_string())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                ),
            )
            .with(
                "causes",
                Detail::list(
                    status
                        .map(|s| s.causes.iter().map(cause_detail).collect::<Vec<_>>())
                        .unwrap_or_default(),
                ),
            )
            .bool("disconnected", snapshot.disconnected.contains(document))
            .bool(
                "render_required",
                !snapshot.outdated_imports_of(document).is_empty(),
            )
            .with(
                "exports",
                Detail::texts(
                    snapshot
                        .documents
                        .get(document)
                        .map(|d| {
                            d.exports
                                .iter()
                                .map(|e| e.id.as_str().to_string())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                ),
            )
            .build();
        nodes.push(node);
        if let Some(parent) = snapshot.ownership.parent_of(document) {
            edges.push(
                DetailMap::default()
                    .text("kind", "owner")
                    .text("from", parent.as_str())
                    .text("to", document.as_str())
                    .build(),
            );
        }
        if let Some(parsed) = snapshot.documents.get(document) {
            for import in &parsed.imports {
                edges.push(
                    DetailMap::default()
                        .text("kind", "import")
                        .text("from", document.as_str())
                        .text("to", import.provider.as_str())
                        .text("export_id", import.export_id.as_str())
                        .number("line", import.location.line as u64)
                        .build(),
                );
            }
            for link in &parsed.links {
                if !parsed.imports.iter().any(|i| &i.provider == link) {
                    edges.push(
                        DetailMap::default()
                            .text("kind", "link")
                            .text("from", document.as_str())
                            .text("to", link.as_str())
                            .build(),
                    );
                }
            }
        }
    }
    let report = GraphReport { nodes, edges };
    if !snapshot.structural_errors().is_empty() {
        return Err(
            AppError::many(ExitClass::Validation, snapshot.diagnostics.clone())
                .with_data(report.to_detail()),
        );
    }
    Ok(Outcome::new(report, snapshot.non_error_diagnostics()))
}
