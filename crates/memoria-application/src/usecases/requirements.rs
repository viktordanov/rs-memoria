//! Decide what one review must read, and why it cannot read less.
//!
//! Focused reading is a candidate only when the whole picture holds: a prior
//! declaration exists, the bytes that declaration named can still be
//! verified, the mapping associations are valid and unchanged, and nothing
//! outside the mapped sources moved. Any doubt produces a full baseline with
//! an explicit reason. Nothing here narrows what acknowledgement validates.

use memoria_domain::{DocumentId, InputChange, InputManifest, ReviewRecord};

use crate::error::AppError;
use crate::packet::{ChangeEntry, compute_token_v3};
use crate::ports::Services;
use crate::review::{
    BaselineInfo, EvidenceStatus, FallbackReason, GuidanceReference, InputEntry, InputRole,
    ReviewManifest, ReviewMode, SectionSuggestion, SnapshotDigests,
};
use crate::review_context;
use crate::snapshot::Snapshot;

use super::diff_changes;

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
    /// Previous README bytes, when they were recovered.
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
/// The previous README is always needed, because mapping associations are
/// compared against it even when only sources changed. An unchanged current
/// README supplies those exact bytes without touching local history.
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

/// Index every guidance entry inside its declaring inline or file list.
fn guidance_references(guidance: &crate::guidance::EffectiveGuidance) -> Vec<GuidanceReference> {
    let mut counters: std::collections::BTreeMap<(String, &'static str), u64> =
        std::collections::BTreeMap::new();
    guidance
        .entries
        .iter()
        .map(|entry| {
            let kind = entry.kind.as_str();
            let counter = counters.entry((entry.source.clone(), kind)).or_insert(0);
            let entry_index = *counter;
            *counter += 1;
            GuidanceReference {
                scope: entry.scope.as_str().to_string(),
                source: entry.source.clone(),
                kind: kind.to_string(),
                entry_index,
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
            "the README's section mappings are invalid, so none of its advice can narrow this review; see the section_mapping_invalid diagnostics",
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
            "this README has no previous review to compare against; review the complete boundary",
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
                    "the effective selection policy changed since the last review; recompute the owned boundary",
                ));
            }
            let diff = record.manifest.diff(&current);
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
                        fallbacks.push(reason(
                            "path_set_changed",
                            Some(file.path.as_str()),
                            "a source was added to this README's boundary; a rename appears as a removal plus an addition",
                        ));
                    }
                    InputChange::Removed(file) => {
                        // A path that still exists but now answers to another
                        // README is an ownership move, not a plain removal.
                        let moved = snapshot
                            .ownership
                            .owner_of(&file.path)
                            .is_some_and(|owner| owner != document);
                        if moved {
                            fallbacks.push(reason(
                                "ownership_changed",
                                Some(file.path.as_str()),
                                "a source moved to another README's boundary; recompute ownership and review the complete current boundary",
                            ));
                        } else {
                            fallbacks.push(reason(
                                "path_set_changed",
                                Some(file.path.as_str()),
                                "a source left this README's boundary; a rename appears as a removal plus an addition",
                            ));
                        }
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
                    "an imported contract changed; review the provider first, render, then review this README against the current boundary",
                ));
            }
            // Mapping associations are compared against the previous README.
            // Without those bytes the comparison is impossible, and the
            // missing-evidence reason above already forces a full baseline.
            if let Some(previous) = &evidence.previous_readme {
                let previous_map = snapshot.section_map_for_bytes(services, document, previous);
                if previous_map.identity() != sections.identity() {
                    fallbacks.push(reason(
                        "mapping_changed",
                        Some(document.as_str()),
                        "the README's section mappings changed since the last review; a changed association cannot reduce the required scope",
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
                "no valid section describes this changed source, so the review cannot narrow to a part of the README",
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
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    } else {
        Vec::new()
    };

    // Suggested reads: the whole README, everything that changed, and the
    // context of every suggested section. This is advice, not the complete
    // ownership inventory.
    let mut inputs: Vec<InputEntry> = vec![InputEntry {
        kind: "document".into(),
        path: document.as_str().to_string(),
        export_id: None,
        bytes: current.document_bytes,
        hash: current.document_hash,
        role: InputRole::WholeReadme,
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
        snapshot: SnapshotDigests {
            inputs_digest: bound.inputs_digest,
            context_digest: bound.context_digest,
            guidance_digest: guidance.digest.0,
            baseline_digest: bound.baseline_digest,
            selection_version: bound.context.selection_version,
        },
        baseline,
        changes,
        mode,
        sections: suggestions,
        fallback_reasons: fallbacks,
        inputs,
        guidance_digest: guidance.digest.0,
        guidance_changed_since_review: snapshot.guidance_changed(document),
        guidance_references: guidance_references(&guidance),
        covered_invalidations: covered,
        selected_files: current.files().len() as u64,
        imports: current.imports().len() as u64,
        raw_input_bytes: current.raw_input_bytes(),
        artifact_digest: String::new(),
    })
}
