//! The small review manifest: what a reviewer must read, never the bytes.
//!
//! A manifest states requirements. It carries no README, source, import,
//! historical, or authored-guidance body. Ordinary file tools supply reading;
//! Memoria supplies the snapshot binding and the attention guidance.
//!
//! A small suggested reading list never narrows the complete input state that
//! acknowledgement validates. `ack` always recomputes the complete input and
//! context digests from the repository.

use memoria_domain::{DocumentId, Hash64};

use crate::error::{Detail, DetailMap};
use crate::packet::ChangeEntry;

/// The artifact kind of a small manifest.
pub const MANIFEST_KIND: &str = "review_manifest";
/// The manifest schema version.
pub const MANIFEST_VERSION: u64 = 1;

/// The built-in workflow a reviewer follows. It is distinct from authored
/// guidance and subordinate to task authority.
pub const WORKFLOW_STEPS: [&str; 5] = [
    "Read current guidance and covered reasons.",
    "Inspect suggested sections and changed sources.",
    "Expand uncertain context or use full baseline.",
    "Read the whole README.",
    "Reconcile a fresh manifest after edits before acknowledgement.",
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
pub const FALLBACK_CODES: [&str; 11] = [
    "unmapped_change",
    "path_set_changed",
    "baseline_missing",
    "baseline_unavailable",
    "mapping_invalid",
    "mapping_changed",
    "ownership_changed",
    "policy_changed",
    "imports_changed",
    "semantic_invalidation",
    "guidance_changed",
];

/// One suggested section, with 1-based inclusive line hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionSuggestion {
    pub id: String,
    pub heading: String,
    pub first_line: usize,
    pub last_line: usize,
    /// Sorted root-relative sources.
    pub sources: Vec<String>,
}

/// Why one suggested read is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputRole {
    WholeReadme,
    ChangedSource,
    SectionContext,
    CurrentImport,
}

impl InputRole {
    pub fn as_str(self) -> &'static str {
        match self {
            InputRole::WholeReadme => "whole_readme",
            InputRole::ChangedSource => "changed_source",
            InputRole::SectionContext => "section_context",
            InputRole::CurrentImport => "current_import",
        }
    }
}

/// One suggested read identity. This list is advice, never the complete
/// ownership inventory that acknowledgement validates.
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
    /// Zero-based index inside its declaring inline or file list.
    pub entry_index: u64,
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
    pub snapshot: SnapshotDigests,
    pub baseline: Option<BaselineInfo>,
    pub changes: Vec<ChangeEntry>,
    pub mode: ReviewMode,
    pub sections: Vec<SectionSuggestion>,
    pub fallback_reasons: Vec<FallbackReason>,
    pub inputs: Vec<InputEntry>,
    pub guidance_digest: Hash64,
    pub guidance_changed_since_review: Option<bool>,
    pub guidance_references: Vec<GuidanceReference>,
    /// `(id, exact reason)` sorted by id.
    pub covered_invalidations: Vec<(u64, String)>,
    pub selected_files: u64,
    pub imports: u64,
    pub raw_input_bytes: u64,
    /// Filled by the codec on encode; verified on decode.
    pub artifact_digest: String,
}

impl ReviewManifest {
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
                Detail::list(self.changes.iter().map(|c| {
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
                                .build()
                        })),
                    )
                    // The whole-README pass is always required. A small
                    // reading list never replaces it.
                    .bool("whole_readme_pass", true)
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
                            DetailMap::default()
                                .text("scope", r.scope.clone())
                                .text("source", r.source.clone())
                                .text("kind", r.kind.clone())
                                .number("entry_index", r.entry_index)
                                .build()
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
            .with(
                "counts",
                DetailMap::default()
                    .number("selected_files", self.selected_files)
                    .number("imports", self.imports)
                    .number("raw_input_bytes", self.raw_input_bytes)
                    .number("suggested_sources", self.suggested_sources())
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
