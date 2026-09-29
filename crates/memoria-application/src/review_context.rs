//! Build the complete review context `C` that a v3 token binds.
//!
//! The context reaches beyond the input manifest: it names the document's
//! kind and handoffs, the effective selection over its scope, the advisory
//! mapping associations, the effective guidance, and the provider closure
//! the reviewer depended on. Incoming handoffs and overlaps are not bound,
//! because they do not change this document's inputs.
//! Every collection is gathered here and sorted by the canonical encoder, so
//! traversal order never reaches the token.

use std::collections::{BTreeMap, BTreeSet};

use memoria_domain::canonical::{
    self, ConsumerEdge, HandoffEdge, ImportEdge, ProviderDescriptor, ReviewContext,
};
use memoria_domain::section::SELECTION_VERSION;
use memoria_domain::{DocumentId, Hash64};

use crate::ports::FingerprintHasher;
use crate::snapshot::Snapshot;

/// The three digests a v3 token combines, plus the context they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInputs {
    /// `I`: the complete input manifest fingerprint.
    pub inputs_digest: Hash64,
    /// `B`: the complete prior review record, or its absence.
    pub baseline_digest: Hash64,
    /// `C`: the review context fingerprint.
    pub context_digest: Hash64,
    pub context: ReviewContext,
}

/// Every import edge one document declares, with the provider's current
/// export body hash.
fn import_edges(snapshot: &Snapshot, document: &DocumentId) -> Vec<ImportEdge> {
    let Some(manifest) = snapshot.manifests.get(document) else {
        return Vec::new();
    };
    manifest
        .imports()
        .iter()
        .map(|import| ImportEdge {
            provider: import.document.as_str().to_string(),
            export_id: import.export_id.as_str().to_string(),
            hash: import.hash,
        })
        .collect()
}

/// The transitive provider closure of `document`, excluding the document.
fn provider_closure(snapshot: &Snapshot, document: &DocumentId) -> Vec<DocumentId> {
    let Some(graph) = snapshot.graph.as_ref() else {
        return Vec::new();
    };
    let mut seen: BTreeSet<DocumentId> = BTreeSet::new();
    let mut frontier = vec![document.clone()];
    while let Some(current) = frontier.pop() {
        for provider in graph.provider_documents(&current) {
            if provider != *document && seen.insert(provider.clone()) {
                frontier.push(provider);
            }
        }
    }
    seen.into_iter().collect()
}

/// Assemble the context for one document from a stable snapshot.
pub fn build(
    hasher: &dyn FingerprintHasher,
    snapshot: &Snapshot,
    document: &DocumentId,
) -> ReviewContext {
    // Scope: the handoffs that shape this document's coverage, and nested
    // repositories inside its covered folders, whose contents are opaque.
    let handoffs: Vec<HandoffEdge> = snapshot
        .scopes
        .handoffs_of(document)
        .iter()
        .map(|handoff| HandoffEdge {
            subtree: handoff.subtree.as_str().to_string(),
            target: handoff.target.as_str().to_string(),
        })
        .collect();
    let nested_repositories: Vec<String> = snapshot
        .collected
        .boundaries
        .iter()
        .filter(|path| snapshot.scopes.covers_dir(document, &path.directory()))
        .map(|path| path.as_str().to_string())
        .collect();

    // Selection: the effective policy and the paths it actually selected.
    let policy_hash = snapshot
        .policy_hashes
        .get(document)
        .copied()
        .unwrap_or(Hash64(0));
    let scope_paths: Vec<String> = snapshot
        .scopes
        .scope_of(document)
        .iter()
        .map(|path| path.as_str().to_string())
        .collect();

    // Mapping, guidance, and the graph.
    let mapping = snapshot
        .sections
        .get(document)
        .cloned()
        .unwrap_or_default()
        .identity();
    let guidance = snapshot.guidance_of(document).digest;
    let imports = import_edges(snapshot, document);
    let consumer_edges: Vec<ConsumerEdge> =
        match (&snapshot.graph, snapshot.documents.get(document)) {
            (Some(graph), Some(parsed)) => parsed
                .exports
                .iter()
                .flat_map(|export| {
                    graph
                        .consumers_of_export(document, &export.id)
                        .into_iter()
                        .map(|consumer| ConsumerEdge {
                            export_id: export.id.as_str().to_string(),
                            consumer: consumer.as_str().to_string(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            _ => Vec::new(),
        };
    let providers: Vec<ProviderDescriptor> = provider_closure(snapshot, document)
        .into_iter()
        .map(|provider| ProviderDescriptor {
            inputs_digest: snapshot
                .manifests
                .get(&provider)
                .map(|manifest| hasher.hash(&canonical::encode_inputs(manifest)))
                .unwrap_or(Hash64(0)),
            guidance_digest: snapshot.guidance_of(&provider).digest.0,
            review_revision: snapshot.state.document_revision(&provider),
            active_invalidations: snapshot
                .state
                .active_invalidations_for(&provider)
                .iter()
                .map(|inv| (inv.id, inv.reason.as_str().to_string()))
                .collect(),
            imports: import_edges(snapshot, &provider),
            document: provider.as_str().to_string(),
        })
        .collect();

    ReviewContext {
        selection_version: SELECTION_VERSION,
        owner: document.as_str().to_string(),
        document_kind: document.kind().as_str().to_string(),
        handoffs,
        nested_repositories,
        policy_hash,
        scope_paths,
        mapping,
        guidance,
        imports,
        consumer_edges,
        providers,
    }
}

/// Compute `I`, `B`, and `C` for one document.
pub fn token_inputs(
    hasher: &dyn FingerprintHasher,
    snapshot: &Snapshot,
    document: &DocumentId,
) -> TokenInputs {
    let context = build(hasher, snapshot, document);
    let inputs_digest = snapshot
        .manifests
        .get(document)
        .map(|manifest| hasher.hash(&canonical::encode_inputs(manifest)))
        .unwrap_or(Hash64(0));
    let baseline_digest = hasher.hash(&canonical::encode_review_baseline(
        snapshot.state.reviews.get(document),
    ));
    let context_digest = hasher.hash(&canonical::encode_review_context(&context));
    TokenInputs {
        inputs_digest,
        baseline_digest,
        context_digest,
        context,
    }
}

/// Canonical descriptors for the `binding` object of a full export, so an
/// offline reader can recompute `C` without the project.
pub fn context_detail(context: &ReviewContext) -> crate::error::Detail {
    use crate::error::{Detail, DetailMap};
    use memoria_domain::SectionMapIdentity;
    let strings = |values: &[String]| {
        let mut sorted: Vec<String> = values.to_vec();
        sorted.sort();
        Detail::texts(sorted)
    };
    let mapping = match &context.mapping {
        SectionMapIdentity::Absent => Detail::text("absent"),
        SectionMapIdentity::Invalid => Detail::text("invalid"),
        SectionMapIdentity::Valid(pairs) => Detail::list(pairs.iter().map(|(id, sources)| {
            DetailMap::default()
                .text("id", id.clone())
                .with("sources", Detail::texts(sources.clone()))
                .build()
        })),
    };
    let edges = |edges: &[ImportEdge]| {
        let mut sorted: Vec<&ImportEdge> = edges.iter().collect();
        sorted.sort();
        Detail::list(sorted.into_iter().map(|edge| {
            DetailMap::default()
                .text("provider", edge.provider.clone())
                .text("export_id", edge.export_id.clone())
                .text("hash", edge.hash.to_hex())
                .build()
        }))
    };
    let mut consumers: Vec<&ConsumerEdge> = context.consumer_edges.iter().collect();
    consumers.sort();
    let mut providers: Vec<&ProviderDescriptor> = context.providers.iter().collect();
    providers.sort();
    DetailMap::default()
        .number("selection_version", context.selection_version)
        .text("owner", context.owner.clone())
        .text("document_kind", context.document_kind.clone())
        .with("handoffs", {
            let mut sorted: Vec<&HandoffEdge> = context.handoffs.iter().collect();
            sorted.sort();
            Detail::list(sorted.into_iter().map(|edge| {
                DetailMap::default()
                    .text("subtree", edge.subtree.clone())
                    .text("target", edge.target.clone())
                    .build()
            }))
        })
        .with("nested_repositories", strings(&context.nested_repositories))
        .text("policy_hash", context.policy_hash.to_hex())
        .with("scope_paths", strings(&context.scope_paths))
        .text(
            "mapping_state",
            match &context.mapping {
                SectionMapIdentity::Absent => "absent",
                SectionMapIdentity::Invalid => "invalid",
                SectionMapIdentity::Valid(_) => "valid",
            },
        )
        .with("mapping", mapping)
        .text("guidance_digest", context.guidance.to_hex())
        .with("imports", edges(&context.imports))
        .with(
            "consumer_edges",
            Detail::list(consumers.into_iter().map(|edge| {
                DetailMap::default()
                    .text("export_id", edge.export_id.clone())
                    .text("consumer", edge.consumer.clone())
                    .build()
            })),
        )
        .with(
            "providers",
            Detail::list(providers.into_iter().map(|provider| {
                let mut invalidations: Vec<&(u64, String)> =
                    provider.active_invalidations.iter().collect();
                invalidations.sort_by_key(|entry| entry.0);
                DetailMap::default()
                    .text("document", provider.document.clone())
                    .text("inputs_digest", provider.inputs_digest.to_hex())
                    .text("guidance_digest", provider.guidance_digest.to_hex())
                    .number("review_revision", provider.review_revision)
                    .with(
                        "active_invalidations",
                        Detail::list(invalidations.into_iter().map(|(id, reason)| {
                            DetailMap::default()
                                .number("id", *id)
                                .text("reason", reason.clone())
                                .build()
                        })),
                    )
                    .with("imports", edges(&provider.imports))
                    .build()
            })),
        )
        .build()
}

/// Ordered unique documents in the closure, for diagnostics.
pub fn provider_documents(snapshot: &Snapshot, document: &DocumentId) -> BTreeMap<String, u64> {
    provider_closure(snapshot, document)
        .into_iter()
        .map(|provider| {
            let revision = snapshot.state.document_revision(&provider);
            (provider.as_str().to_string(), revision)
        })
        .collect()
}
