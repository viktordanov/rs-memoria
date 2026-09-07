//! Immutable review inputs and their comparison.

use std::fmt;

use crate::document::ExportId;
use crate::path::{DocumentId, ProjectPath};

/// A 64-bit content hash, displayed as 16 lowercase hexadecimal digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash64(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHash(pub String);

impl fmt::Display for InvalidHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hash {:?} must be 16 lowercase hexadecimal digits",
            self.0
        )
    }
}

impl std::error::Error for InvalidHash {}

impl Hash64 {
    pub fn parse(text: &str) -> Result<Hash64, InvalidHash> {
        if text.len() != 16 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(InvalidHash(text.to_string()));
        }
        u64::from_str_radix(text, 16)
            .map(Hash64)
            .map_err(|_| InvalidHash(text.to_string()))
    }

    pub fn to_hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

impl fmt::Debug for Hash64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash64({:016x})", self.0)
    }
}

impl fmt::Display for Hash64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// One owned source file in a manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileInput {
    pub path: ProjectPath,
    pub bytes: u64,
    pub hash: Hash64,
}

/// One imported export in a manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportInput {
    pub document: DocumentId,
    pub export_id: ExportId,
    pub bytes: u64,
    pub hash: Hash64,
}

/// The complete reviewed inputs of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputManifest {
    pub document: DocumentId,
    pub policy_hash: Hash64,
    pub document_bytes: u64,
    pub document_hash: Hash64,
    files: Vec<FileInput>,
    imports: Vec<ImportInput>,
}

impl InputManifest {
    /// Build a manifest. Files sort by path; imports sort by provider path
    /// then export id. Duplicate identities are rejected.
    pub fn new(
        document: DocumentId,
        policy_hash: Hash64,
        document_bytes: u64,
        document_hash: Hash64,
        mut files: Vec<FileInput>,
        mut imports: Vec<ImportInput>,
    ) -> Result<InputManifest, ManifestError> {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        if let Some(pair) = files.windows(2).find(|w| w[0].path == w[1].path) {
            return Err(ManifestError::DuplicateFile(pair[0].path.clone()));
        }
        imports.sort_by(|a, b| (&a.document, &a.export_id).cmp(&(&b.document, &b.export_id)));
        if let Some(pair) = imports
            .windows(2)
            .find(|w| w[0].document == w[1].document && w[0].export_id == w[1].export_id)
        {
            return Err(ManifestError::DuplicateImport(
                pair[0].document.clone(),
                pair[0].export_id.clone(),
            ));
        }
        Ok(InputManifest {
            document,
            policy_hash,
            document_bytes,
            document_hash,
            files,
            imports,
        })
    }

    pub fn files(&self) -> &[FileInput] {
        &self.files
    }

    pub fn imports(&self) -> &[ImportInput] {
        &self.imports
    }

    /// Bytes of README, owned files, and imported export bodies.
    pub fn raw_input_bytes(&self) -> u64 {
        self.document_bytes
            + self.files.iter().map(|f| f.bytes).sum::<u64>()
            + self.imports.iter().map(|i| i.bytes).sum::<u64>()
    }

    /// Array elements this manifest contributes to a packet: one per file
    /// and one per import.
    pub fn record_count(&self) -> u64 {
        self.files.len() as u64 + self.imports.len() as u64
    }

    /// Compare a previous manifest (`self`) with the current one.
    pub fn diff(&self, current: &InputManifest) -> ManifestDiff {
        let mut diff = ManifestDiff::default();
        if self.policy_hash != current.policy_hash {
            diff.policy = Some((self.policy_hash, current.policy_hash));
        }
        if self.document_bytes != current.document_bytes
            || self.document_hash != current.document_hash
        {
            diff.document = Some((
                (self.document_bytes, self.document_hash),
                (current.document_bytes, current.document_hash),
            ));
        }
        diff.files = diff_sorted(&self.files, &current.files, |a, b| a.path.cmp(&b.path));
        diff.imports = diff_sorted(&self.imports, &current.imports, |a, b| {
            (&a.document, &a.export_id).cmp(&(&b.document, &b.export_id))
        });
        diff
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    DuplicateFile(ProjectPath),
    DuplicateImport(DocumentId, ExportId),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::DuplicateFile(path) => write!(f, "duplicate file input {path}"),
            ManifestError::DuplicateImport(doc, id) => {
                write!(f, "duplicate import input {doc}#{id}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// A difference for one input identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputChange<T> {
    Added(T),
    Removed(T),
    Changed { before: T, after: T },
}

fn diff_sorted<T: Clone + PartialEq>(
    before: &[T],
    after: &[T],
    key: impl Fn(&T, &T) -> std::cmp::Ordering,
) -> Vec<InputChange<T>> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        match (before.get(i), after.get(j)) {
            (Some(b), Some(a)) => match key(b, a) {
                std::cmp::Ordering::Less => {
                    out.push(InputChange::Removed(b.clone()));
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    out.push(InputChange::Added(a.clone()));
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    if b != a {
                        out.push(InputChange::Changed {
                            before: b.clone(),
                            after: a.clone(),
                        });
                    }
                    i += 1;
                    j += 1;
                }
            },
            (Some(b), None) => {
                out.push(InputChange::Removed(b.clone()));
                i += 1;
            }
            (None, Some(a)) => {
                out.push(InputChange::Added(a.clone()));
                j += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// Every difference between two manifests, in canonical order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestDiff {
    pub policy: Option<(Hash64, Hash64)>,
    pub document: Option<((u64, Hash64), (u64, Hash64))>,
    /// The document itself no longer exists: its previous size and hash.
    pub document_removed: Option<(u64, Hash64)>,
    pub files: Vec<InputChange<FileInput>>,
    pub imports: Vec<InputChange<ImportInput>>,
}

impl ManifestDiff {
    /// The difference between a previous manifest and a project in which
    /// its document has disappeared: the document and every input it had
    /// are removed from this document's baseline. No current policy exists
    /// for a scope that is gone, so no policy change is claimed.
    pub fn removed(previous: &InputManifest) -> ManifestDiff {
        ManifestDiff {
            policy: None,
            document: None,
            document_removed: Some((previous.document_bytes, previous.document_hash)),
            files: previous
                .files
                .iter()
                .cloned()
                .map(InputChange::Removed)
                .collect(),
            imports: previous
                .imports
                .iter()
                .cloned()
                .map(InputChange::Removed)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.policy.is_none()
            && self.document.is_none()
            && self.document_removed.is_none()
            && self.files.is_empty()
            && self.imports.is_empty()
    }

    /// Whether anything other than the document's own bytes changed.
    pub fn inputs_changed(&self) -> bool {
        self.policy.is_some() || !self.files.is_empty() || !self.imports.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, bytes: u64, hash: u64) -> FileInput {
        FileInput {
            path: ProjectPath::parse(path).unwrap(),
            bytes,
            hash: Hash64(hash),
        }
    }

    #[test]
    fn removed_diff_lists_the_document_and_every_previous_input() {
        let previous = manifest(vec![file("a.rs", 3, 7), file("b.rs", 4, 8)], 5);
        let diff = ManifestDiff::removed(&previous);
        assert!(!diff.is_empty());
        assert!(diff.inputs_changed());
        assert_eq!(diff.policy, None);
        assert_eq!(diff.document, None);
        assert_eq!(diff.document_removed, Some((10, Hash64(5))));
        assert_eq!(
            diff.files,
            vec![
                InputChange::Removed(file("a.rs", 3, 7)),
                InputChange::Removed(file("b.rs", 4, 8))
            ]
        );
        assert!(diff.imports.is_empty());
        // A document with no inputs still yields a non-empty conflict.
        assert!(!ManifestDiff::removed(&manifest(vec![], 5)).is_empty());
        // Ordinary comparison never claims removal.
        assert_eq!(previous.diff(&previous), ManifestDiff::default());
    }

    fn manifest(files: Vec<FileInput>, doc_hash: u64) -> InputManifest {
        InputManifest::new(
            DocumentId::parse("README.md").unwrap(),
            Hash64(1),
            10,
            Hash64(doc_hash),
            files,
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn hash_round_trip() {
        let hash = Hash64::parse("ef46db3751d8e999").unwrap();
        assert_eq!(hash.0, 0xef46db3751d8e999);
        assert_eq!(hash.to_hex(), "ef46db3751d8e999");
        assert!(Hash64::parse("EF46DB3751D8E999").is_err());
        assert!(Hash64::parse("ef46db3751d8e99").is_err());
    }

    #[test]
    fn sorts_and_rejects_duplicates() {
        let m = manifest(vec![file("b.rs", 1, 1), file("a.rs", 1, 1)], 5);
        assert_eq!(m.files()[0].path.as_str(), "a.rs");
        assert!(
            InputManifest::new(
                DocumentId::parse("README.md").unwrap(),
                Hash64(1),
                0,
                Hash64(0),
                vec![file("a.rs", 1, 1), file("a.rs", 2, 2)],
                vec![]
            )
            .is_err()
        );
    }

    #[test]
    fn diff_reports_added_removed_changed_in_order() {
        let before = manifest(
            vec![file("a.rs", 1, 1), file("b.rs", 2, 2), file("d.rs", 4, 4)],
            5,
        );
        let after = manifest(
            vec![file("a.rs", 1, 1), file("b.rs", 3, 3), file("c.rs", 9, 9)],
            5,
        );
        let diff = before.diff(&after);
        assert!(diff.document.is_none());
        assert_eq!(diff.files.len(), 3);
        assert!(
            matches!(&diff.files[0], InputChange::Changed { before, .. } if before.path.as_str() == "b.rs")
        );
        assert!(matches!(&diff.files[1], InputChange::Added(f) if f.path.as_str() == "c.rs"));
        assert!(matches!(&diff.files[2], InputChange::Removed(f) if f.path.as_str() == "d.rs"));
        assert!(diff.inputs_changed());
    }

    #[test]
    fn document_only_change_is_not_input_change() {
        let before = manifest(vec![file("a.rs", 1, 1)], 5);
        let after = manifest(vec![file("a.rs", 1, 1)], 6);
        let diff = before.diff(&after);
        assert!(diff.document.is_some());
        assert!(!diff.inputs_changed());
        assert!(!diff.is_empty());
    }
}
