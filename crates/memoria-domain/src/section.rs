//! Advisory section mappings.
//!
//! A section is an optional reading hint authored in a tracked document. Its
//! `files` must name selected sources in that document's scope. It adds or
//! removes no input, never carries its own freshness, and never narrows the
//! complete input state that an acknowledgement validates. One invalid
//! mapping makes the whole document's advice unusable, so a reviewer falls
//! back to the full baseline instead of trusting partial advice.
//!
//! A section can also name one registered section guide. The guide is review
//! context only: it adds no input, never changes the mapping identity, and
//! never makes a section a suggestion. A section with a guide and no `files`
//! is a guide-only section.

use std::collections::BTreeSet;
use std::fmt;

use crate::path::ProjectPath;

/// The selection policy version that section advice belongs to. It enters the
/// review context so a policy change invalidates an outstanding token.
pub const SELECTION_VERSION: u64 = 2;

/// The built-in workflow identifier reported beside section advice.
pub const SELECTION_POLICY: &str = "section-review-v2";

/// Validated section identifier: `[A-Za-z][A-Za-z0-9_-]{0,63}`.
///
/// Identifiers are case-sensitive and unique inside one README. They never
/// imply identity across READMEs.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSectionId(pub String);

impl fmt::Display for InvalidSectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "section id {:?} must match [A-Za-z][A-Za-z0-9_-]{{0,63}}",
            self.0
        )
    }
}

impl std::error::Error for InvalidSectionId {}

impl SectionId {
    pub fn parse(raw: &str) -> Result<SectionId, InvalidSectionId> {
        let mut chars = raw.chars();
        let valid = match chars.next() {
            Some(first) if first.is_ascii_alphabetic() => {
                raw.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            }
            _ => false,
        };
        if valid {
            Ok(SectionId(raw.to_string()))
        } else {
            Err(InvalidSectionId(raw.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SectionId({:?})", self.0)
    }
}

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why one authored `files` token is not a usable literal source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionPathError {
    Empty,
    Absolute,
    DrivePrefix,
    Backslash,
    Glob(char),
    Whitespace,
    Quote,
    Control,
    EmptyComponent,
    DotComponent,
    TooLong,
}

impl fmt::Display for SectionPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SectionPathError::Empty => f.write_str("path is empty"),
            SectionPathError::Absolute => {
                f.write_str("path must be relative to the README directory, not absolute")
            }
            SectionPathError::DrivePrefix => f.write_str(
                "path must not contain `:`; drive prefixes and stream names are unsupported",
            ),
            SectionPathError::Backslash => {
                f.write_str("path must use `/` separators; `\\` is not permitted")
            }
            SectionPathError::Glob(c) => write!(
                f,
                "path must be literal; the glob character {c:?} is not permitted"
            ),
            SectionPathError::Whitespace => f.write_str(
                "path must not contain whitespace; this section syntax cannot spell such a name",
            ),
            SectionPathError::Quote => f.write_str("path must not contain a quote character"),
            SectionPathError::Control => f.write_str("path must not contain a control character"),
            SectionPathError::EmptyComponent => {
                f.write_str("path must not contain an empty component")
            }
            SectionPathError::DotComponent => {
                f.write_str("path must not contain a `.` or `..` component")
            }
            SectionPathError::TooLong => f.write_str("path is longer than 1024 bytes"),
        }
    }
}

impl std::error::Error for SectionPathError {}

/// Longest authored path token this syntax accepts.
pub const MAX_SECTION_PATH_BYTES: usize = 1024;

/// Check one authored `files` token before it is resolved against the
/// README's directory. Every rejection is explicit; nothing is repaired,
/// percent-decoded, or escaped.
pub fn validate_section_path(raw: &str) -> Result<(), SectionPathError> {
    if raw.is_empty() {
        return Err(SectionPathError::Empty);
    }
    if raw.len() > MAX_SECTION_PATH_BYTES {
        return Err(SectionPathError::TooLong);
    }
    if raw.starts_with('/') {
        return Err(SectionPathError::Absolute);
    }
    for c in raw.chars() {
        match c {
            '\\' => return Err(SectionPathError::Backslash),
            ':' => return Err(SectionPathError::DrivePrefix),
            '*' | '?' | '[' | ']' | '{' | '}' => return Err(SectionPathError::Glob(c)),
            '"' | '\'' => return Err(SectionPathError::Quote),
            c if c.is_control() => return Err(SectionPathError::Control),
            c if c.is_whitespace() => return Err(SectionPathError::Whitespace),
            _ => {}
        }
    }
    for component in raw.split('/') {
        if component.is_empty() {
            return Err(SectionPathError::EmptyComponent);
        }
        if component == "." || component == ".." {
            return Err(SectionPathError::DotComponent);
        }
    }
    Ok(())
}

/// Why one authored `guidance` token is not a usable guide path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuidancePathError {
    Empty,
    Absolute,
    Forbidden(char),
    Whitespace,
    Quote,
    Control,
    TooLong,
}

impl fmt::Display for GuidancePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuidancePathError::Empty => f.write_str("guide path is empty"),
            GuidancePathError::Absolute => {
                f.write_str("guide path must be relative to the document's folder, not absolute")
            }
            GuidancePathError::Forbidden(c) => {
                write!(f, "guide path must not contain the character {c:?}")
            }
            GuidancePathError::Whitespace => {
                f.write_str("guide path must be exactly one path with no whitespace")
            }
            GuidancePathError::Quote => {
                f.write_str("guide path must not contain a quote character")
            }
            GuidancePathError::Control => {
                f.write_str("guide path must not contain a control character")
            }
            GuidancePathError::TooLong => f.write_str("guide path is longer than 1024 bytes"),
        }
    }
}

impl std::error::Error for GuidancePathError {}

/// Most distinct section guides that one document may name.
pub const MAX_SECTION_GUIDES: usize = 64;

/// Largest registered section guide, in bytes.
pub const MAX_SECTION_GUIDE_BYTES: u64 = 65_536;

/// Check one authored `guidance` token before it is resolved against the
/// document's directory. `.` and `..` components are allowed, exactly as in
/// an import `src`; the resolver refuses an escape above the project root.
pub fn validate_guidance_path(raw: &str) -> Result<(), GuidancePathError> {
    if raw.is_empty() {
        return Err(GuidancePathError::Empty);
    }
    if raw.len() > MAX_SECTION_PATH_BYTES {
        return Err(GuidancePathError::TooLong);
    }
    if raw.starts_with('/') {
        return Err(GuidancePathError::Absolute);
    }
    for c in raw.chars() {
        match c {
            '"' | '\'' => return Err(GuidancePathError::Quote),
            '\\' | ':' | '#' | '?' | '*' | '[' | ']' | '{' | '}' => {
                return Err(GuidancePathError::Forbidden(c));
            }
            c if c.is_control() => return Err(GuidancePathError::Control),
            c if c.is_whitespace() => return Err(GuidancePathError::Whitespace),
            _ => {}
        }
    }
    Ok(())
}

/// One resolved advisory mapping.
///
/// `lines` is the 1-based inclusive line range of the authored body, with
/// both markers excluded. The range is a reading hint for the current README
/// bytes only; the complete README hash separately binds every byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionMapping {
    pub id: SectionId,
    /// Text of the first heading in the body. A hint, never an identity.
    pub heading: String,
    pub first_line: usize,
    pub last_line: usize,
    /// Sorted, deduplicated owned sources this section describes. Empty only
    /// for a guide-only section.
    pub sources: Vec<ProjectPath>,
    /// The resolved section guide this section names, if any. It is never
    /// part of the mapping identity.
    pub guidance: Option<ProjectPath>,
}

/// A README's complete advisory mapping state.
///
/// `Invalid` is deliberately total: partial valid mappings cannot narrow a
/// review, because the reader cannot tell which advice the author intended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SectionMap {
    /// The README authored no section comments.
    #[default]
    Absent,
    /// Every authored section parsed, resolved, and validated.
    Valid(Vec<SectionMapping>),
    /// At least one section is malformed or unusable.
    Invalid,
}

impl SectionMap {
    pub fn is_valid(&self) -> bool {
        !matches!(self, SectionMap::Invalid)
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, SectionMap::Absent)
    }

    pub fn sections(&self) -> &[SectionMapping] {
        match self {
            SectionMap::Valid(sections) => sections,
            _ => &[],
        }
    }

    /// Every section that names `path`, in authored order. One source can
    /// appear in several sections; all of them become suggestions.
    pub fn sections_for(&self, path: &ProjectPath) -> Vec<&SectionMapping> {
        self.sections()
            .iter()
            .filter(|section| section.sources.iter().any(|source| source == path))
            .collect()
    }

    /// The comparable association set.
    ///
    /// Body edits, heading text, moved line ranges, and the section guide do
    /// not change this identity. A guide-only section contributes its id with
    /// no sources. Any change to it requires a full baseline review.
    pub fn identity(&self) -> SectionMapIdentity {
        match self {
            SectionMap::Absent => SectionMapIdentity::Absent,
            SectionMap::Invalid => SectionMapIdentity::Invalid,
            SectionMap::Valid(sections) => {
                let mut pairs: Vec<(String, Vec<String>)> = sections
                    .iter()
                    .map(|section| {
                        let sources: BTreeSet<String> = section
                            .sources
                            .iter()
                            .map(|p| p.as_str().to_string())
                            .collect();
                        (
                            section.id.as_str().to_string(),
                            sources.into_iter().collect(),
                        )
                    })
                    .collect();
                pairs.sort();
                SectionMapIdentity::Valid(pairs)
            }
        }
    }
}

/// The canonical, order-independent form of a README's mapping associations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionMapIdentity {
    Absent,
    Invalid,
    /// Sorted `(id, sorted sources)` pairs.
    Valid(Vec<(String, Vec<String>)>),
}

impl SectionMapIdentity {
    /// A stable small tag for the canonical encoder.
    pub fn tag(&self) -> u64 {
        match self {
            SectionMapIdentity::Absent => 0,
            SectionMapIdentity::Valid(_) => 1,
            SectionMapIdentity::Invalid => 2,
        }
    }

    pub fn pairs(&self) -> &[(String, Vec<String>)] {
        match self {
            SectionMapIdentity::Valid(pairs) => pairs,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::parse(raw).unwrap()
    }

    fn mapping(id: &str, sources: &[&str]) -> SectionMapping {
        SectionMapping {
            id: SectionId::parse(id).unwrap(),
            heading: "Heading".into(),
            first_line: 1,
            last_line: 2,
            sources: sources.iter().map(|s| path(s)).collect(),
            guidance: None,
        }
    }

    #[test]
    fn guidance_paths_are_one_relative_token() {
        for ok in [
            "docs/templates/agent-commands.md",
            "../templates/x.md",
            "./x.md",
            "a/../b.md",
            "guide",
        ] {
            assert!(validate_guidance_path(ok).is_ok(), "{ok}");
        }
        assert_eq!(validate_guidance_path(""), Err(GuidancePathError::Empty));
        assert_eq!(
            validate_guidance_path("/x.md"),
            Err(GuidancePathError::Absolute)
        );
        for (raw, c) in [
            ("a\\b", '\\'),
            ("C:x", ':'),
            ("a#b", '#'),
            ("a?b", '?'),
            ("*.md", '*'),
            ("[a].md", '['),
            ("a].md", ']'),
            ("{a}.md", '{'),
            ("a}.md", '}'),
        ] {
            assert_eq!(
                validate_guidance_path(raw),
                Err(GuidancePathError::Forbidden(c)),
                "{raw}"
            );
        }
        assert_eq!(
            validate_guidance_path("a b.md"),
            Err(GuidancePathError::Whitespace)
        );
        assert_eq!(
            validate_guidance_path("a\u{a0}b"),
            Err(GuidancePathError::Whitespace)
        );
        assert_eq!(validate_guidance_path("a'b"), Err(GuidancePathError::Quote));
        assert_eq!(
            validate_guidance_path("a\"b"),
            Err(GuidancePathError::Quote)
        );
        assert_eq!(
            validate_guidance_path("a\u{1}b"),
            Err(GuidancePathError::Control)
        );
        assert!(validate_guidance_path(&"a".repeat(MAX_SECTION_PATH_BYTES)).is_ok());
        assert_eq!(
            validate_guidance_path(&"a".repeat(MAX_SECTION_PATH_BYTES + 1)),
            Err(GuidancePathError::TooLong)
        );
    }

    #[test]
    fn a_section_guide_never_changes_the_identity() {
        let plain = SectionMap::Valid(vec![mapping("commands", &["justfile"])]);
        let mut guided = mapping("commands", &["justfile"]);
        guided.guidance = Some(path("docs/templates/agent-commands.md"));
        let with_guide = SectionMap::Valid(vec![guided]);
        assert_eq!(plain.identity(), with_guide.identity());
        // A guide-only section contributes its id with no sources, so adding
        // one is a changed identity.
        let mut only = mapping("boundaries", &[]);
        only.guidance = Some(path("docs/templates/agent-rules.md"));
        let with_only = SectionMap::Valid(vec![mapping("commands", &["justfile"]), only]);
        assert_ne!(plain.identity(), with_only.identity());
        assert_eq!(
            with_only.identity().pairs(),
            &[
                ("boundaries".to_string(), vec![]),
                ("commands".to_string(), vec!["justfile".to_string()]),
            ]
        );
        // A guide-only section is never suggested by a change.
        assert!(
            with_only
                .sections_for(&path("docs/templates/agent-rules.md"))
                .is_empty()
        );
    }

    #[test]
    fn section_id_grammar() {
        assert!(SectionId::parse("persistence").is_ok());
        assert!(SectionId::parse("a-b_c9").is_ok());
        assert!(SectionId::parse("9a").is_err());
        assert!(SectionId::parse("").is_err());
        assert!(SectionId::parse("a b").is_err());
        assert!(SectionId::parse(&"a".repeat(64)).is_ok());
        assert!(SectionId::parse(&"a".repeat(65)).is_err());
    }

    #[test]
    fn section_paths_must_be_literal_and_relative() {
        assert!(validate_section_path("service.go").is_ok());
        assert!(validate_section_path("a/b/c.rs").is_ok());
        assert_eq!(validate_section_path(""), Err(SectionPathError::Empty));
        assert_eq!(
            validate_section_path("/etc/passwd"),
            Err(SectionPathError::Absolute)
        );
        assert_eq!(
            validate_section_path("C:/x"),
            Err(SectionPathError::DrivePrefix)
        );
        assert_eq!(
            validate_section_path("a\\b"),
            Err(SectionPathError::Backslash)
        );
        assert_eq!(
            validate_section_path("src/*.rs"),
            Err(SectionPathError::Glob('*'))
        );
        assert_eq!(
            validate_section_path("a//b"),
            Err(SectionPathError::EmptyComponent)
        );
        assert_eq!(
            validate_section_path("../a.rs"),
            Err(SectionPathError::DotComponent)
        );
        assert_eq!(
            validate_section_path("./a.rs"),
            Err(SectionPathError::DotComponent)
        );
        assert_eq!(
            validate_section_path("a\u{7f}b"),
            Err(SectionPathError::Control)
        );
        assert_eq!(
            validate_section_path("a\u{a0}b"),
            Err(SectionPathError::Whitespace)
        );
        assert_eq!(validate_section_path("a\"b"), Err(SectionPathError::Quote));
        assert_eq!(
            validate_section_path(&"a".repeat(MAX_SECTION_PATH_BYTES + 1)),
            Err(SectionPathError::TooLong)
        );
    }

    #[test]
    fn identity_ignores_author_order_and_presentation() {
        let a = SectionMap::Valid(vec![
            mapping("beta", &["b.rs", "a.rs"]),
            mapping("alpha", &["c.rs"]),
        ]);
        let mut moved = mapping("alpha", &["c.rs"]);
        moved.heading = "Different words".into();
        moved.first_line = 90;
        moved.last_line = 120;
        let b = SectionMap::Valid(vec![moved, mapping("beta", &["a.rs", "b.rs"])]);
        assert_eq!(a.identity(), b.identity());
        // A changed association is a different identity.
        let c = SectionMap::Valid(vec![
            mapping("beta", &["b.rs"]),
            mapping("alpha", &["c.rs"]),
        ]);
        assert_ne!(a.identity(), c.identity());
        // Absent, invalid, and empty-but-valid are three distinct states.
        assert_ne!(
            SectionMap::Absent.identity(),
            SectionMap::Invalid.identity()
        );
        assert_ne!(
            SectionMap::Absent.identity(),
            SectionMap::Valid(vec![]).identity()
        );
    }

    #[test]
    fn one_source_can_belong_to_several_sections() {
        let map = SectionMap::Valid(vec![
            mapping("one", &["shared.rs", "a.rs"]),
            mapping("two", &["shared.rs"]),
        ]);
        let hits = map.sections_for(&path("shared.rs"));
        assert_eq!(hits.len(), 2);
        assert_eq!(map.sections_for(&path("a.rs")).len(), 1);
        assert!(map.sections_for(&path("absent.rs")).is_empty());
        // An invalid map advertises nothing at all.
        assert!(
            SectionMap::Invalid
                .sections_for(&path("shared.rs"))
                .is_empty()
        );
    }
}
