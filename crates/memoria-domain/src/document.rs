//! Authored relationships parsed from a README document.

use std::fmt;

use crate::path::DocumentId;

/// A half-open byte range `[start, end)` inside a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    pub fn new(start: usize, end: usize) -> ByteRange {
        debug_assert!(start <= end);
        ByteRange { start, end }
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn overlaps(&self, other: &ByteRange) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// One-based line and column of a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Validated export identifier: `[A-Za-z][A-Za-z0-9_-]{0,63}`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExportId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidExportId(pub String);

impl fmt::Display for InvalidExportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "export id {:?} must match [A-Za-z][A-Za-z0-9_-]{{0,63}}",
            self.0
        )
    }
}

impl std::error::Error for InvalidExportId {}

impl ExportId {
    pub fn parse(raw: &str) -> Result<ExportId, InvalidExportId> {
        let mut chars = raw.chars();
        let valid = match chars.next() {
            Some(first) if first.is_ascii_alphabetic() => {
                raw.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            }
            _ => false,
        };
        if valid {
            Ok(ExportId(raw.to_string()))
        } else {
            Err(InvalidExportId(raw.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExportId({:?})", self.0)
    }
}

impl fmt::Display for ExportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A marked section that other documents can import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub id: ExportId,
    pub body: ByteRange,
    pub location: SourceLocation,
}

/// A declared dependency on another document's export and the byte range of
/// the generated copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub provider: DocumentId,
    pub export_id: ExportId,
    /// The `src` attribute exactly as authored, for diagnostics.
    pub source_text: String,
    pub body: ByteRange,
    pub location: SourceLocation,
}

/// A parsed README with its declarations and normal navigation links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: DocumentId,
    pub exports: Vec<Export>,
    pub imports: Vec<Import>,
    /// Normal local README links that resolve to discovered documents.
    pub links: Vec<DocumentId>,
}

impl Document {
    pub fn export(&self, id: &ExportId) -> Option<&Export> {
        self.exports.iter().find(|export| &export.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_id_grammar() {
        assert!(ExportId::parse("summary").is_ok());
        assert!(ExportId::parse("a-b_c9").is_ok());
        assert!(ExportId::parse("9a").is_err());
        assert!(ExportId::parse("").is_err());
        assert!(ExportId::parse("a b").is_err());
        assert!(ExportId::parse(&"a".repeat(64)).is_ok());
        assert!(ExportId::parse(&"a".repeat(65)).is_err());
    }

    #[test]
    fn byte_range_overlap() {
        let a = ByteRange::new(0, 10);
        assert!(a.overlaps(&ByteRange::new(5, 15)));
        assert!(!a.overlaps(&ByteRange::new(10, 15)));
    }
}
