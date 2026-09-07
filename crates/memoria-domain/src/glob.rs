//! A small, deterministic glob matcher for Memoria ignore and include rules.
//!
//! Patterns match whole paths relative to their rule scope. Supported syntax:
//! `*` (any characters within one component), `?` (one character), character
//! classes such as `[abc]` or `[a-z]` with a leading `!` for negation, and the
//! `**` component that matches zero or more directories. A trailing `**`
//! matches everything inside a directory, but not a file with that name.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobError {
    Empty,
    Negation(String),
    Absolute(String),
    ParentComponent(String),
    Backslash(String),
    TrailingSlash(String),
    UnterminatedClass(String),
    EmptyComponent(String),
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlobError::Empty => write!(f, "pattern is empty"),
            GlobError::Negation(p) => {
                write!(f, "pattern {p:?} uses an unsupported negation prefix")
            }
            GlobError::Absolute(p) => write!(f, "pattern {p:?} must be relative to its scope"),
            GlobError::ParentComponent(p) => write!(f, "pattern {p:?} contains a `..` component"),
            GlobError::Backslash(p) => write!(f, "pattern {p:?} contains a backslash"),
            GlobError::TrailingSlash(p) => write!(
                f,
                "pattern {p:?} ends with `/`; use `{p}**` for a directory"
            ),
            GlobError::UnterminatedClass(p) => {
                write!(f, "pattern {p:?} has an unterminated character class")
            }
            GlobError::EmptyComponent(p) => write!(f, "pattern {p:?} contains an empty component"),
        }
    }
}

impl std::error::Error for GlobError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Literal(char),
    Any,
    One,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    DoubleStar,
    Tokens(Vec<Token>),
}

/// A compiled glob pattern.
#[derive(Clone, PartialEq, Eq)]
pub struct Glob {
    source: String,
    segments: Vec<Segment>,
}

impl fmt::Debug for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Glob({:?})", self.source)
    }
}

impl Glob {
    pub fn parse(source: &str) -> Result<Glob, GlobError> {
        if source.is_empty() {
            return Err(GlobError::Empty);
        }
        if source.starts_with('!') {
            return Err(GlobError::Negation(source.to_string()));
        }
        if source.starts_with('/') {
            return Err(GlobError::Absolute(source.to_string()));
        }
        if source.contains('\\') {
            return Err(GlobError::Backslash(source.to_string()));
        }
        if source.ends_with('/') {
            return Err(GlobError::TrailingSlash(source.to_string()));
        }
        let mut segments = Vec::new();
        for component in source.split('/') {
            if component.is_empty() {
                return Err(GlobError::EmptyComponent(source.to_string()));
            }
            if component == ".." {
                return Err(GlobError::ParentComponent(source.to_string()));
            }
            if component == "." {
                continue;
            }
            if component == "**" {
                segments.push(Segment::DoubleStar);
                continue;
            }
            segments.push(Segment::Tokens(parse_tokens(component, source)?));
        }
        if segments.is_empty() {
            return Err(GlobError::Empty);
        }
        Ok(Glob {
            source: source.to_string(),
            segments,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Match a scope-relative path with `/` separators.
    pub fn matches(&self, path: &str) -> bool {
        let components: Vec<&str> = path.split('/').collect();
        match_segments(&self.segments, &components)
    }
}

fn parse_tokens(component: &str, source: &str) -> Result<Vec<Token>, GlobError> {
    let chars: Vec<char> = component.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                tokens.push(Token::Any);
                // Collapse repeated stars inside one component.
                while i + 1 < chars.len() && chars[i + 1] == '*' {
                    i += 1;
                }
            }
            '?' => tokens.push(Token::One),
            '[' => {
                let mut j = i + 1;
                let negated = j < chars.len() && (chars[j] == '!' || chars[j] == '^');
                if negated {
                    j += 1;
                }
                let mut ranges = Vec::new();
                let mut first = true;
                loop {
                    if j >= chars.len() {
                        return Err(GlobError::UnterminatedClass(source.to_string()));
                    }
                    let c = chars[j];
                    if c == ']' && !first {
                        break;
                    }
                    first = false;
                    if j + 2 < chars.len() && chars[j + 1] == '-' && chars[j + 2] != ']' {
                        ranges.push((c, chars[j + 2]));
                        j += 3;
                    } else {
                        ranges.push((c, c));
                        j += 1;
                    }
                }
                tokens.push(Token::Class { negated, ranges });
                i = j;
            }
            c => tokens.push(Token::Literal(c)),
        }
        i += 1;
    }
    Ok(tokens)
}

fn match_segments(segments: &[Segment], components: &[&str]) -> bool {
    match segments.first() {
        None => components.is_empty(),
        Some(Segment::DoubleStar) if segments.len() == 1 => {
            // A trailing `**` matches everything inside a directory, but not
            // a file with the directory's name.
            !components.is_empty()
        }
        Some(Segment::DoubleStar) => {
            // A leading or middle `**` matches zero or more components.
            (0..=components.len()).any(|skip| match_segments(&segments[1..], &components[skip..]))
        }
        Some(Segment::Tokens(tokens)) => match components.first() {
            None => false,
            Some(component) => {
                let chars: Vec<char> = component.chars().collect();
                match_tokens(tokens, &chars) && match_segments(&segments[1..], &components[1..])
            }
        },
    }
}

fn match_tokens(tokens: &[Token], chars: &[char]) -> bool {
    match tokens.first() {
        None => chars.is_empty(),
        Some(Token::Any) => {
            (0..=chars.len()).any(|skip| match_tokens(&tokens[1..], &chars[skip..]))
        }
        Some(Token::One) => !chars.is_empty() && match_tokens(&tokens[1..], &chars[1..]),
        Some(Token::Literal(expected)) => {
            chars.first() == Some(expected) && match_tokens(&tokens[1..], &chars[1..])
        }
        Some(Token::Class { negated, ranges }) => match chars.first() {
            None => false,
            Some(c) => {
                let inside = ranges.iter().any(|(lo, hi)| lo <= c && c <= hi);
                inside != *negated && match_tokens(&tokens[1..], &chars[1..])
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, path: &str) -> bool {
        Glob::parse(pattern).unwrap().matches(path)
    }

    #[test]
    fn matches_double_star_across_directories() {
        assert!(m("**/generated/**", "src/retrieval/generated/table.rs"));
        assert!(m("**/generated/**", "generated/table.rs"));
        assert!(!m("**/generated/**", "src/generated"));
        assert!(m("**/*.snap", "a/b/c.snap"));
        assert!(m("**/*.snap", "c.snap"));
        assert!(m("fixtures/**", "fixtures/sample.txt"));
        assert!(m("fixtures/**", "fixtures/deep/sample.txt"));
        assert!(!m("fixtures/**", "other/fixtures/sample.txt"));
        assert!(m("src/**/README.md", "src/README.md"));
    }

    #[test]
    fn single_star_does_not_cross_separators() {
        assert!(m("*.rs", "main.rs"));
        assert!(!m("*.rs", "src/main.rs"));
        assert!(m("src/*.rs", "src/main.rs"));
        assert!(m("src/?ain.rs", "src/main.rs"));
        assert!(m("src/[a-m]ain.rs", "src/main.rs"));
        assert!(!m("src/[!a-m]ain.rs", "src/main.rs"));
        assert!(m("a*b*c", "axxbyyc"));
        assert!(m("data/módulo/*.txt", "data/módulo/ñ.txt"));
    }

    #[test]
    fn rejects_unsupported_forms() {
        assert!(matches!(Glob::parse("!x"), Err(GlobError::Negation(_))));
        assert!(matches!(Glob::parse("/x"), Err(GlobError::Absolute(_))));
        assert!(matches!(
            Glob::parse("../x"),
            Err(GlobError::ParentComponent(_))
        ));
        assert!(matches!(
            Glob::parse("x/"),
            Err(GlobError::TrailingSlash(_))
        ));
        assert!(matches!(
            Glob::parse("x/[a"),
            Err(GlobError::UnterminatedClass(_))
        ));
        assert!(matches!(Glob::parse(""), Err(GlobError::Empty)));
        assert!(matches!(
            Glob::parse("a//b"),
            Err(GlobError::EmptyComponent(_))
        ));
    }
}
