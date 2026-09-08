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

    /// The `data.context.guidance` shape used by focused packets and the
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

pub fn entry_detail(entry: &GuidanceEntry) -> Detail {
    DetailMap::default()
        .text("scope", entry.scope.as_str())
        .text("source", entry.source.clone())
        .text("kind", entry.kind.as_str())
        .text("text", entry.text.clone())
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
    }
}

/// The exact byte budget guidance contributes to a packet.
pub fn text_bytes(entries: &[GuidanceEntry]) -> u64 {
    entries.iter().map(|e| e.text.len() as u64).sum()
}
