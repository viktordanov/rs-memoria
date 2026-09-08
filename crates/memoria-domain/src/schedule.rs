//! Pending causes and readiness for every document.

use std::collections::BTreeMap;

use crate::graph::ImportGraph;
use crate::manifest::{Hash64, InputManifest, ManifestDiff};
use crate::path::DocumentId;
use crate::review::ReviewState;

/// Why a document needs review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingCause {
    NeverReviewed,
    InputChanged(ManifestDiff),
    DocumentChanged {
        before: (u64, Hash64),
        after: (u64, Hash64),
    },
    ExplicitInvalidation {
        id: u64,
        reason: String,
    },
}

impl PendingCause {
    pub fn code(&self) -> &'static str {
        match self {
            PendingCause::NeverReviewed => "never_reviewed",
            PendingCause::InputChanged(_) => "input_changed",
            PendingCause::DocumentChanged { .. } => "document_changed",
            PendingCause::ExplicitInvalidation { .. } => "explicit_invalidation",
        }
    }
}

/// Review status and readiness of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStatus {
    pub document: DocumentId,
    pub revision: u64,
    pub causes: Vec<PendingCause>,
    /// Direct providers that are pending or themselves waiting, sorted.
    pub waiting_on: Vec<DocumentId>,
}

impl DocumentStatus {
    pub fn pending(&self) -> bool {
        !self.causes.is_empty()
    }

    pub fn waiting(&self) -> bool {
        !self.waiting_on.is_empty()
    }

    /// Pending and every dependency is current and not waiting.
    pub fn ready(&self) -> bool {
        self.pending() && !self.waiting()
    }

    pub fn current(&self) -> bool {
        !self.pending()
    }
}

/// Compute status for every document in the graph's dependency order.
pub fn schedule(
    graph: &ImportGraph,
    state: &ReviewState,
    manifests: &BTreeMap<DocumentId, InputManifest>,
) -> Vec<DocumentStatus> {
    let mut statuses: BTreeMap<DocumentId, DocumentStatus> = BTreeMap::new();
    let mut ordered = Vec::new();
    for document in graph.order() {
        let mut causes = Vec::new();
        let revision = state.document_revision(document);
        match (state.reviews.get(document), manifests.get(document)) {
            (None, _) => causes.push(PendingCause::NeverReviewed),
            (Some(record), Some(current)) => {
                let diff = record.manifest.diff(current);
                if diff.inputs_changed() {
                    let mut inputs_only = diff.clone();
                    inputs_only.document = None;
                    causes.push(PendingCause::InputChanged(inputs_only));
                }
                if let Some((before, after)) = diff.document {
                    causes.push(PendingCause::DocumentChanged { before, after });
                }
            }
            (Some(_), None) => causes.push(PendingCause::NeverReviewed),
        }
        for invalidation in state.active_invalidations_for(document) {
            causes.push(PendingCause::ExplicitInvalidation {
                id: invalidation.id,
                reason: invalidation.reason.as_str().to_string(),
            });
        }
        let waiting_on: Vec<DocumentId> = graph
            .provider_documents(document)
            .into_iter()
            .filter(|provider| {
                statuses
                    .get(provider)
                    .is_none_or(|s| s.pending() || s.waiting())
            })
            .collect();
        let status = DocumentStatus {
            document: document.clone(),
            revision,
            causes,
            waiting_on,
        };
        statuses.insert(document.clone(), status.clone());
        ordered.push(status);
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ByteRange, Document, Export, ExportId, Import, SourceLocation};
    use crate::manifest::{FileInput, ImportInput};
    use crate::path::ProjectPath;
    use crate::review::{
        AckRequest, GitContext, InvalidationScope, Reason, ReviewNote, ReviewResult, ReviewerName,
        Timestamp,
    };

    fn doc(id: &str, imports: &[&str]) -> (DocumentId, Document) {
        let id = DocumentId::parse(id).unwrap();
        (
            id.clone(),
            Document {
                id: id.clone(),
                exports: vec![Export {
                    id: ExportId::parse("summary").unwrap(),
                    body: ByteRange::new(0, 0),
                    location: SourceLocation { line: 1, column: 1 },
                }],
                imports: imports
                    .iter()
                    .map(|t| Import {
                        provider: DocumentId::parse(t).unwrap(),
                        export_id: ExportId::parse("summary").unwrap(),
                        source_text: t.to_string(),
                        body: ByteRange::new(0, 0),
                        location: SourceLocation { line: 2, column: 1 },
                    })
                    .collect(),
                links: vec![],
            },
        )
    }

    fn manifest(d: &str, file_hash: u64, imports: &[(&str, u64)]) -> InputManifest {
        InputManifest::new(
            DocumentId::parse(d).unwrap(),
            Hash64(1),
            3,
            Hash64(3),
            vec![FileInput {
                path: ProjectPath::parse(&format!(
                    "{}/x.rs",
                    d.trim_end_matches("README.md").trim_end_matches('/')
                ))
                .unwrap_or(ProjectPath::parse("x.rs").unwrap()),
                bytes: 1,
                hash: Hash64(file_hash),
            }],
            imports
                .iter()
                .map(|(p, h)| ImportInput {
                    document: DocumentId::parse(p).unwrap(),
                    export_id: ExportId::parse("summary").unwrap(),
                    bytes: 2,
                    hash: Hash64(*h),
                })
                .collect(),
        )
        .unwrap()
    }

    fn ack(state: &mut ReviewState, m: &InputManifest) {
        state
            .acknowledge(AckRequest {
                document: m.document.clone(),
                packet_revision: state.document_revision(&m.document),
                packet_manifest: m.clone(),
                current_manifest: m.clone(),
                covered: vec![],
                input_fingerprint: Hash64(0),
                token_digest: Hash64(0),
                guidance: crate::GuidanceDigest::default(),
                reviewed_at: Timestamp("t".into()),
                reviewer: ReviewerName::parse("fixture").unwrap(),
                result: ReviewResult::NoUpdate,
                note: ReviewNote::parse("The current summary describes all reviewed inputs.")
                    .unwrap(),
                git: GitContext::default(),
            })
            .unwrap();
    }

    fn three_level() -> ImportGraph {
        ImportGraph::build(
            &vec![
                doc("README.md", &["src/retrieval/README.md"]),
                doc(
                    "src/retrieval/README.md",
                    &["src/retrieval/naive/README.md"],
                ),
                doc("src/retrieval/naive/README.md", &[]),
                doc("src/other/README.md", &[]),
            ]
            .into_iter()
            .collect(),
        )
        .unwrap()
    }

    #[test]
    fn source_change_makes_owner_pending_and_consumers_wait_without_staleness() {
        let graph = three_level();
        let mut manifests: BTreeMap<DocumentId, InputManifest> = BTreeMap::new();
        manifests.insert(
            DocumentId::parse("src/retrieval/naive/README.md").unwrap(),
            manifest("src/retrieval/naive/README.md", 1, &[]),
        );
        manifests.insert(
            DocumentId::parse("src/retrieval/README.md").unwrap(),
            manifest(
                "src/retrieval/README.md",
                1,
                &[("src/retrieval/naive/README.md", 7)],
            ),
        );
        manifests.insert(
            DocumentId::parse("README.md").unwrap(),
            manifest("README.md", 1, &[("src/retrieval/README.md", 8)]),
        );
        manifests.insert(
            DocumentId::parse("src/other/README.md").unwrap(),
            manifest("src/other/README.md", 1, &[]),
        );
        let mut state = ReviewState::empty();
        for m in manifests.values() {
            ack(&mut state, m);
        }
        let all_current = schedule(&graph, &state, &manifests);
        assert!(all_current.iter().all(|s| s.current() && !s.waiting()));

        // Change naive's source file only.
        manifests.insert(
            DocumentId::parse("src/retrieval/naive/README.md").unwrap(),
            manifest("src/retrieval/naive/README.md", 2, &[]),
        );
        let statuses = schedule(&graph, &state, &manifests);
        let by: BTreeMap<&str, &DocumentStatus> =
            statuses.iter().map(|s| (s.document.as_str(), s)).collect();
        assert!(by["src/retrieval/naive/README.md"].ready());
        assert!(matches!(
            by["src/retrieval/naive/README.md"].causes[0],
            PendingCause::InputChanged(_)
        ));
        assert!(by["src/retrieval/README.md"].current());
        assert!(by["src/retrieval/README.md"].waiting());
        assert!(by["README.md"].current());
        assert!(by["README.md"].waiting());
        assert!(by["src/other/README.md"].current() && !by["src/other/README.md"].waiting());

        // Acknowledge naive with unchanged export: consumers stop waiting.
        ack(
            &mut state,
            &manifests[&DocumentId::parse("src/retrieval/naive/README.md").unwrap()],
        );
        let statuses = schedule(&graph, &state, &manifests);
        assert!(statuses.iter().all(|s| s.current() && !s.waiting()));

        // Naive's export changes: only retrieval's import input changes.
        manifests.insert(
            DocumentId::parse("src/retrieval/README.md").unwrap(),
            manifest(
                "src/retrieval/README.md",
                1,
                &[("src/retrieval/naive/README.md", 9)],
            ),
        );
        let statuses = schedule(&graph, &state, &manifests);
        let by: BTreeMap<&str, &DocumentStatus> =
            statuses.iter().map(|s| (s.document.as_str(), s)).collect();
        assert!(by["src/retrieval/README.md"].ready());
        assert!(by["README.md"].current() && by["README.md"].waiting());
    }

    #[test]
    fn explicit_invalidation_is_a_separate_cause() {
        let graph = three_level();
        let mut manifests: BTreeMap<DocumentId, InputManifest> = BTreeMap::new();
        for d in [
            "README.md",
            "src/retrieval/README.md",
            "src/retrieval/naive/README.md",
            "src/other/README.md",
        ] {
            manifests.insert(DocumentId::parse(d).unwrap(), manifest(d, 1, &[]));
        }
        let mut state = ReviewState::empty();
        for m in manifests.values() {
            ack(&mut state, m);
        }
        state
            .add_invalidation(
                InvalidationScope::All,
                Reason::parse("Use Simplified English everywhere.").unwrap(),
                vec![DocumentId::parse("src/other/README.md").unwrap()],
                Timestamp("t".into()),
            )
            .unwrap();
        let statuses = schedule(&graph, &state, &manifests);
        let other = statuses
            .iter()
            .find(|s| s.document.as_str() == "src/other/README.md")
            .unwrap();
        assert_eq!(
            other.causes,
            vec![PendingCause::ExplicitInvalidation {
                id: 1,
                reason: "Use Simplified English everywhere.".into()
            }]
        );
        assert!(statuses.iter().filter(|s| s.pending()).count() == 1);
    }
}
