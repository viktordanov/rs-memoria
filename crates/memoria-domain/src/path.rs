//! Validated project-relative paths and document identities.

use std::fmt;

/// A validation failure for a project path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Absolute(String),
    Escapes(String),
    Backslash(String),
    ControlCharacter(String),
    EmptyComponent(String),
    NotReadme(String),
    FileNameMissing(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => write!(f, "path is empty"),
            PathError::Absolute(p) => write!(f, "path {p:?} is absolute"),
            PathError::Escapes(p) => write!(f, "path {p:?} escapes the project root"),
            PathError::Backslash(p) => write!(f, "path {p:?} contains a backslash"),
            PathError::ControlCharacter(p) => write!(f, "path {p:?} contains a control character"),
            PathError::EmptyComponent(p) => write!(f, "path {p:?} contains an empty component"),
            PathError::NotReadme(p) => write!(f, "path {p:?} is not a README.md document"),
            PathError::FileNameMissing(p) => write!(f, "path {p:?} has no file name"),
        }
    }
}

impl std::error::Error for PathError {}

/// The name of every recognized documentation boundary file.
pub const README_FILE_NAME: &str = "README.md";

fn check_bytes(raw: &str) -> Result<(), PathError> {
    if raw.is_empty() {
        return Err(PathError::Empty);
    }
    if raw.contains('\\') {
        return Err(PathError::Backslash(raw.to_string()));
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(PathError::ControlCharacter(raw.to_string()));
    }
    if raw.starts_with('/') {
        return Err(PathError::Absolute(raw.to_string()));
    }
    Ok(())
}

/// Lexically normalize `.` and `..` components. Returns the normalized
/// components; an escape above the root is an error.
fn normalize(raw: &str) -> Result<Vec<&str>, PathError> {
    let trimmed = raw.strip_suffix('/').unwrap_or(raw);
    let mut out: Vec<&str> = Vec::new();
    for component in trimmed.split('/') {
        match component {
            "" => return Err(PathError::EmptyComponent(raw.to_string())),
            "." => continue,
            ".." => {
                if out.pop().is_none() {
                    return Err(PathError::Escapes(raw.to_string()));
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// A normalized, root-relative directory path. The root is the empty path.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DirPath(String);

impl DirPath {
    /// The project root directory.
    pub const fn root() -> DirPath {
        DirPath(String::new())
    }

    /// Parse a directory path. `""`, `"."` and `"./"` denote the root.
    pub fn parse(raw: &str) -> Result<DirPath, PathError> {
        if raw.is_empty() || raw == "." || raw == "./" {
            return Ok(DirPath::root());
        }
        check_bytes(raw)?;
        let components = normalize(raw)?;
        Ok(DirPath(components.join("/")))
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parent directory, or `None` at the root.
    pub fn parent(&self) -> Option<DirPath> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rfind('/') {
            Some(index) => DirPath(self.0[..index].to_string()),
            None => DirPath::root(),
        })
    }

    /// This directory followed by each ancestor up to and including the root.
    pub fn ancestors(&self) -> Vec<DirPath> {
        let mut out = vec![self.clone()];
        let mut current = self.clone();
        while let Some(parent) = current.parent() {
            out.push(parent.clone());
            current = parent;
        }
        out
    }

    /// Whether `self` equals `other` or is nested below it.
    pub fn is_within(&self, other: &DirPath) -> bool {
        other.is_root()
            || self == other
            || (self.0.len() > other.0.len()
                && self.0.starts_with(other.as_str())
                && self.0.as_bytes()[other.0.len()] == b'/')
    }

    /// Join a validated relative path below this directory.
    pub fn join(&self, relative: &str) -> Result<ProjectPath, PathError> {
        if self.is_root() {
            ProjectPath::parse(relative)
        } else {
            ProjectPath::parse(&format!("{}/{}", self.0, relative))
        }
    }

    /// The path of the README document that would sit in this directory.
    pub fn readme(&self) -> DocumentId {
        let path = if self.is_root() {
            README_FILE_NAME.to_string()
        } else {
            format!("{}/{}", self.0, README_FILE_NAME)
        };
        DocumentId(ProjectPath(path))
    }

    /// Depth of the directory: zero at the root.
    pub fn depth(&self) -> usize {
        if self.is_root() {
            0
        } else {
            self.0.split('/').count()
        }
    }
}

impl fmt::Debug for DirPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DirPath({:?})", self.0)
    }
}

impl fmt::Display for DirPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            f.write_str(".")
        } else {
            f.write_str(&self.0)
        }
    }
}

/// A normalized, root-relative file path using `/` separators.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectPath(String);

impl ProjectPath {
    /// Parse and normalize a root-relative file path.
    pub fn parse(raw: &str) -> Result<ProjectPath, PathError> {
        check_bytes(raw)?;
        if raw.ends_with('/') {
            return Err(PathError::FileNameMissing(raw.to_string()));
        }
        let components = normalize(raw)?;
        if components.is_empty() {
            return Err(PathError::FileNameMissing(raw.to_string()));
        }
        Ok(ProjectPath(components.join("/")))
    }

    /// Resolve a path written relative to `base`, rejecting escapes.
    pub fn resolve_relative(base: &DirPath, relative: &str) -> Result<ProjectPath, PathError> {
        check_bytes(relative)?;
        base.join(relative)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn file_name(&self) -> &str {
        match self.0.rfind('/') {
            Some(index) => &self.0[index + 1..],
            None => &self.0,
        }
    }

    /// Directory that contains this file.
    pub fn directory(&self) -> DirPath {
        match self.0.rfind('/') {
            Some(index) => DirPath(self.0[..index].to_string()),
            None => DirPath::root(),
        }
    }

    pub fn is_within(&self, dir: &DirPath) -> bool {
        self.directory().is_within(dir)
    }

    /// The path relative to `dir`, when the file sits within it.
    pub fn strip_dir<'a>(&'a self, dir: &DirPath) -> Option<&'a str> {
        if dir.is_root() {
            Some(&self.0)
        } else if self.is_within(dir) {
            Some(&self.0[dir.0.len() + 1..])
        } else {
            None
        }
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    pub fn is_readme(&self) -> bool {
        self.file_name() == README_FILE_NAME
    }
}

impl fmt::Debug for ProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProjectPath({:?})", self.0)
    }
}

impl fmt::Display for ProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of a README document: its root-relative path.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(ProjectPath);

impl DocumentId {
    pub fn parse(raw: &str) -> Result<DocumentId, PathError> {
        let path = ProjectPath::parse(raw)?;
        DocumentId::from_path(path)
    }

    pub fn from_path(path: ProjectPath) -> Result<DocumentId, PathError> {
        if path.is_readme() {
            Ok(DocumentId(path))
        } else {
            Err(PathError::NotReadme(path.0))
        }
    }

    pub fn path(&self) -> &ProjectPath {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Directory that the document describes.
    pub fn directory(&self) -> DirPath {
        self.0.directory()
    }

    pub fn is_root(&self) -> bool {
        self.directory().is_root()
    }
}

impl fmt::Debug for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocumentId({:?})", self.0.0)
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_dot_components() {
        assert_eq!(
            ProjectPath::parse("./a/./b/../c.rs").unwrap().as_str(),
            "a/c.rs"
        );
        assert_eq!(DirPath::parse("src/").unwrap().as_str(), "src");
        assert_eq!(DirPath::parse(".").unwrap(), DirPath::root());
    }

    #[test]
    fn rejects_invalid_forms() {
        assert_eq!(ProjectPath::parse(""), Err(PathError::Empty));
        assert!(matches!(
            ProjectPath::parse("/etc/passwd"),
            Err(PathError::Absolute(_))
        ));
        assert!(matches!(
            ProjectPath::parse("../x"),
            Err(PathError::Escapes(_))
        ));
        assert!(matches!(
            ProjectPath::parse("a/../../x"),
            Err(PathError::Escapes(_))
        ));
        assert!(matches!(
            ProjectPath::parse("a\\b"),
            Err(PathError::Backslash(_))
        ));
        assert!(matches!(
            ProjectPath::parse("a\tb"),
            Err(PathError::ControlCharacter(_))
        ));
        assert!(matches!(
            ProjectPath::parse("a//b"),
            Err(PathError::EmptyComponent(_))
        ));
        assert!(matches!(
            ProjectPath::parse("a/"),
            Err(PathError::FileNameMissing(_))
        ));
        assert!(matches!(
            DocumentId::parse("a/readme.md"),
            Err(PathError::NotReadme(_))
        ));
    }

    #[test]
    fn preserves_unicode_and_spaces() {
        let path = ProjectPath::parse("src/módulo/with space.rs").unwrap();
        assert_eq!(path.file_name(), "with space.rs");
        assert_eq!(path.directory().as_str(), "src/módulo");
    }

    #[test]
    fn directory_relations() {
        let dir = DirPath::parse("src/retrieval/naive").unwrap();
        let ancestors: Vec<String> = dir
            .ancestors()
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        assert_eq!(
            ancestors,
            vec!["src/retrieval/naive", "src/retrieval", "src", ""]
        );
        assert!(dir.is_within(&DirPath::parse("src").unwrap()));
        assert!(!dir.is_within(&DirPath::parse("src/retrievalx").unwrap()));
        assert!(dir.is_within(&DirPath::root()));
        let file = ProjectPath::parse("src/retrieval/naive/search.rs").unwrap();
        assert_eq!(
            file.strip_dir(&DirPath::parse("src/retrieval").unwrap()),
            Some("naive/search.rs")
        );
        assert_eq!(file.strip_dir(&DirPath::parse("src/exec").unwrap()), None);
        assert_eq!(dir.readme().as_str(), "src/retrieval/naive/README.md");
        assert_eq!(DirPath::root().readme().as_str(), "README.md");
    }

    #[test]
    fn relative_resolution_rejects_escapes() {
        let base = DirPath::parse("src").unwrap();
        assert_eq!(
            ProjectPath::resolve_relative(&base, "retrieval/README.md")
                .unwrap()
                .as_str(),
            "src/retrieval/README.md"
        );
        assert_eq!(
            ProjectPath::resolve_relative(&base, "../README.md")
                .unwrap()
                .as_str(),
            "README.md"
        );
        assert!(ProjectPath::resolve_relative(&base, "../../README.md").is_err());
        assert!(ProjectPath::resolve_relative(&base, "/README.md").is_err());
    }
}
