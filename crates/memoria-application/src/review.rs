//! The small review manifest: what a reviewer must read, never the bytes.
//!
//! A manifest states requirements. It carries no document, source, import,
//! historical, or authored-guidance body. Ordinary file tools supply reading;
//! Memoria supplies the snapshot binding and the attention guidance.
//!
//! A small suggested reading list never narrows the complete input state that
//! acknowledgement validates. `ack` always recomputes the complete input and
//! context digests from the repository.

use memoria_domain::{DocumentId, DocumentKind, Hash64};

use crate::error::{Detail, DetailMap};
use crate::packet::ChangeEntry;

/// The artifact kind of a small manifest.
pub const MANIFEST_KIND: &str = "review_manifest";
/// The manifest schema version. Version 3 adds section guides: the guide of
/// each suggested section, and `section` guidance references.
pub const MANIFEST_VERSION: u64 = 3;

/// Most entries any bounded manifest list carries. Each bounded list has a
/// `*_total` beside it with the complete count.
pub const MAX_LIST_ENTRIES: usize = 64;

/// The built-in workflow a reviewer follows. It is distinct from authored
/// guidance and subordinate to task authority.
pub const WORKFLOW_STEPS: [&str; 5] = [
    "Read current guidance and covered reasons.",
    "Inspect the changes, their relationships, and suggested sections.",
    "Expand uncertain context or use the full baseline.",
    "Read the whole document.",
    "Capture a fresh artifact after edits and reconcile before acknowledgement.",
];

/// Whether focused reading is technically eligible.
///
/// `FocusedCandidate` is eligibility, never certification of the prior
/// review. Without explicit reviewer trust, the workflow still uses the full
/// baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewMode {
    FocusedCandidate,
    FullBaseline,
}

impl ReviewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewMode::FocusedCandidate => "focused_candidate",
            ReviewMode::FullBaseline => "full_baseline",
        }
    }
}

/// Whether the bytes a focused comparison needs could be verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    Verified,
    Partial,
    Unavailable,
}

impl EvidenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceStatus::Verified => "verified",
            EvidenceStatus::Partial => "partial",
            EvidenceStatus::Unavailable => "unavailable",
        }
    }
}

/// One reason the review cannot narrow to suggested sections.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FallbackReason {
    /// One of the fixed codes in `FALLBACK_CODES`.
    pub code: String,
    /// Root-relative path, import identity, or null.
    pub identity: Option<String>,
    pub message: String,
}

/// Every fallback code this release emits.
pub const FALLBACK_CODES: [&str; 13] = [
    "unmapped_change",
    "path_set_changed",
    "baseline_missing",
    "baseline_unavailable",
    "mapping_invalid",
    "mapping_changed",
    "handoff_changed",
    "coverage_unrecorded",
    "document_classification_changed",
    "policy_changed",
    "imports_changed",
    "semantic_invalidation",
    "guidance_changed",
];

/// How one change relates to the reviewed document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationshipKind {
    /// The document's own text.
    OwnText,
    /// A source in the document's scope.
    ScopeSource,
    /// A source that entered or left the scope because a handoff changed.
    Handoff,
    /// A source that entered the scope of a document whose last review did
    /// not record its handed-off folders, when no exact proof exists.
    CoverageUnrecorded,
    /// An imported export body.
    Import,
    /// The effective selection policy.
    SelectionPolicy,
}

impl RelationshipKind {
    pub const ALL: [RelationshipKind; 6] = [
        RelationshipKind::OwnText,
        RelationshipKind::ScopeSource,
        RelationshipKind::Handoff,
        RelationshipKind::CoverageUnrecorded,
        RelationshipKind::Import,
        RelationshipKind::SelectionPolicy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RelationshipKind::OwnText => "own_text",
            RelationshipKind::ScopeSource => "scope_source",
            RelationshipKind::Handoff => "handoff",
            RelationshipKind::CoverageUnrecorded => "coverage_unrecorded",
            RelationshipKind::Import => "import",
            RelationshipKind::SelectionPolicy => "selection_policy",
        }
    }

    pub fn parse(text: &str) -> Option<RelationshipKind> {
        RelationshipKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

/// Why the former coverage of a document without recorded evidence cannot
/// be proven, in the order the legacy proof checks its conditions.
pub const UNRECORDED_REASONS: [&str; 5] = [
    "revision_not_first",
    "acknowledged_invalidations",
    "previous_text_unavailable",
    "candidate_limit",
    "no_matching_reconstruction",
];

/// The relationship of one change to the reviewed document. Shown for
/// judgment; it is not a semantic-change detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    pub kind: RelationshipKind,
    /// For a scope source: this document's valid section IDs that map it.
    pub sections: Vec<String>,
    /// For a source: how many other documents cover it now.
    pub also_covered_by_total: u64,
    /// For an import: the provider and export.
    pub provider: Option<String>,
    pub export_id: Option<String>,
    /// For `coverage_unrecorded`: one of `UNRECORDED_REASONS`.
    pub unrecorded_reason: Option<String>,
}

impl Relationship {
    pub fn of_kind(kind: RelationshipKind) -> Relationship {
        Relationship {
            kind,
            sections: Vec::new(),
            also_covered_by_total: 0,
            provider: None,
            export_id: None,
            unrecorded_reason: None,
        }
    }

    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", self.kind.as_str())
            .with("sections", Detail::texts(self.sections.clone()))
            .number("also_covered_by_total", self.also_covered_by_total)
            .with("provider", Detail::option_text(self.provider.clone()))
            .with("export_id", Detail::option_text(self.export_id.clone()))
            .with(
                "unrecorded_reason",
                Detail::option_text(self.unrecorded_reason.clone()),
            )
            .build()
    }
}

/// One outgoing handoff of the reviewed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeHandoff {
    pub subtree: String,
    pub target: String,
    /// `link`, `import`, or `both`.
    pub via: String,
    pub line: u64,
}

/// One incoming handoff: a parent that hands this document's folder to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingHandoff {
    pub parent: String,
    pub via: String,
    pub line: u64,
}

/// The reviewed document's scope: its size and the handoffs that shape it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeInfo {
    pub files: u64,
    /// At most `MAX_LIST_ENTRIES`.
    pub handoffs: Vec<ScopeHandoff>,
    pub handoffs_total: u64,
    /// At most `MAX_LIST_ENTRIES`.
    pub handed_off_by: Vec<IncomingHandoff>,
    pub handed_off_by_total: u64,
}

impl ScopeInfo {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .number("files", self.files)
            .with(
                "handoffs",
                Detail::list(self.handoffs.iter().map(|h| {
                    DetailMap::default()
                        .text("subtree", h.subtree.clone())
                        .text("target", h.target.clone())
                        .text("via", h.via.clone())
                        .number("line", h.line)
                        .build()
                })),
            )
            .number("handoffs_total", self.handoffs_total)
            .with(
                "handed_off_by",
                Detail::list(self.handed_off_by.iter().map(|h| {
                    DetailMap::default()
                        .text("parent", h.parent.clone())
                        .text("via", h.via.clone())
                        .number("line", h.line)
                        .build()
                })),
            )
            .number("handed_off_by_total", self.handed_off_by_total)
            .build()
    }
}

/// One direct consumer of an export of the reviewed document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerInfo {
    pub export_id: String,
    pub consumer: String,
    pub consumer_kind: String,
    /// `current`, `pending`, `never_reviewed`, or `unknown`.
    pub status: String,
    /// Whether the consumer waits for this document's review.
    pub waits_for_this_document: bool,
}

/// Another document whose scope contains a source that changed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoCovering {
    pub document: String,
    pub document_kind: String,
    pub status: String,
}

/// Who else a change reaches. Advisory; never bound into the token.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Downstream {
    /// At most `MAX_LIST_ENTRIES`.
    pub consumers: Vec<ConsumerInfo>,
    pub consumers_total: u64,
    /// At most `MAX_LIST_ENTRIES`.
    pub co_covering: Vec<CoCovering>,
    pub co_covering_total: u64,
}

impl Downstream {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .with(
                "consumers",
                Detail::list(self.consumers.iter().map(|c| {
                    DetailMap::default()
                        .text("export_id", c.export_id.clone())
                        .text("consumer", c.consumer.clone())
                        .text("consumer_kind", c.consumer_kind.clone())
                        .text("status", c.status.clone())
                        .bool("waits_for_this_document", c.waits_for_this_document)
                        .build()
                })),
            )
            .number("consumers_total", self.consumers_total)
            .with(
                "co_covering",
                Detail::list(self.co_covering.iter().map(|c| {
                    DetailMap::default()
                        .text("document", c.document.clone())
                        .text("document_kind", c.document_kind.clone())
                        .text("status", c.status.clone())
                        .build()
                })),
            )
            .number("co_covering_total", self.co_covering_total)
            .build()
    }
}

/// One suggested section, with 1-based inclusive line hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionSuggestion {
    pub id: String,
    pub heading: String,
    pub first_line: usize,
    pub last_line: usize,
    /// Sorted root-relative sources.
    pub sources: Vec<String>,
    /// The resolved section guide this section names, if any.
    pub guidance: Option<String>,
}

/// Why one suggested read is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputRole {
    WholeDocument,
    ChangedSource,
    SectionContext,
    CurrentImport,
}

impl InputRole {
    pub fn as_str(self) -> &'static str {
        match self {
            InputRole::WholeDocument => "whole_document",
            InputRole::ChangedSource => "changed_source",
            InputRole::SectionContext => "section_context",
            InputRole::CurrentImport => "current_import",
        }
    }
}

/// One suggested read identity. This list is advice, never the complete
/// scope inventory that acknowledgement validates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEntry {
    /// `document`, `file`, or `import`.
    pub kind: String,
    pub path: String,
    /// Non-null only for imports.
    pub export_id: Option<String>,
    pub bytes: u64,
    pub hash: Hash64,
    pub role: InputRole,
}

/// The prior acknowledgement a focused review would reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineInfo {
    pub revision: u64,
    pub token_digest: Hash64,
    pub reviewer: String,
    pub result: String,
    pub recorded_commit: Option<String>,
    pub evidence_status: EvidenceStatus,
}

/// One effective guidance reference, without its authored prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceReference {
    pub scope: String,
    pub source: String,
    pub kind: String,
    /// Zero-based index inside its declaring inline or file list, or, for a
    /// section guide, inside the document's section guide entries.
    pub entry_index: u64,
    /// For a section guide: the ids of the sections that name it, in
    /// authored order. `None` for project guidance.
    pub sections: Option<Vec<String>>,
}

/// The digests a v3 token binds, as displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDigests {
    pub inputs_digest: Hash64,
    pub context_digest: Hash64,
    pub guidance_digest: Hash64,
    pub baseline_digest: Hash64,
    pub selection_version: u64,
}

/// The complete small review manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewManifest {
    pub document: DocumentId,
    pub review_revision: u64,
    pub token: String,
    pub scope: ScopeInfo,
    pub snapshot: SnapshotDigests,
    pub baseline: Option<BaselineInfo>,
    pub changes: Vec<ChangeEntry>,
    /// One relationship per change, in the same order.
    pub relationships: Vec<Relationship>,
    pub mode: ReviewMode,
    pub sections: Vec<SectionSuggestion>,
    pub fallback_reasons: Vec<FallbackReason>,
    pub inputs: Vec<InputEntry>,
    pub guidance_digest: Hash64,
    pub guidance_changed_since_review: Option<bool>,
    pub guidance_references: Vec<GuidanceReference>,
    /// `(id, exact reason)` sorted by id.
    pub covered_invalidations: Vec<(u64, String)>,
    pub downstream: Downstream,
    pub scope_files: u64,
    pub imports: u64,
    pub raw_input_bytes: u64,
    /// Filled by the codec on encode; verified on decode.
    pub artifact_digest: String,
}

impl ReviewManifest {
    /// The document's kind, derived from its path.
    pub fn document_kind(&self) -> DocumentKind {
        self.document.kind()
    }

    /// Unique suggested sources across every suggested section.
    pub fn suggested_sources(&self) -> u64 {
        let mut sources: Vec<&str> = self
            .sections
            .iter()
            .flat_map(|section| section.sources.iter().map(String::as_str))
            .collect();
        sources.sort_unstable();
        sources.dedup();
        sources.len() as u64
    }

    /// The `data` object shared by the small manifest and, as
    /// `requirements`, by a full export.
    pub fn to_detail(&self) -> Detail {
        let mut detail = self.requirements_detail();
        if let Detail::Map(map) = &mut detail {
            map.insert(
                "artifact_digest".into(),
                Detail::text(self.artifact_digest.clone()),
            );
        }
        detail
    }

    /// Everything except `artifact_digest`, which a full export embeds.
    pub fn requirements_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", MANIFEST_KIND)
            .number("manifest_version", MANIFEST_VERSION)
            .text("document", self.document.as_str())
            .text("document_kind", self.document_kind().as_str())
            .with("scope", self.scope.to_detail())
            .number("review_revision", self.review_revision)
            .text("token", self.token.clone())
            .with(
                "snapshot",
                DetailMap::default()
                    .text("inputs_digest", self.snapshot.inputs_digest.to_hex())
                    .text("context_digest", self.snapshot.context_digest.to_hex())
                    .text("guidance_digest", self.snapshot.guidance_digest.to_hex())
                    .text("baseline_digest", self.snapshot.baseline_digest.to_hex())
                    .number("selection_version", self.snapshot.selection_version)
                    .build(),
            )
            .with(
                "baseline",
                match &self.baseline {
                    None => Detail::Null,
                    Some(baseline) => DetailMap::default()
                        .number("revision", baseline.revision)
                        .text("token_digest", baseline.token_digest.to_hex())
                        .text("reviewer", baseline.reviewer.clone())
                        .text("result", baseline.result.clone())
                        .with(
                            "recorded_commit",
                            Detail::option_text(baseline.recorded_commit.clone()),
                        )
                        .text("evidence_status", baseline.evidence_status.as_str())
                        .build(),
                },
            )
            .with(
                "changes",
                Detail::list(self.changes.iter().zip(&self.relationships).map(|(c, r)| {
                    DetailMap::default()
                        .text("kind", c.kind.clone())
                        .text("change", c.change.clone())
                        .text("identity", c.identity.clone())
                        .with(
                            "before_bytes",
                            c.before_bytes.map(Detail::Number).unwrap_or(Detail::Null),
                        )
                        .with(
                            "before_hash",
                            Detail::option_text(c.before_hash.map(|h| h.to_hex())),
                        )
                        .with(
                            "after_bytes",
                            c.after_bytes.map(Detail::Number).unwrap_or(Detail::Null),
                        )
                        .with(
                            "after_hash",
                            Detail::option_text(c.after_hash.map(|h| h.to_hex())),
                        )
                        .with("relationship", r.to_detail())
                        .build()
                })),
            )
            .with(
                "review",
                DetailMap::default()
                    .text("mode", self.mode.as_str())
                    .with(
                        "sections",
                        Detail::list(self.sections.iter().map(|s| {
                            DetailMap::default()
                                .text("id", s.id.clone())
                                .text("heading", s.heading.clone())
                                .with(
                                    "lines",
                                    Detail::list([
                                        Detail::Number(s.first_line as u64),
                                        Detail::Number(s.last_line as u64),
                                    ]),
                                )
                                .with("sources", Detail::texts(s.sources.clone()))
                                .with("guidance", Detail::option_text(s.guidance.clone()))
                                .build()
                        })),
                    )
                    // The whole-document pass is always required. A small
                    // reading list never replaces it.
                    .bool("whole_document_pass", true)
                    .with(
                        "fallback_reasons",
                        Detail::list(self.fallback_reasons.iter().map(|r| {
                            DetailMap::default()
                                .text("code", r.code.clone())
                                .with("identity", Detail::option_text(r.identity.clone()))
                                .text("message", r.message.clone())
                                .build()
                        })),
                    )
                    .build(),
            )
            .with(
                "inputs",
                Detail::list(self.inputs.iter().map(|i| {
                    DetailMap::default()
                        .text("kind", i.kind.clone())
                        .text("path", i.path.clone())
                        .with("export_id", Detail::option_text(i.export_id.clone()))
                        .number("bytes", i.bytes)
                        .text("hash", i.hash.to_hex())
                        .text("role", i.role.as_str())
                        .build()
                })),
            )
            .with(
                "guidance",
                DetailMap::default()
                    .text("digest", self.guidance_digest.to_hex())
                    .with(
                        "changed_since_review",
                        match self.guidance_changed_since_review {
                            None => Detail::Null,
                            Some(value) => Detail::Bool(value),
                        },
                    )
                    .with(
                        "references",
                        Detail::list(self.guidance_references.iter().map(|r| {
                            let mut reference = DetailMap::default()
                                .text("scope", r.scope.clone())
                                .text("source", r.source.clone())
                                .text("kind", r.kind.clone())
                                .number("entry_index", r.entry_index);
                            if let Some(sections) = &r.sections {
                                reference =
                                    reference.with("sections", Detail::texts(sections.clone()));
                            }
                            reference.build()
                        })),
                    )
                    .with(
                        "command",
                        Detail::texts(vec![
                            "memoria".to_string(),
                            "guidance".to_string(),
                            self.document.as_str().to_string(),
                        ]),
                    )
                    .build(),
            )
            .with(
                "covered_invalidations",
                Detail::list(self.covered_invalidations.iter().map(|(id, reason)| {
                    DetailMap::default()
                        .number("id", *id)
                        .text("reason", reason.clone())
                        .build()
                })),
            )
            .with("downstream", self.downstream.to_detail())
            .with(
                "counts",
                DetailMap::default()
                    .number("scope_files", self.scope_files)
                    .number("imports", self.imports)
                    .number("raw_input_bytes", self.raw_input_bytes)
                    .number("suggested_sources", self.suggested_sources())
                    .number("handoffs", self.scope.handoffs_total)
                    .build(),
            )
            .with(
                "workflow",
                DetailMap::default()
                    .text("policy", memoria_domain::section::SELECTION_POLICY)
                    .with(
                        "steps",
                        Detail::texts(
                            WORKFLOW_STEPS
                                .iter()
                                .map(|s| s.to_string())
                                .collect::<Vec<_>>(),
                        ),
                    )
                    .build(),
            )
            .build()
    }
}
