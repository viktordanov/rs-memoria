//! Project documentation guidance: the value types that a review records.
//!
//! Guidance states documentation goals, reader needs, and writing standards.
//! It informs human or agent judgment. It never controls file selection or
//! byte-based freshness: deterministic policy does that. A review stores the
//! digest of the guidance its reviewer saw, so a later wording change becomes
//! a visible advisory instead of a silent staleness claim.

use std::fmt;

use crate::manifest::{Hash64, InvalidHash};
use crate::path::DirPath;

/// Whether an entry came from configuration text, from a guidance file, or
/// from a section guide that a section of the document names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GuidanceKind {
    Inline,
    File,
    /// A registered section guide. It supplements project guidance for the
    /// sections that name it and never overrides it.
    Section,
}

impl GuidanceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GuidanceKind::Inline => "inline",
            GuidanceKind::File => "file",
            GuidanceKind::Section => "section",
        }
    }

    pub fn parse(raw: &str) -> Option<GuidanceKind> {
        match raw {
            "inline" => Some(GuidanceKind::Inline),
            "file" => Some(GuidanceKind::File),
            "section" => Some(GuidanceKind::Section),
            _ => None,
        }
    }
}

impl fmt::Display for GuidanceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One applicable guidance entry, in authored order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceEntry {
    /// The configuration scope that contributes the entry. The root is the
    /// empty directory path.
    pub scope: DirPath,
    /// `memoria.toml`, a sidecar path, or a guidance file path.
    pub source: String,
    pub kind: GuidanceKind,
    /// The exact decoded configuration string or exact guidance file text.
    pub text: String,
    /// For a section guide: the sections of the document that name it, in
    /// authored order. Presentation only. It is never digested, because the
    /// association already lives in the document bytes. Empty for project
    /// guidance.
    pub sections: Vec<GuidanceSection>,
}

/// One section that names a section guide, as a reading hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceSection {
    /// The authored section id.
    pub id: String,
    /// Text of the section's first heading. Empty when the section could not
    /// be parsed far enough to read it.
    pub heading: String,
    /// 1-based inclusive body line range.
    pub first_line: usize,
    pub last_line: usize,
}

/// The digest of an ordered guidance list. Advisory review context only: it
/// stays outside the policy hash, the input manifest, the review schedule,
/// and import propagation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GuidanceDigest(pub Hash64);

impl GuidanceDigest {
    pub fn parse(text: &str) -> Result<GuidanceDigest, InvalidHash> {
        Hash64::parse(text).map(GuidanceDigest)
    }

    pub fn to_hex(self) -> String {
        self.0.to_hex()
    }
}

impl fmt::Debug for GuidanceDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GuidanceDigest({})", self.0)
    }
}

impl fmt::Display for GuidanceDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trips_through_its_exact_names() {
        assert_eq!(GuidanceKind::parse("inline"), Some(GuidanceKind::Inline));
        assert_eq!(GuidanceKind::parse("file"), Some(GuidanceKind::File));
        assert_eq!(GuidanceKind::parse("section"), Some(GuidanceKind::Section));
        assert_eq!(GuidanceKind::parse("Inline"), None);
        assert_eq!(GuidanceKind::Section.to_string(), "section");
        assert_eq!(GuidanceKind::Inline.to_string(), "inline");
    }

    #[test]
    fn digest_renders_as_sixteen_hexadecimal_digits() {
        let digest = GuidanceDigest(Hash64(0x0123456789abcdef));
        assert_eq!(digest.to_hex(), "0123456789abcdef");
        assert_eq!(GuidanceDigest::parse("0123456789abcdef"), Ok(digest));
        assert!(GuidanceDigest::parse("0123456789ABCDEF").is_err());
    }
}
