//! Decide what one review must read, and why it cannot read less.
//!
//! Focused reading is a candidate only when the whole picture holds: a prior
//! declaration exists, the bytes that declaration named can still be
//! verified, the mapping associations are valid and unchanged, and nothing
//! outside the mapped sources moved. Any doubt produces a full baseline with
//! an explicit reason. Nothing here narrows what acknowledgement validates.

use memoria_domain::{
    CoverageEvidence, DirPath, DocumentId, InputChange, InputManifest, ProjectPath, ReviewRecord,
};

use crate::error::AppError;
use crate::packet::{ChangeEntry, compute_token_v3};
use crate::ports::Services;
use crate::review::{
    BaselineInfo, CoCovering, ConsumerInfo, Downstream, EvidenceStatus, FallbackReason,
    GuidanceReference, IncomingHandoff, InputEntry, InputRole, MAX_LIST_ENTRIES, Relationship,
    RelationshipKind, ReviewManifest, ReviewMode, ScopeHandoff, ScopeInfo, SectionSuggestion,
    SnapshotDigests,
};
use crate::review_context;
use crate::snapshot::Snapshot;

use super::{diff_changes, status_label};

/// One historical body a focused comparison needs.
struct Needed {
    path: String,
    export: Option<String>,
    identity: String,
    expected: (u64, memoria_domain::Hash64),
}

/// Whether every needed old body could be verified against the record.
struct Evidence {
    status: EvidenceStatus,
    /// The first reason code that explains a missing body.
    reason: Option<String>,
    /// Previous document bytes, when they were recovered.
    previous_readme: Option<Vec<u8>>,
}

fn reason(code: &str, identity: Option<&str>, message: impl Into<String>) -> FallbackReason {
    FallbackReason {
        code: code.to_string(),
        identity: identity.map(str::to_string),
        message: message.into(),
    }
}

/// Collect the historical bodies a focused comparison would need, then look
/// each one up under one shared per-command budget.
///
/// The previous document text is always needed, because mapping
/// associations are compared against it even when only sources changed. An
/// unchanged current document supplies those exact bytes without touching
/// local history.
fn gather_evidence(
    services: &Services<'_>,
    snapshot: &Snapshot,
    document: &DocumentId,
    record: &ReviewRecord,
    current: &InputManifest,
) -> Evidence {
    let diff = record.manifest.diff(current);
    let mut needed: Vec<Needed> = Vec::new();
    let previous_readme_unchanged = record.manifest.document_hash == current.document_hash
        && record.manifest.document_bytes == current.document_bytes;
    if !previous_readme_unchanged {
        needed.push(Needed {
            path: document.as_str().to_string(),
            export: None,
            identity: document.as_str().to_string(),
            expected: (
                record.manifest.document_bytes,
                record.manifest.document_hash,
            ),
        });
    }
    for change in &diff.files {
        if let InputChange::Changed { before, .. } | InputChange::Removed(before) = change {
            needed.push(Needed {
                path: before.path.as_str().to_string(),
                export: None,
                identity: before.path.as_str().to_string(),
                expected: (before.bytes, before.hash),
            });
        }
    }
    for change in &diff.imports {
        if let InputChange::Changed { before, .. } | InputChange::Removed(before) = change {
            needed.push(Needed {
                path: before.document.as_str().to_string(),
                export: Some(before.export_id.as_str().to_string()),
                identity: format!("{}#{}", before.document, before.export_id),
                expected: (before.bytes, before.hash),
            });
        }
    }

    let mut previous_readme = if previous_readme_unchanged {
        Some(snapshot.document_bytes(document).to_vec())
    } else {
        None
    };
    if needed.is_empty() {
        return Evidence {
            status: EvidenceStatus::Verified,
            reason: None,
            previous_readme,
        };
    }
    // One shared budget for the whole command, not one budget per input.
    let mut history = super::history::History::new(services);
    let total = needed.len();
    let mut found = 0usize;
    let mut first_reason: Option<String> = None;
    for item in &needed {
        let baseline = history.lookup(
            record.git.base_commit.as_deref(),
            &item.path,
            item.export.as_deref(),
            item.expected,
        );
        match baseline.bytes {
            Some(bytes) => {
                found += 1;
                if item.export.is_none() && item.path == document.as_str() {
                    previous_readme = Some(bytes);
                }
            }
            None => {
                if first_reason.is_none() {
                    let code = baseline.reason_code.unwrap_or("blob_unavailable");
                    let text = baseline.reason.unwrap_or_default();
                    first_reason = Some(format!("{} ({code}): {text}", item.identity));
                }
            }
        }
    }
    let status = if found == total {
        EvidenceStatus::Verified
    } else if found == 0 {
        EvidenceStatus::Unavailable
    } else {
        EvidenceStatus::Partial
    };
    Evidence {
        status,
        reason: first_reason,
        previous_readme,
    }
}

/// Index every guidance entry inside its declaring inline or file list. A
/// section guide is indexed among the document's section guide entries.
fn guidance_references(guidance: &crate::guidance::EffectiveGuidance) -> Vec<GuidanceReference> {
    let mut counters: std::collections::BTreeMap<(String, &'static str), u64> =
        std::collections::BTreeMap::new();
    guidance
        .entries
        .iter()
        .map(|entry| {
            let kind = entry.kind.as_str();
            let section = entry.kind == memoria_domain::GuidanceKind::Section;
            let key = if section {
                String::new()
            } else {
                entry.source.clone()
            };
            let counter = counters.entry((key, kind)).or_insert(0);
            let entry_index = *counter;
            *counter += 1;
            GuidanceReference {
                scope: entry.scope.as_str().to_string(),
                source: entry.source.clone(),
                kind: kind.to_string(),
                entry_index,
                sections: section.then(|| entry.sections.iter().map(|s| s.id.clone()).collect()),
            }
        })
        .collect()
}

/// Build the complete review requirements for a ready document.
pub fn build(
    services: &Services<'_>,
    snapshot: &Snapshot,
    document: &DocumentId,
) -> Result<ReviewManifest, AppError> {
    let current = snapshot.manifests.get(document).cloned().ok_or_else(|| {
        AppError::validation("document_not_found", format!("{document} has no manifest"))
    })?;
    let hasher = services.hasher;
    let guidance = snapshot.guidance_of(document);
    let covered: Vec<(u64, String)> = snapshot
        .state
        .active_invalidations_for(document)
        .iter()
        .map(|inv| (inv.id, inv.reason.as_str().to_string()))
        .collect();
    let review_revision = snapshot.state.document_revision(document);
    let bound = review_context::token_inputs(hasher, snapshot, document);
    let token = compute_token_v3(
        hasher,
        document,
        review_revision,
        bound.inputs_digest,
        bound.baseline_digest,
        bound.context_digest,
        &covered,
    );

    let record = snapshot.state.reviews.get(document).cloned();
    let mut fallbacks: Vec<FallbackReason> = Vec::new();
    let mut changes: Vec<ChangeEntry> = Vec::new();
    let mut relationships: Vec<Relationship> = Vec::new();
    // `changed_*` drives the section suggestions and the unmapped-change
    // fallback. `read_*` is what the reviewer must actually open now, so it
    // also holds identities that were added since the last review.
    let mut changed_sources: Vec<memoria_domain::ProjectPath> = Vec::new();
    let mut changed_imports: Vec<(DocumentId, memoria_domain::ExportId)> = Vec::new();
    let mut read_sources: Vec<memoria_domain::ProjectPath> = Vec::new();
    let mut read_imports: Vec<(DocumentId, memoria_domain::ExportId)> = Vec::new();
    let sections = snapshot.sections_of(document).clone();

    // The current mapping must be usable before any advice can be given.
    if !sections.is_valid() {
        fallbacks.push(reason(
            "mapping_invalid",
            Some(document.as_str()),
            "the document's section mappings are invalid, so none of its advice can narrow this review; see the section_mapping_invalid diagnostics",
        ));
    }
    if snapshot.guidance_changed(document) == Some(true) {
        fallbacks.push(reason(
            "guidance_changed",
            Some(document.as_str()),
            "project documentation guidance changed since the last review; read the current guidance in full",
        ));
    }
    for (id, text) in &covered {
        fallbacks.push(reason(
            "semantic_invalidation",
            Some(document.as_str()),
            format!("invalidation {id} requires a semantic review: {text}"),
        ));
    }

    let mut baseline = None;
    match &record {
        None => fallbacks.push(reason(
            "baseline_missing",
            Some(document.as_str()),
            "this document has no previous review to compare against; review its complete scope",
        )),
        Some(record) => {
            let evidence = gather_evidence(services, snapshot, document, record, &current);
            baseline = Some(BaselineInfo {
                revision: record.revision,
                token_digest: record.token_digest,
                reviewer: record.reviewer.as_str().to_string(),
                result: record.result.as_str().to_string(),
                recorded_commit: record.git.base_commit.clone(),
                evidence_status: evidence.status,
            });
            if evidence.status != EvidenceStatus::Verified {
                fallbacks.push(reason(
                    "baseline_unavailable",
                    Some(document.as_str()),
                    evidence.reason.clone().unwrap_or_else(|| {
                        "the reviewed bytes needed for a focused comparison could not be verified"
                            .to_string()
                    }),
                ));
            }
            if record.manifest.policy_hash != current.policy_hash {
                fallbacks.push(reason(
                    "policy_changed",
                    Some(document.as_str()),
                    "the effective selection policy changed since the last review; review the complete current scope",
                ));
            }
            let diff = record.manifest.diff(&current);
            let handed_now = snapshot.scopes.handed_off_subtrees(document);
            // The folders this document's previous review excluded through
            // handoffs: recorded at acknowledgement, or, for a record
            // without evidence, proven from its own token when possible.
            // Needed only when a source entered the scope.
            let entered = diff
                .files
                .iter()
                .any(|change| matches!(change, InputChange::Added(_)));
            let former_coverage: Result<Vec<DirPath>, &'static str> = if !entered {
                Ok(Vec::new())
            } else {
                match &record.coverage {
                    CoverageEvidence::Recorded(folders) => Ok(folders.clone()),
                    CoverageEvidence::Unrecorded => match proven_former_exclusions(
                        services,
                        snapshot,
                        document,
                        record,
                        evidence.previous_readme.as_deref(),
                    ) {
                        Proof::Proven(folders) => Ok(folders),
                        Proof::Unavailable(reason) => Err(reason),
                    },
                }
            };
            let classify = |change: &InputChange<memoria_domain::FileInput>| -> Classified {
                match change {
                    InputChange::Changed { .. } => Classified::Source,
                    InputChange::Removed(file) => {
                        if snapshot.is_document(&file.path) {
                            Classified::Classification
                        } else if let Some(subtree) =
                            handed_now.iter().find(|s| file.path.is_within(s))
                        {
                            Classified::Handoff(subtree.clone())
                        } else {
                            Classified::PathSet
                        }
                    }
                    InputChange::Added(file) => {
                        let was_document = DocumentId::from_path(file.path.clone())
                            .is_ok_and(|id| snapshot.state.reviews.contains_key(&id));
                        if was_document {
                            return Classified::Classification;
                        }
                        match &former_coverage {
                            // Sorted folders put an ancestor before its
                            // descendants, so the first match is the
                            // shallowest handoff that no longer applies.
                            Ok(folders) => match folders.iter().find(|s| file.path.is_within(s)) {
                                Some(subtree) => Classified::Handoff(subtree.clone()),
                                None => Classified::PathSet,
                            },
                            Err(reason) => Classified::Unrecorded(reason),
                        }
                    }
                }
            };
            for change in diff_changes(&diff) {
                changes.push(ChangeEntry {
                    kind: change.kind.to_string(),
                    change: change.change.to_string(),
                    identity: change.identity.clone(),
                    before_bytes: change.before.map(|b| b.0),
                    before_hash: change.before.map(|b| b.1),
                    after_bytes: change.after.map(|a| a.0),
                    after_hash: change.after.map(|a| a.1),
                });
                relationships.push(match change.kind {
                    "policy" => Relationship::of_kind(RelationshipKind::SelectionPolicy),
                    "document" => Relationship::of_kind(RelationshipKind::OwnText),
                    "import" => {
                        let (provider, export_id) = change
                            .identity
                            .split_once('#')
                            .map(|(p, e)| (p.to_string(), e.to_string()))
                            .unwrap_or_else(|| (change.identity.clone(), String::new()));
                        Relationship {
                            provider: Some(provider),
                            export_id: Some(export_id),
                            ..Relationship::of_kind(RelationshipKind::Import)
                        }
                    }
                    _ => {
                        let path = ProjectPath::parse(&change.identity).ok();
                        let file_change = path.as_ref().and_then(|path| {
                            diff.files.iter().find(|c| match c {
                                InputChange::Added(f) | InputChange::Removed(f) => &f.path == path,
                                InputChange::Changed { after, .. } => &after.path == path,
                            })
                        });
                        let classified = file_change.map(&classify);
                        let kind = match classified {
                            Some(Classified::Handoff(_)) => RelationshipKind::Handoff,
                            Some(Classified::Unrecorded(_)) => RelationshipKind::CoverageUnrecorded,
                            _ => RelationshipKind::ScopeSource,
                        };
                        let mut relationship =
                            source_relationship(snapshot, document, &sections, path.as_ref(), kind);
                        if let Some(Classified::Unrecorded(reason)) = classified {
                            relationship.unrecorded_reason = Some(reason.to_string());
                        }
                        relationship
                    }
                });
            }
            for change in &diff.files {
                match change {
                    InputChange::Changed { after, .. } => {
                        changed_sources.push(after.path.clone());
                        read_sources.push(after.path.clone());
                    }
                    InputChange::Added(file) => {
                        // The file exists now, so the review must read it. It
                        // has no previous association, so it never becomes a
                        // section suggestion or an unmapped-change reason.
                        read_sources.push(file.path.clone());
                        fallbacks.push(match classify(change) {
                            Classified::Handoff(subtree) => reason(
                                "handoff_changed",
                                Some(subtree.as_str()),
                                format!(
                                    "a handoff of {subtree}/ no longer applies, so its sources entered this document's scope; 0.7 hands a folder to a document only when this document links or imports it. Review the complete current scope, or link the document there to hand the folder off"
                                ),
                            ),
                            Classified::Classification => reason(
                                "document_classification_changed",
                                Some(file.path.as_str()),
                                "a Markdown file lost its last Memoria marker and is now an ordinary source in this document's scope; review the complete current scope",
                            ),
                            Classified::Unrecorded(why) => reason(
                                "coverage_unrecorded",
                                Some(file.path.as_str()),
                                format!(
                                    "`{}` entered this document's scope. This document was last reviewed by a release that did not record handed-off folders, so Memoria cannot tell whether a handoff ended or the source is new (`{why}`). Review the complete current scope. The next acknowledgement records the coverage.",
                                    file.path
                                ),
                            ),
                            _ => reason(
                                "path_set_changed",
                                Some(file.path.as_str()),
                                "a source was added to this document's scope; a rename appears as a removal plus an addition",
                            ),
                        });
                    }
                    InputChange::Removed(file) => {
                        fallbacks.push(match classify(change) {
                            Classified::Handoff(subtree) => reason(
                                "handoff_changed",
                                Some(subtree.as_str()),
                                format!(
                                    "{subtree}/ is now handed off to a tracked document there, so its sources left this document's scope; make sure that this document no longer claims detail that the other document now covers"
                                ),
                            ),
                            Classified::Classification => reason(
                                "document_classification_changed",
                                Some(file.path.as_str()),
                                "a source gained a Memoria marker and is now a tracked document, so it left this document's scope; review the complete current scope",
                            ),
                            _ => reason(
                                "path_set_changed",
                                Some(file.path.as_str()),
                                "a source left this document's scope; a rename appears as a removal plus an addition",
                            ),
                        });
                    }
                }
            }
            for change in &diff.imports {
                let identity = match change {
                    // A removed import has no current body to read. It stays
                    // in `changes` and in the fallback reasons only.
                    InputChange::Removed(i) => format!("{}#{}", i.document, i.export_id),
                    InputChange::Added(i) => {
                        read_imports.push((i.document.clone(), i.export_id.clone()));
                        format!("{}#{}", i.document, i.export_id)
                    }
                    InputChange::Changed { after, .. } => {
                        changed_imports.push((after.document.clone(), after.export_id.clone()));
                        read_imports.push((after.document.clone(), after.export_id.clone()));
                        format!("{}#{}", after.document, after.export_id)
                    }
                };
                fallbacks.push(reason(
                    "imports_changed",
                    Some(&identity),
                    "an imported contract changed; review the provider first, render, then review this document against its current scope",
                ));
            }
            // Mapping associations are compared against the previous text.
            // Without those bytes the comparison is impossible, and the
            // missing-evidence reason above already forces a full baseline.
            if let Some(previous) = &evidence.previous_readme {
                let previous_map = snapshot.section_map_for_bytes(services, document, previous);
                if previous_map.identity() != sections.identity() {
                    fallbacks.push(reason(
                        "mapping_changed",
                        Some(document.as_str()),
                        "the document's section mappings changed since the last review; a changed association cannot reduce the required scope",
                    ));
                }
            }
        }
    }

    // Every changed source must be described by at least one valid section.
    for path in &changed_sources {
        if sections.sections_for(path).is_empty() {
            fallbacks.push(reason(
                "unmapped_change",
                Some(path.as_str()),
                "no valid section describes this changed source, so the review cannot narrow to a part of the document",
            ));
        }
    }

    fallbacks.sort();
    fallbacks.dedup();
    let mode = if fallbacks.is_empty() {
        ReviewMode::FocusedCandidate
    } else {
        ReviewMode::FullBaseline
    };

    // Suggestions exist only when focused reading is a candidate. A full
    // baseline reports no sections at all.
    let suggestions: Vec<SectionSuggestion> = if mode == ReviewMode::FocusedCandidate {
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for path in &changed_sources {
            for section in sections.sections_for(path) {
                let id = section.id.as_str().to_string();
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id.clone());
                out.push(SectionSuggestion {
                    id,
                    heading: section.heading.clone(),
                    first_line: section.first_line,
                    last_line: section.last_line,
                    sources: section
                        .sources
                        .iter()
                        .map(|p| p.as_str().to_string())
                        .collect(),
                    guidance: section.guidance.as_ref().map(|p| p.as_str().to_string()),
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    } else {
        Vec::new()
    };

    // Suggested reads: the whole document, everything that changed, and the
    // context of every suggested section. This is advice, not the complete
    // scope inventory.
    let mut inputs: Vec<InputEntry> = vec![InputEntry {
        kind: "document".into(),
        path: document.as_str().to_string(),
        export_id: None,
        bytes: current.document_bytes,
        hash: current.document_hash,
        role: InputRole::WholeDocument,
    }];
    let push_file =
        |inputs: &mut Vec<InputEntry>, path: &memoria_domain::ProjectPath, role: InputRole| {
            let Some(file) = current.files().iter().find(|f| &f.path == path) else {
                return;
            };
            match inputs.iter_mut().find(|e| e.path == path.as_str()) {
                // Deduplicate identities and keep the strongest role.
                Some(existing) => existing.role = existing.role.min(role),
                None => inputs.push(InputEntry {
                    kind: "file".into(),
                    path: path.as_str().to_string(),
                    export_id: None,
                    bytes: file.bytes,
                    hash: file.hash,
                    role,
                }),
            }
        };
    for path in &read_sources {
        push_file(&mut inputs, path, InputRole::ChangedSource);
    }
    for suggestion in &suggestions {
        for source in &suggestion.sources {
            if let Ok(path) = memoria_domain::ProjectPath::parse(source) {
                push_file(&mut inputs, &path, InputRole::SectionContext);
            }
        }
    }
    read_imports.sort();
    read_imports.dedup();
    for (provider, export_id) in &read_imports {
        if inputs.iter().any(|e| {
            e.path == provider.as_str() && e.export_id.as_deref() == Some(export_id.as_str())
        }) {
            continue;
        }
        if let Some(import) = current
            .imports()
            .iter()
            .find(|i| &i.document == provider && &i.export_id == export_id)
        {
            inputs.push(InputEntry {
                kind: "import".into(),
                path: provider.as_str().to_string(),
                export_id: Some(export_id.as_str().to_string()),
                bytes: import.bytes,
                hash: import.hash,
                role: InputRole::CurrentImport,
            });
        }
    }
    inputs.sort_by(|a, b| {
        (a.kind.as_str(), a.path.as_str(), a.export_id.as_deref()).cmp(&(
            b.kind.as_str(),
            b.path.as_str(),
            b.export_id.as_deref(),
        ))
    });

    Ok(ReviewManifest {
        document: document.clone(),
        review_revision,
        token,
        scope: scope_info(snapshot, document),
        snapshot: SnapshotDigests {
            inputs_digest: bound.inputs_digest,
            context_digest: bound.context_digest,
            guidance_digest: guidance.digest.0,
            baseline_digest: bound.baseline_digest,
            selection_version: bound.context.selection_version,
        },
        baseline,
        changes,
        relationships,
        mode,
        sections: suggestions,
        fallback_reasons: fallbacks,
        inputs,
        guidance_digest: guidance.digest.0,
        guidance_changed_since_review: snapshot.guidance_changed(document),
        guidance_references: guidance_references(&guidance),
        covered_invalidations: covered,
        downstream: downstream(snapshot, document),
        scope_files: current.files().len() as u64,
        imports: current.imports().len() as u64,
        raw_input_bytes: current.raw_input_bytes(),
        artifact_digest: String::new(),
    })
}

/// Most candidate handoffs or boundaries one proof enumerates. Each subset is
/// one exact token recomputation; beyond this bound the proof is not tried.
const MAX_PROOF_CANDIDATES: usize = 12;

/// The result of the exclusion proof for a record without coverage evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Proof {
    /// The exact subtrees the recorded review excluded.
    Proven(Vec<DirPath>),
    /// Why no exact proof exists: one of `UNRECORDED_REASONS`.
    Unavailable(&'static str),
}

/// The subtrees that this document's previous review provably excluded from
/// its scope, through a handoff (0.7) or a README boundary (0.6). Used only
/// for a record without coverage evidence.
///
/// The proof uses this document's own recorded token. A token binds the
/// complete review context, which names the handoffs (layout v2) or the
/// descendant README boundaries (0.6 layout v1) of that review. This helper
/// rebuilds the recorded token under each subset of the candidates: the
/// referenced documents, or the current descendant READMEs, whose subtree
/// the recorded manifest excludes. An exact digest match proves which
/// subtrees were excluded at that review.
///
/// The proof exists only when every other bound value is recoverable: the
/// record is the document's first review (its baseline is then known to be
/// absent), it covered no invalidation (their reasons are not stored), the
/// previous text is available, and the provider and consumer context is
/// unchanged. Otherwise the result names the first condition that failed,
/// and an added source reports `coverage_unrecorded`. Absence of source rows
/// alone never proves a former handoff, and a skipped layout is never a
/// silent "nothing was excluded".
fn proven_former_exclusions(
    services: &Services<'_>,
    snapshot: &Snapshot,
    document: &DocumentId,
    record: &ReviewRecord,
    previous: Option<&[u8]>,
) -> Proof {
    use memoria_domain::canonical::{
        HandoffEdge, ImportEdge, LegacyReviewContextV1, ReviewContext, encode_inputs,
        encode_legacy_absent_review_baseline_v1, encode_legacy_review_context_v1,
        encode_review_context,
    };

    if record.revision != 1 {
        return Proof::Unavailable("revision_not_first");
    }
    if !record.acknowledged_invalidations.is_empty() {
        return Proof::Unavailable("acknowledged_invalidations");
    }
    let Some(previous) = previous else {
        return Proof::Unavailable("previous_text_unavailable");
    };
    let mut over_limit = false;
    let hasher = services.hasher;
    let dir = document.directory();
    let scope: Vec<&ProjectPath> = record.manifest.files().iter().map(|f| &f.path).collect();
    let excluded = |subtree: &DirPath| !scope.iter().any(|path| path.is_within(subtree));
    let scope_paths: Vec<String> = scope.iter().map(|p| p.as_str().to_string()).collect();
    let imports: Vec<ImportEdge> = record
        .manifest
        .imports()
        .iter()
        .map(|import| ImportEdge {
            provider: import.document.as_str().to_string(),
            export_id: import.export_id.as_str().to_string(),
            hash: import.hash,
        })
        .collect();
    let mapping = prior_mapping(services, document, previous, &scope);
    let current = review_context::build(hasher, snapshot, document);
    let inputs_digest = hasher.hash(&encode_inputs(&record.manifest));
    // Records without evidence were written before `B` bound coverage, so
    // their first review bound the v1 absent baseline.
    let baseline_digest = hasher.hash(&encode_legacy_absent_review_baseline_v1());
    let recorded = format!(
        "{}{}",
        crate::packet::TOKEN_PREFIX,
        record.token_digest.to_hex()
    );
    let proves = |context: Vec<u8>| {
        compute_token_v3(
            hasher,
            document,
            0,
            inputs_digest,
            baseline_digest,
            hasher.hash(&context),
            &[],
        ) == recorded
    };
    let subsets = |count: usize| 0u32..(1u32 << count);
    let chosen = |mask: u32, index: usize| mask & (1 << index) != 0;

    // Layout v2: the handoffs that 0.7 bound.
    let mut candidates: Vec<(DirPath, ProjectPath)> = snapshot
        .referenced_targets_for_bytes(services, document, previous)
        .into_iter()
        .filter(|target| target.is_markdown())
        .map(|target| (target.directory(), target))
        .filter(|(subtree, _)| subtree.is_strictly_within(&dir) && excluded(subtree))
        .collect();
    candidates.sort();
    candidates.dedup();
    if candidates.len() <= MAX_PROOF_CANDIDATES {
        for mask in subsets(candidates.len()) {
            let picked: Vec<&(DirPath, ProjectPath)> = candidates
                .iter()
                .enumerate()
                .filter(|(index, _)| chosen(mask, *index))
                .map(|(_, candidate)| candidate)
                .collect();
            let nested_repositories: Vec<String> = snapshot
                .collected
                .boundaries
                .iter()
                .filter(|path| {
                    let parent = path.directory();
                    parent.is_within(&dir)
                        && !picked.iter().any(|(subtree, _)| parent.is_within(subtree))
                })
                .map(|path| path.as_str().to_string())
                .collect();
            let context = ReviewContext {
                selection_version: memoria_domain::section::SELECTION_VERSION,
                owner: document.as_str().to_string(),
                document_kind: document.kind().as_str().to_string(),
                handoffs: picked
                    .iter()
                    .map(|(subtree, target)| HandoffEdge {
                        subtree: subtree.as_str().to_string(),
                        target: target.as_str().to_string(),
                    })
                    .collect(),
                nested_repositories,
                policy_hash: record.manifest.policy_hash,
                scope_paths: scope_paths.clone(),
                mapping: mapping.clone(),
                guidance: record.guidance,
                imports: imports.clone(),
                consumer_edges: current.consumer_edges.clone(),
                providers: current.providers.clone(),
            };
            if proves(encode_review_context(&context)) {
                let mut out: Vec<DirPath> = picked.iter().map(|(s, _)| s.clone()).collect();
                out.sort();
                out.dedup();
                return Proof::Proven(out);
            }
        }
    } else {
        over_limit = true;
    }

    // Layout v1: the README boundaries that Memoria 0.6 applied implicitly.
    let readmes: Vec<&DocumentId> = snapshot
        .collected
        .documents
        .iter()
        .filter(|id| id.path().is_readme() && *id != document)
        .collect();
    let ancestor_boundaries: Vec<String> = readmes
        .iter()
        .filter(|id| dir.is_within(&id.directory()))
        .map(|id| id.as_str().to_string())
        .collect();
    let descendants: Vec<&DocumentId> = readmes
        .iter()
        .copied()
        .filter(|id| id.directory().is_strictly_within(&dir) && excluded(&id.directory()))
        .collect();
    if descendants.len() <= MAX_PROOF_CANDIDATES {
        let nested_repositories: Vec<String> = snapshot
            .collected
            .boundaries
            .iter()
            .filter(|path| path.is_within(&dir))
            .map(|path| path.as_str().to_string())
            .collect();
        for mask in subsets(descendants.len()) {
            let picked: Vec<&DocumentId> = descendants
                .iter()
                .enumerate()
                .filter(|(index, _)| chosen(mask, *index))
                .map(|(_, id)| *id)
                .collect();
            let context = LegacyReviewContextV1 {
                selection_version: 1,
                owner: document.as_str().to_string(),
                ancestor_boundaries: ancestor_boundaries.clone(),
                descendant_boundaries: picked.iter().map(|id| id.as_str().to_string()).collect(),
                nested_repositories: nested_repositories.clone(),
                policy_hash: record.manifest.policy_hash,
                owned_paths: scope_paths.clone(),
                mapping: mapping.clone(),
                guidance: record.guidance,
                imports: imports.clone(),
                consumer_edges: current.consumer_edges.clone(),
                providers: current.providers.clone(),
            };
            if proves(encode_legacy_review_context_v1(&context)) {
                let mut out: Vec<DirPath> = picked.iter().map(|id| id.directory()).collect();
                out.sort();
                out.dedup();
                return Proof::Proven(out);
            }
        }
    } else {
        over_limit = true;
    }
    // A layout skipped at the bound could still have held the answer.
    Proof::Unavailable(if over_limit {
        "candidate_limit"
    } else {
        "no_matching_reconstruction"
    })
}

/// The section mapping identity that the previous text had over the
/// recorded scope, with the same total-validity rule as current resolution.
fn prior_mapping(
    services: &Services<'_>,
    document: &DocumentId,
    previous: &[u8],
    scope: &[&ProjectPath],
) -> memoria_domain::SectionMapIdentity {
    use memoria_domain::{SectionId, SectionMap, SectionMapping};
    let parsed = services.markdown.parse(document, previous);
    if !parsed.section_issues.is_empty() {
        return SectionMap::Invalid.identity();
    }
    if parsed.sections.is_empty() {
        return SectionMap::Absent.identity();
    }
    let dir = document.directory();
    let mut mappings = Vec::new();
    for section in &parsed.sections {
        let Ok(id) = SectionId::parse(&section.id) else {
            return SectionMap::Invalid.identity();
        };
        let mut sources = Vec::new();
        for raw in &section.files {
            match ProjectPath::resolve_relative(&dir, raw) {
                Ok(path) if scope.contains(&&path) => sources.push(path),
                _ => return SectionMap::Invalid.identity(),
            }
        }
        sources.sort();
        sources.dedup();
        mappings.push(SectionMapping {
            id,
            heading: section.heading.clone(),
            first_line: section.first_line,
            last_line: section.last_line,
            sources,
            // The guide is never part of the identity.
            guidance: None,
        });
    }
    SectionMap::Valid(mappings).identity()
}

/// How an added or removed source relates to this document's scope.
enum Classified {
    Source,
    PathSet,
    Handoff(DirPath),
    Classification,
    /// The former coverage is unknown: one of `UNRECORDED_REASONS`.
    Unrecorded(&'static str),
}

/// The relationship of one scope source change.
fn source_relationship(
    snapshot: &Snapshot,
    document: &DocumentId,
    sections: &memoria_domain::SectionMap,
    path: Option<&ProjectPath>,
    kind: RelationshipKind,
) -> Relationship {
    let Some(path) = path else {
        return Relationship::of_kind(kind);
    };
    let mapped: Vec<String> = if kind == RelationshipKind::ScopeSource {
        let mut ids: Vec<String> = sections
            .sections_for(path)
            .iter()
            .map(|section| section.id.as_str().to_string())
            .collect();
        ids.sort();
        ids
    } else {
        Vec::new()
    };
    let others = snapshot
        .scopes
        .covering(path)
        .iter()
        .filter(|other| *other != document)
        .count() as u64;
    Relationship {
        sections: mapped,
        also_covered_by_total: others,
        ..Relationship::of_kind(kind)
    }
}

/// The document's scope size and the handoffs that shape it, bounded.
pub fn scope_info(snapshot: &Snapshot, document: &DocumentId) -> ScopeInfo {
    let handoffs = snapshot.scopes.handoffs_of(document);
    let incoming = snapshot.scopes.handed_off_by(document);
    ScopeInfo {
        files: snapshot.scopes.scope_of(document).len() as u64,
        handoffs: handoffs
            .iter()
            .take(MAX_LIST_ENTRIES)
            .map(|h| ScopeHandoff {
                subtree: h.subtree.as_str().to_string(),
                target: h.target.as_str().to_string(),
                via: h.via.as_str().to_string(),
                line: h.line as u64,
            })
            .collect(),
        handoffs_total: handoffs.len() as u64,
        handed_off_by: incoming
            .iter()
            .take(MAX_LIST_ENTRIES)
            .map(|h| IncomingHandoff {
                parent: h.parent.as_str().to_string(),
                via: h.via.as_str().to_string(),
                line: h.line as u64,
            })
            .collect(),
        handed_off_by_total: incoming.len() as u64,
    }
}

/// Export consumers and co-covering documents, bounded. Shown for judgment;
/// never bound into the token.
pub fn downstream(snapshot: &Snapshot, document: &DocumentId) -> Downstream {
    let label = |id: &DocumentId| {
        snapshot
            .status_of(id)
            .map(status_label)
            .unwrap_or("unknown")
            .to_string()
    };
    let mut consumers: Vec<ConsumerInfo> = Vec::new();
    if let (Some(graph), Some(parsed)) = (&snapshot.graph, snapshot.documents.get(document)) {
        for export in &parsed.exports {
            for consumer in graph.consumers_of_export(document, &export.id) {
                let waits = snapshot
                    .status_of(&consumer)
                    .is_some_and(|status| status.waiting_on.contains(document));
                consumers.push(ConsumerInfo {
                    export_id: export.id.as_str().to_string(),
                    consumer_kind: consumer.kind().as_str().to_string(),
                    status: label(&consumer),
                    consumer: consumer.as_str().to_string(),
                    waits_for_this_document: waits,
                });
            }
        }
    }
    consumers.sort_by(|a, b| (&a.export_id, &a.consumer).cmp(&(&b.export_id, &b.consumer)));
    let co: Vec<DocumentId> = snapshot.co_covering(document);
    Downstream {
        consumers_total: consumers.len() as u64,
        consumers: consumers.into_iter().take(MAX_LIST_ENTRIES).collect(),
        co_covering_total: co.len() as u64,
        co_covering: co
            .iter()
            .take(MAX_LIST_ENTRIES)
            .map(|other| CoCovering {
                document: other.as_str().to_string(),
                document_kind: other.kind().as_str().to_string(),
                status: label(other),
            })
            .collect(),
    }
}
