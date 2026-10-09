//! Effective project documentation guidance for one document boundary.
//!
//! Guidance appends from the root scope toward the document scope. Within
//! each scope, inline entries precede file entries, and each list preserves
//! its authored order. The tool never overrides, deduplicates, or ranks
//! conflicting prose: the reviewer resolves a conflict with the author.

use memoria_domain::canonical;
use memoria_domain::{DirPath, DocumentId, GuidanceDigest, GuidanceEntry, GuidanceKind};

use crate::error::{Detail, DetailMap};
use crate::ports::FingerprintHasher;

/// The authority rule for section guides. Guidance views print it once,
/// and only when a section guide applies.
pub const CONFLICT_RULE: &str = "Project guidance applies to the whole document. A section guide adds to it for the sections that name it. If they conflict, follow project guidance and report the conflict.";

/// The complete effective guidance of one boundary and its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveGuidance {
    pub document: DocumentId,
    pub entries: Vec<GuidanceEntry>,
    pub digest: GuidanceDigest,
}

impl EffectiveGuidance {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The section guide entries, sorted by path.
    pub fn section_guides(&self) -> impl Iterator<Item = &GuidanceEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == GuidanceKind::Section)
    }

    /// The `data.context.guidance` shape used by full exports and the
    /// `guidance` command.
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("digest", self.digest.to_hex())
            .with(
                "entries",
                Detail::list(self.entries.iter().map(entry_detail)),
            )
            .build()
    }
}

/// One entry. A section guide also lists the sections that name it; that
/// list is presentation only and is never digested.
pub fn entry_detail(entry: &GuidanceEntry) -> Detail {
    let detail = DetailMap::default()
        .text("scope", entry.scope.as_str())
        .text("source", entry.source.clone())
        .text("kind", entry.kind.as_str())
        .text("text", entry.text.clone());
    if entry.kind != GuidanceKind::Section {
        return detail.build();
    }
    detail
        .with(
            "sections",
            Detail::list(entry.sections.iter().map(|section| {
                DetailMap::default()
                    .text("id", section.id.clone())
                    .text("heading", section.heading.clone())
                    .with(
                        "lines",
                        Detail::list([
                            Detail::Number(section.first_line as u64),
                            Detail::Number(section.last_line as u64),
                        ]),
                    )
                    .build()
            })),
        )
        .build()
}

/// Hash an ordered guidance list with `memoria.guidance.v1`.
pub fn digest_of(hasher: &dyn FingerprintHasher, entries: &[GuidanceEntry]) -> GuidanceDigest {
    GuidanceDigest(hasher.hash(&canonical::encode_guidance(entries)))
}

/// Build one entry without repeating the field names at every call site.
pub fn entry(scope: DirPath, source: &str, kind: GuidanceKind, text: String) -> GuidanceEntry {
    GuidanceEntry {
        scope,
        source: source.to_string(),
        kind,
        text,
        sections: Vec::new(),
    }
}

/// The exact byte budget guidance contributes to a packet.
pub fn text_bytes(entries: &[GuidanceEntry]) -> u64 {
    entries.iter().map(|e| e.text.len() as u64).sum()
}
