//! Advisory section mappings.
//!
//! A section is an optional reading hint authored in a tracked document. Its
//! `files` name selected sources in that document's scope: literal paths,
//! glob patterns that expand over the scope, and `!` exclusions that subtract
//! from both. It adds or
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

use crate::glob::{Glob, GlobError, is_pattern};
use crate::path::{DirPath, ProjectPath};

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

/// Why one authored `files` token is not a usable path or pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionPathError {
    Empty,
    Absolute,
    DrivePrefix,
    Backslash,
    /// A character that neither a literal path nor a pattern may contain.
    Glob(char),
    /// The pattern does not compile.
    Pattern(GlobError),
    /// An exclusion must name a path or a pattern after exactly one `!`.
    Negation,
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
                "the character {c:?} is not permitted; a pattern uses only `*`, `?`, `**`, and `[...]`"
            ),
            SectionPathError::Pattern(err) => write!(f, "{err}"),
            SectionPathError::Negation => {
                f.write_str("an exclusion is one `!` followed by a path or a pattern")
            }
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

/// Check one authored literal `files` token before it is resolved against
/// the document's directory. Every rejection is explicit; nothing is
/// repaired, percent-decoded, or escaped.
pub fn validate_section_path(raw: &str) -> Result<(), SectionPathError> {
    check_section_token(raw, false)
}

/// The grammar shared by literal paths and patterns. A pattern may also use
/// `*`, `?`, `[`, and `]`. Braces are never permitted.
fn check_section_token(raw: &str, pattern: bool) -> Result<(), SectionPathError> {
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
            '*' | '?' | '[' | ']' if pattern => {}
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

/// What one `files` token names, relative to the document's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionTarget {
    /// A literal path. It must name a selected source in the scope.
    Path(String),
    /// A glob pattern, matched against every source in the scope.
    Pattern(Glob),
}

/// One authored `files` token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRule {
    /// The token exactly as authored, including a leading `!`.
    pub token: String,
    /// A `!` token subtracts its matches from the section.
    pub exclude: bool,
    pub target: SectionTarget,
}

impl SectionRule {
    /// Parse one token. A leading `!` makes an exclusion. A token with `*`,
    /// `?`, or `[` is a pattern; any other token is a literal path.
    pub fn parse(token: &str) -> Result<SectionRule, SectionPathError> {
        let (exclude, raw) = match token.strip_prefix('!') {
            Some(rest) if rest.starts_with('!') => return Err(SectionPathError::Negation),
            Some(rest) => (true, rest),
            None => (false, token),
        };
        let target = if is_pattern(raw) {
            check_section_token(raw, true)?;
            SectionTarget::Pattern(Glob::parse(raw).map_err(SectionPathError::Pattern)?)
        } else {
            check_section_token(raw, false)?;
            SectionTarget::Path(raw.to_string())
        };
        Ok(SectionRule {
            token: token.to_string(),
            exclude,
            target,
        })
    }

    /// Whether this token names `path`, ignoring `!`.
    pub fn matches(&self, dir: &DirPath, path: &ProjectPath) -> bool {
        match &self.target {
            SectionTarget::Path(raw) => {
                ProjectPath::resolve_relative(dir, raw).is_ok_and(|named| &named == path)
            }
            SectionTarget::Pattern(glob) => {
                path.strip_dir(dir).is_some_and(|rel| glob.matches(rel))
            }
        }
    }
}

/// The `files` attribute of one section: includes and exclusions in authored
/// order. The order never changes the result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionFiles(Vec<SectionRule>);

/// A `files` attribute with exclusions and nothing to include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NothingIncluded;

impl fmt::Display for NothingIncluded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("section `files` needs at least one path or pattern to include; a `!` token only removes files")
    }
}

/// The sources that one section maps in its document's scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionExpansion {
    /// Every included source minus every excluded one, sorted.
    pub sources: Vec<ProjectPath>,
    /// The sources that a literal include names and no exclusion removes.
    pub literals: Vec<ProjectPath>,
    /// Tokens that match nothing: an include pattern with no source in the
    /// scope, or an exclusion that removes no included source.
    pub unmatched: Vec<String>,
}

impl SectionFiles {
    /// A guide-only section has no `files`.
    pub fn none() -> SectionFiles {
        SectionFiles(Vec::new())
    }

    pub fn new(rules: Vec<SectionRule>) -> Result<SectionFiles, NothingIncluded> {
        if !rules.is_empty() && rules.iter().all(|rule| rule.exclude) {
            return Err(NothingIncluded);
        }
        Ok(SectionFiles(rules))
    }

    pub fn rules(&self) -> &[SectionRule] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The authored tokens, in authored order.
    pub fn tokens(&self) -> Vec<String> {
        self.0.iter().map(|rule| rule.token.clone()).collect()
    }

    /// Whether any token is a pattern, included or excluded.
    pub fn has_patterns(&self) -> bool {
        self.0
            .iter()
            .any(|rule| matches!(rule.target, SectionTarget::Pattern(_)))
    }

    /// The literal include tokens, which must each name a source in scope.
    pub fn literal_includes(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|rule| match &rule.target {
            SectionTarget::Path(raw) if !rule.exclude => Some(raw.as_str()),
            _ => None,
        })
    }

    /// The include tokens that name `path`, in authored order. An
    /// exclusion that also names it is not listed.
    pub fn includes_naming(&self, dir: &DirPath, path: &ProjectPath) -> Vec<String> {
        self.0
            .iter()
            .filter(|rule| !rule.exclude && rule.matches(dir, path))
            .map(|rule| rule.token.clone())
            .collect()
    }

    /// Expand the rules over a document's scope. The union of all includes
    /// minus the union of all exclusions, so authored order never matters.
    /// `Err` lists the literal include tokens that name no source in `scope`.
    pub fn expand(
        &self,
        dir: &DirPath,
        scope: &BTreeSet<&ProjectPath>,
    ) -> Result<SectionExpansion, Vec<String>> {
        let mut included: BTreeSet<ProjectPath> = BTreeSet::new();
        let mut literals: BTreeSet<ProjectPath> = BTreeSet::new();
        let mut missing = Vec::new();
        let mut unmatched = Vec::new();
        for rule in self.0.iter().filter(|rule| !rule.exclude) {
            match &rule.target {
                SectionTarget::Path(raw) => match ProjectPath::resolve_relative(dir, raw) {
                    Ok(path) if scope.contains(&path) => {
                        literals.insert(path.clone());
                        included.insert(path);
                    }
                    _ => missing.push(rule.token.clone()),
                },
                SectionTarget::Pattern(_) => {
                    let mut matched = false;
                    for path in scope {
                        if rule.matches(dir, path) {
                            matched = true;
                            included.insert((*path).clone());
                        }
                    }
                    if !matched {
                        unmatched.push(rule.token.clone());
                    }
                }
            }
        }
        if !missing.is_empty() {
            return Err(missing);
        }
        // Every exclusion sees the same included set, so its hint does not
        // depend on the order of the other exclusions.
        let mut removed: BTreeSet<ProjectPath> = BTreeSet::new();
        for rule in self.0.iter().filter(|rule| rule.exclude) {
            let hits: Vec<&ProjectPath> = included
                .iter()
                .filter(|path| rule.matches(dir, path))
                .collect();
            if hits.is_empty() {
                unmatched.push(rule.token.clone());
            }
            removed.extend(hits.into_iter().cloned());
        }
        let sources: Vec<ProjectPath> = included.difference(&removed).cloned().collect();
        let literals = literals.difference(&removed).cloned().collect();
        Ok(SectionExpansion {
            sources,
            literals,
            unmatched,
        })
    }
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
    /// The authored `files` rules. Empty only for a guide-only section.
    pub files: SectionFiles,
    /// Sorted, deduplicated sources in the scope that the rules expand to.
    /// Empty for a guide-only section or when no source matches.
    pub sources: Vec<ProjectPath>,
    /// The sources that a literal include names, after exclusions. A
    /// suggestion reads all of them; it reads a pattern match only when that
    /// match changed.
    pub literals: Vec<ProjectPath>,
    /// Tokens that match nothing in the current scope. A lint hint only.
    pub unmatched: Vec<String>,
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
    /// The set holds expanded sources, never authored tokens: a pattern and
    /// the literal list it matches are the same association. Body edits,
    /// heading text, moved line ranges, and the section guide do not change
    /// this identity. A guide-only section contributes its id with
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
            files: SectionFiles::none(),
            sources: sources.iter().map(|s| path(s)).collect(),
            literals: sources.iter().map(|s| path(s)).collect(),
            unmatched: Vec::new(),
            guidance: None,
        }
    }

    fn files(tokens: &[&str]) -> SectionFiles {
        SectionFiles::new(
            tokens
                .iter()
                .map(|token| SectionRule::parse(token).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn expand(dir: &str, tokens: &[&str], scope: &[&str]) -> Result<SectionExpansion, Vec<String>> {
        let scope: Vec<ProjectPath> = scope.iter().map(|s| path(s)).collect();
        let scope: BTreeSet<&ProjectPath> = scope.iter().collect();
        files(tokens).expand(&DirPath::parse(dir).unwrap(), &scope)
    }

    fn strings(paths: &[ProjectPath]) -> Vec<&str> {
        paths.iter().map(ProjectPath::as_str).collect()
    }

    #[test]
    fn tokens_are_literals_patterns_or_exclusions() {
        let rule = SectionRule::parse("src/auth/login.rs").unwrap();
        assert!(!rule.exclude);
        assert_eq!(rule.target, SectionTarget::Path("src/auth/login.rs".into()));
        let rule = SectionRule::parse("!src/auth/tests/**").unwrap();
        assert!(rule.exclude);
        assert_eq!(rule.token, "!src/auth/tests/**");
        assert!(matches!(rule.target, SectionTarget::Pattern(_)));
        assert!(matches!(
            SectionRule::parse("!generated.rs").unwrap().target,
            SectionTarget::Path(_)
        ));
        // Contextual rules stay here; the glob grammar is shared.
        for (token, err) in [
            ("!", SectionPathError::Empty),
            ("!!x.rs", SectionPathError::Negation),
            ("src/{a,b}.rs", SectionPathError::Glob('{')),
            ("src/a].rs", SectionPathError::Glob(']')),
            ("src/*.rs\\x", SectionPathError::Backslash),
            ("./src/*.rs", SectionPathError::DotComponent),
            ("../*.rs", SectionPathError::DotComponent),
            ("/src/*.rs", SectionPathError::Absolute),
            ("src//*.rs", SectionPathError::EmptyComponent),
            ("src/", SectionPathError::EmptyComponent),
        ] {
            assert_eq!(SectionRule::parse(token).unwrap_err(), err, "{token}");
        }
        assert!(matches!(
            SectionRule::parse("src/[a.rs"),
            Err(SectionPathError::Pattern(GlobError::UnterminatedClass(_)))
        ));
        // Only exclusions is an error; no files at all is a guide-only section.
        assert_eq!(
            SectionFiles::new(vec![SectionRule::parse("!a.rs").unwrap()]),
            Err(NothingIncluded)
        );
        assert!(SectionFiles::new(vec![]).unwrap().is_empty());
    }

    const AUTH: [&str; 5] = [
        "src/auth/generated.rs",
        "src/auth/login.rs",
        "src/auth/passkeys.rs",
        "src/auth/tests/login.rs",
        "src/main.rs",
    ];

    #[test]
    fn includes_union_and_exclusions_subtract_in_any_order() {
        let tokens = [
            "src/auth/**",
            "!src/auth/tests/**",
            "!src/auth/generated.rs",
        ];
        let expected = ["src/auth/login.rs", "src/auth/passkeys.rs"];
        let mut orders = vec![tokens.to_vec()];
        orders.push(tokens.iter().rev().copied().collect());
        orders.push(vec![tokens[1], tokens[0], tokens[2]]);
        for order in orders {
            let out = expand("", &order, &AUTH).unwrap();
            assert_eq!(strings(&out.sources), expected, "{order:?}");
            assert!(out.literals.is_empty());
            assert!(out.unmatched.is_empty());
        }
        // Overlapping includes are one union, and a literal that a pattern
        // also matches is listed once.
        let out = expand("", &["src/auth/*.rs", "src/auth/login.rs", "src/**"], &AUTH).unwrap();
        assert_eq!(out.sources.len(), 5);
        assert_eq!(strings(&out.literals), ["src/auth/login.rs"]);
        // An exclusion also subtracts a literal include.
        let out = expand(
            "",
            &["src/auth/login.rs", "src/main.rs", "!src/auth/**"],
            &AUTH,
        )
        .unwrap();
        assert_eq!(strings(&out.sources), ["src/main.rs"]);
        assert_eq!(strings(&out.literals), ["src/main.rs"]);
    }

    #[test]
    fn tokens_resolve_against_the_document_directory() {
        let out = expand("src", &["auth/*.rs", "!auth/generated.rs"], &AUTH[..4]).unwrap();
        assert_eq!(
            strings(&out.sources),
            ["src/auth/login.rs", "src/auth/passkeys.rs"]
        );
        // A pattern never reaches a path outside the given scope.
        let out = expand("src/auth", &["**"], &AUTH[..2]).unwrap();
        assert_eq!(out.sources.len(), 2);
    }

    #[test]
    fn empty_matches_are_hints_and_missing_literals_are_errors() {
        let out = expand("", &["docs/**", "src/main.rs", "!src/auth/**"], &AUTH).unwrap();
        assert_eq!(strings(&out.sources), ["src/main.rs"]);
        assert_eq!(out.unmatched, ["docs/**", "!src/auth/**"]);
        // A section that expands to nothing is still a valid mapping.
        let out = expand("", &["docs/**"], &AUTH).unwrap();
        assert!(out.sources.is_empty());
        // A literal include must name a source in the scope.
        assert_eq!(
            expand("", &["src/gone.rs", "src/**", "lib.rs"], &AUTH).unwrap_err(),
            ["src/gone.rs", "lib.rs"]
        );
        // A literal exclusion of a missing file is only unmatched.
        let out = expand("", &["src/**", "!src/gone.rs"], &AUTH).unwrap();
        assert_eq!(out.unmatched, ["!src/gone.rs"]);
    }

    #[test]
    fn a_pattern_follows_additions_deletions_and_renames() {
        let tokens = ["src/auth/**", "!src/auth/tests/**"];
        let before = expand("", &tokens, &AUTH).unwrap();
        let mut renamed: Vec<&str> = AUTH.to_vec();
        renamed.retain(|p| *p != "src/auth/login.rs");
        renamed.push("src/auth/sign_in.rs");
        renamed.retain(|p| *p != "src/auth/generated.rs");
        let after = expand("", &tokens, &renamed).unwrap();
        assert_eq!(strings(&before.sources).len(), 3);
        assert_eq!(
            strings(&after.sources),
            ["src/auth/passkeys.rs", "src/auth/sign_in.rs"]
        );
        // The same rename makes a literal mapping unusable.
        assert!(expand("", &["src/auth/login.rs"], &renamed).is_err());
    }

    #[test]
    fn identity_is_the_expansion_not_the_tokens() {
        let scope: Vec<ProjectPath> = AUTH.iter().map(|s| path(s)).collect();
        let scope: BTreeSet<&ProjectPath> = scope.iter().collect();
        let dir = DirPath::root();
        let map = |tokens: &[&str]| {
            let rules = files(tokens);
            let expansion = rules.expand(&dir, &scope).unwrap();
            SectionMap::Valid(vec![SectionMapping {
                files: rules,
                sources: expansion.sources,
                literals: expansion.literals,
                ..mapping("auth", &[])
            }])
        };
        let pattern = map(&["src/auth/*.rs", "!src/auth/generated.rs"]);
        let literal = map(&["src/auth/login.rs", "src/auth/passkeys.rs"]);
        assert_eq!(pattern.identity(), literal.identity());
        assert_ne!(pattern.identity(), map(&["src/auth/*.rs"]).identity());
        assert_eq!(pattern.sections_for(&path("src/auth/passkeys.rs")).len(), 1);
        assert!(
            pattern
                .sections_for(&path("src/auth/generated.rs"))
                .is_empty()
        );
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
