//! The one glob matcher for Memoria selection rules and section mappings.
//!
//! Patterns match whole paths relative to their rule scope. Supported syntax:
//! `*` (any characters within one component), `?` (one character), character
//! classes such as `[abc]` or `[a-z]` with a leading `!` or `^` for negation,
//! and the `**` component that matches zero or more directories. A trailing
//! `**` matches everything inside a directory, but not a file with that name.
//! Braces are ordinary characters.
//!
//! The `glob` crate compiles and matches each pattern, once per pattern, by
//! Unicode character. This module adds the Memoria grammar around it: the
//! rejected forms below, `.` components that are skipped, and a run of `*`
//! inside one component that means one `*`. Contextual rules, such as `!` for
//! a section exclusion, belong to the callers.

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
    /// Any other syntax error that the matcher reports.
    Syntax {
        pattern: String,
        message: String,
    },
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
            GlobError::Syntax { pattern, message } => write!(f, "pattern {pattern:?}: {message}"),
        }
    }
}

impl std::error::Error for GlobError {}

/// Whether an authored token uses glob syntax: `*`, `?`, or `[`.
pub fn is_pattern(token: &str) -> bool {
    token.contains(['*', '?', '['])
}

const OPTIONS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// A compiled glob pattern.
#[derive(Clone)]
pub struct Glob {
    source: String,
    pattern: glob::Pattern,
}

impl PartialEq for Glob {
    fn eq(&self, other: &Glob) -> bool {
        self.source == other.source
    }
}

impl Eq for Glob {}

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
        let mut components = Vec::new();
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
            components.push(
                normalize_component(component)
                    .ok_or_else(|| GlobError::UnterminatedClass(source.to_string()))?,
            );
        }
        if components.is_empty() {
            return Err(GlobError::Empty);
        }
        let pattern = glob::Pattern::new(&components.join("/")).map_err(|err| {
            if err.msg == "invalid range pattern" {
                GlobError::UnterminatedClass(source.to_string())
            } else {
                GlobError::Syntax {
                    pattern: source.to_string(),
                    message: err.msg.to_string(),
                }
            }
        })?;
        Ok(Glob {
            source: source.to_string(),
            pattern,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Match a scope-relative path with `/` separators.
    pub fn matches(&self, path: &str) -> bool {
        self.pattern.matches_with(path, OPTIONS)
    }
}

/// Rewrite one component into the matcher's syntax with the same meaning: a
/// run of `*` outside a class is one `*`, unless the whole component is the
/// recursive `**`, and a class negated with `^` is negated with `!`. `None`
/// when a class does not close inside this component.
fn normalize_component(component: &str) -> Option<String> {
    if component == "**" {
        return Some(component.to_string());
    }
    let mut out = String::with_capacity(component.len());
    let mut chars = component.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                out.push('*');
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
            }
            '[' => {
                out.push('[');
                if chars.next_if(|c| *c == '!' || *c == '^').is_some() {
                    out.push('!');
                }
                // The first character of a class is literal, even `]`.
                out.push(chars.next()?);
                loop {
                    let c = chars.next()?;
                    out.push(c);
                    if c == ']' {
                        break;
                    }
                }
            }
            c => out.push(c),
        }
    }
    Some(out)
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
    fn keeps_the_released_grammar() {
        // One character is one Unicode scalar value, not one byte.
        assert!(m("caf?.txt", "café.txt"));
        assert!(m("[!a]", "ñ"));
        assert!(m("[^a]", "b"));
        assert!(!m("[^a]", "a"));
        // Braces are ordinary characters.
        assert!(m("{a,b}", "{a,b}"));
        assert!(!m("{a,b}", "a"));
        // A run of stars inside one component is one star.
        assert!(m("a**b", "axxb"));
        assert!(!m("a**b", "a/b"));
        assert!(m("src/**.rs", "src/a.rs"));
        assert!(m("***", "a"));
        // `.` components are skipped.
        assert!(m("./x", "x"));
        assert!(m("x/./y", "x/y"));
        // A class keeps a leading `]` and its stars.
        assert!(m("[]a]", "]"));
        assert!(m("[*]", "*"));
        assert!(!m("[*]", "a"));
        // A reversed range matches nothing.
        assert!(!m("[z-a]", "m"));
        // A leading dot is an ordinary character.
        assert!(m("*", ".hidden"));
        assert!(m("**/*.rs", ".cache/a.rs"));
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
        assert!(matches!(
            Glob::parse("a[/]b"),
            Err(GlobError::UnterminatedClass(_))
        ));
        assert!(matches!(Glob::parse(""), Err(GlobError::Empty)));
        assert!(matches!(Glob::parse("./."), Err(GlobError::Empty)));
        assert!(matches!(
            Glob::parse("a//b"),
            Err(GlobError::EmptyComponent(_))
        ));
    }
}
