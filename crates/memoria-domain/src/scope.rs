//! Document scopes: the backbone rule.
//!
//! Every tracked document covers the selected sources in its own folder and
//! below. A nested document's existence alone removes nothing. A document
//! stops covering a subfolder only when it links to or imports a tracked
//! document strictly inside that subfolder: a handoff. READMEs and opted-in
//! Markdown follow the same rule.
//!
//! Handoff edges always point strictly downward in the folder tree, so they
//! cannot form a cycle, and no document's scope depends on another
//! document's scope. Scopes may overlap; coverage never has a gap below a
//! tracked root document, because a handed-off subtree is always covered by
//! its target or by a deeper target.

use std::collections::{BTreeMap, BTreeSet};

use crate::document::SourceLocation;
use crate::path::{DirPath, DocumentId, ProjectPath};

/// How a document refers to another path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReferenceKind {
    /// A normal Markdown link outside code.
    Link,
    /// A Memoria import block.
    Import,
}

/// One resolved local reference from a tracked document.
///
/// The target is the resolved project path: a folder link already names the
/// folder's `README.md`. Whether the target is tracked is decided by
/// [`ScopeMap::build`], never by the reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DocumentReference {
    pub from: DocumentId,
    pub target: ProjectPath,
    pub kind: ReferenceKind,
    pub location: SourceLocation,
}

/// How a parent hands a subtree to a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandoffVia {
    Link,
    Import,
    Both,
}

impl HandoffVia {
    pub fn as_str(self) -> &'static str {
        match self {
            HandoffVia::Link => "link",
            HandoffVia::Import => "import",
            HandoffVia::Both => "both",
        }
    }

    fn merge(self, other: HandoffVia) -> HandoffVia {
        if self == other {
            self
        } else {
            HandoffVia::Both
        }
    }
}

impl From<ReferenceKind> for HandoffVia {
    fn from(kind: ReferenceKind) -> HandoffVia {
        match kind {
            ReferenceKind::Link => HandoffVia::Link,
            ReferenceKind::Import => HandoffVia::Import,
        }
    }
}

/// One handoff: `parent` stops covering `subtree` because it links to or
/// imports `target`, a tracked document whose folder is `subtree`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Handoff {
    pub parent: DocumentId,
    pub subtree: DirPath,
    pub target: DocumentId,
    pub via: HandoffVia,
    /// One-based line of the first reference that creates the handoff.
    pub line: usize,
}

/// Whether a reference from `from` to `target` is a handoff when `target`
/// is a tracked document: the target's folder lies strictly below `from`'s.
pub fn is_handoff_direction(from: &DocumentId, target: &ProjectPath) -> bool {
    target.directory().is_strictly_within(&from.directory())
}

/// The complete scope inventory of one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeMap {
    documents: BTreeSet<DocumentId>,
    /// Outgoing handoffs per document, sorted by `(subtree, target)`.
    handoffs: BTreeMap<DocumentId, Vec<Handoff>>,
    /// Scope per document, sorted.
    scopes: BTreeMap<DocumentId, Vec<ProjectPath>>,
    /// Covering documents per source, sorted.
    covering: BTreeMap<ProjectPath, Vec<DocumentId>>,
    uncovered: Vec<ProjectPath>,
}

impl ScopeMap {
    /// Apply the handoff rule to every document.
    ///
    /// Only references from tracked documents to tracked documents strictly
    /// below them are handoffs. Every other reference is ignored here; the
    /// caller explains the ones that look like attempted handoffs. Input
    /// order never changes the result.
    pub fn build(
        documents: &BTreeSet<DocumentId>,
        references: &[DocumentReference],
        sources: &[ProjectPath],
    ) -> ScopeMap {
        let mut merged: BTreeMap<(DocumentId, DirPath, DocumentId), (HandoffVia, usize)> =
            BTreeMap::new();
        for reference in references {
            if !documents.contains(&reference.from) {
                continue;
            }
            let Ok(target) = DocumentId::from_path(reference.target.clone()) else {
                continue;
            };
            if !documents.contains(&target)
                || !is_handoff_direction(&reference.from, &reference.target)
            {
                continue;
            }
            let key = (reference.from.clone(), target.directory(), target);
            let via = HandoffVia::from(reference.kind);
            merged
                .entry(key)
                .and_modify(|(existing, line)| {
                    *existing = existing.merge(via);
                    *line = (*line).min(reference.location.line);
                })
                .or_insert((via, reference.location.line));
        }
        let mut handoffs: BTreeMap<DocumentId, Vec<Handoff>> = documents
            .iter()
            .map(|document| (document.clone(), Vec::new()))
            .collect();
        for ((parent, subtree, target), (via, line)) in merged {
            handoffs.entry(parent.clone()).or_default().push(Handoff {
                parent,
                subtree,
                target,
                via,
                line,
            });
        }
        let subtrees: BTreeMap<&DocumentId, BTreeSet<&DirPath>> = handoffs
            .iter()
            .map(|(document, list)| (document, list.iter().map(|h| &h.subtree).collect()))
            .collect();
        let mut by_dir: BTreeMap<DirPath, Vec<&DocumentId>> = BTreeMap::new();
        for document in documents {
            by_dir
                .entry(document.directory())
                .or_default()
                .push(document);
        }

        let mut sorted: Vec<ProjectPath> = sources
            .iter()
            .filter(|path| {
                DocumentId::from_path((*path).clone())
                    .map(|id| !documents.contains(&id))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        sorted.sort();
        sorted.dedup();
        let mut scopes: BTreeMap<DocumentId, Vec<ProjectPath>> = documents
            .iter()
            .map(|document| (document.clone(), Vec::new()))
            .collect();
        let mut covering: BTreeMap<ProjectPath, Vec<DocumentId>> = BTreeMap::new();
        let mut uncovered = Vec::new();
        for source in sorted {
            // Innermost first: `ancestors[0]` is the source's own folder.
            let ancestors = source.directory().ancestors();
            let mut covered_by: Vec<DocumentId> = Vec::new();
            for (depth, dir) in ancestors.iter().enumerate() {
                let Some(candidates) = by_dir.get(dir) else {
                    continue;
                };
                for document in candidates {
                    let handed_off = subtrees.get(document).is_some_and(|set| {
                        ancestors[..depth].iter().any(|inner| set.contains(inner))
                    });
                    if !handed_off {
                        covered_by.push((*document).clone());
                    }
                }
            }
            if covered_by.is_empty() {
                uncovered.push(source);
                continue;
            }
            covered_by.sort();
            for document in &covered_by {
                scopes
                    .get_mut(document)
                    .expect("every document has a scope")
                    .push(source.clone());
            }
            covering.insert(source, covered_by);
        }
        ScopeMap {
            documents: documents.clone(),
            handoffs,
            scopes,
            covering,
            uncovered,
        }
    }

    pub fn documents(&self) -> impl Iterator<Item = &DocumentId> {
        self.documents.iter()
    }

    /// The sorted selected sources a document covers.
    pub fn scope_of(&self, document: &DocumentId) -> &[ProjectPath] {
        self.scopes.get(document).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The sorted documents that cover a source. Empty for an uncovered
    /// source and for any path that is not a selected source.
    pub fn covering(&self, source: &ProjectPath) -> &[DocumentId] {
        self.covering.get(source).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Outgoing handoffs of a document, sorted by subtree then target.
    pub fn handoffs_of(&self, document: &DocumentId) -> &[Handoff] {
        self.handoffs
            .get(document)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Incoming handoffs: every handoff whose target is `document`.
    pub fn handed_off_by(&self, document: &DocumentId) -> Vec<&Handoff> {
        self.handoffs
            .values()
            .flatten()
            .filter(|handoff| &handoff.target == document)
            .collect()
    }

    /// The distinct subtrees a document hands off, sorted.
    pub fn handed_off_subtrees(&self, document: &DocumentId) -> Vec<DirPath> {
        let set: BTreeSet<DirPath> = self
            .handoffs_of(document)
            .iter()
            .map(|handoff| handoff.subtree.clone())
            .collect();
        set.into_iter().collect()
    }

    /// Whether `dir` is one of the document's covered folders: at or below
    /// its own folder and inside no subtree it hands off.
    pub fn covers_dir(&self, document: &DocumentId, dir: &DirPath) -> bool {
        dir.is_within(&document.directory())
            && !self
                .handoffs_of(document)
                .iter()
                .any(|handoff| dir.is_within(&handoff.subtree))
    }

    /// Selected sources no document covers.
    pub fn uncovered(&self) -> &[ProjectPath] {
        &self.uncovered
    }

    /// Selected sources that more than one document covers, sorted.
    pub fn overlapping(&self) -> Vec<&ProjectPath> {
        self.covering
            .iter()
            .filter(|(_, documents)| documents.len() > 1)
            .map(|(path, _)| path)
            .collect()
    }

    /// Total handoffs across every document.
    pub fn handoff_count(&self) -> usize {
        self.handoffs.values().map(Vec::len).sum()
    }

    /// The nearest tracked documents strictly below `document` that it does
    /// not hand off. Each one's folder is also covered by `document`, so
    /// both review the same sources there.
    pub fn absent_handoffs(&self, document: &DocumentId) -> Vec<DocumentId> {
        let dir = document.directory();
        let unhanded: Vec<&DocumentId> = self
            .documents
            .iter()
            .filter(|other| {
                let other_dir = other.directory();
                other_dir.is_strictly_within(&dir) && self.covers_dir(document, &other_dir)
            })
            .collect();
        unhanded
            .iter()
            .filter(|candidate| {
                let candidate_dir = candidate.directory();
                !unhanded
                    .iter()
                    .any(|other| candidate_dir.is_strictly_within(&other.directory()))
            })
            .map(|candidate| (*candidate).clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(paths: &[&str]) -> BTreeSet<DocumentId> {
        paths
            .iter()
            .map(|p| DocumentId::parse(p).unwrap())
            .collect()
    }

    fn files(paths: &[&str]) -> Vec<ProjectPath> {
        paths
            .iter()
            .map(|p| ProjectPath::parse(p).unwrap())
            .collect()
    }

    fn link(from: &str, to: &str) -> DocumentReference {
        reference(from, to, ReferenceKind::Link, 3)
    }

    fn import(from: &str, to: &str) -> DocumentReference {
        reference(from, to, ReferenceKind::Import, 7)
    }

    fn reference(from: &str, to: &str, kind: ReferenceKind, line: usize) -> DocumentReference {
        DocumentReference {
            from: DocumentId::parse(from).unwrap(),
            target: ProjectPath::parse(to).unwrap(),
            kind,
            location: SourceLocation { line, column: 1 },
        }
    }

    fn id(path: &str) -> DocumentId {
        DocumentId::parse(path).unwrap()
    }

    fn scope(map: &ScopeMap, document: &str) -> Vec<String> {
        map.scope_of(&id(document))
            .iter()
            .map(|p| p.as_str().to_string())
            .collect()
    }

    const AUTH: [&str; 2] = ["app.rs", "auth/login.rs"];

    #[test]
    fn without_a_handoff_both_documents_cover_the_subfolder() {
        let map = ScopeMap::build(&docs(&["README.md", "auth/README.md"]), &[], &files(&AUTH));
        assert_eq!(scope(&map, "README.md"), ["app.rs", "auth/login.rs"]);
        assert_eq!(scope(&map, "auth/README.md"), ["auth/login.rs"]);
        assert_eq!(
            map.overlapping(),
            vec![&ProjectPath::parse("auth/login.rs").unwrap()]
        );
        assert_eq!(
            map.absent_handoffs(&id("README.md")),
            vec![id("auth/README.md")]
        );
        assert!(map.uncovered().is_empty());
    }

    #[test]
    fn a_link_or_an_import_hands_the_subfolder_off() {
        for reference in [
            link("README.md", "auth/README.md"),
            import("README.md", "auth/README.md"),
        ] {
            let map = ScopeMap::build(
                &docs(&["README.md", "auth/README.md"]),
                std::slice::from_ref(&reference),
                &files(&AUTH),
            );
            assert_eq!(scope(&map, "README.md"), ["app.rs"]);
            assert_eq!(scope(&map, "auth/README.md"), ["auth/login.rs"]);
            assert!(map.overlapping().is_empty());
            assert!(map.absent_handoffs(&id("README.md")).is_empty());
            let handoffs = map.handoffs_of(&id("README.md"));
            assert_eq!(handoffs.len(), 1);
            assert_eq!(handoffs[0].subtree.as_str(), "auth");
            assert_eq!(handoffs[0].via, HandoffVia::from(reference.kind));
            assert_eq!(map.handed_off_by(&id("auth/README.md")).len(), 1);
        }
    }

    #[test]
    fn link_and_import_to_one_target_merge() {
        let map = ScopeMap::build(
            &docs(&["README.md", "auth/README.md"]),
            &[
                import("README.md", "auth/README.md"),
                link("README.md", "auth/README.md"),
            ],
            &files(&AUTH),
        );
        let handoffs = map.handoffs_of(&id("README.md"));
        assert_eq!(handoffs.len(), 1);
        assert_eq!(handoffs[0].via, HandoffVia::Both);
        assert_eq!(handoffs[0].line, 3);
    }

    #[test]
    fn an_untracked_or_missing_target_is_not_a_handoff() {
        let map = ScopeMap::build(
            &docs(&["README.md"]),
            &[
                link("README.md", "auth/notes.md"),
                link("README.md", "auth/README.md"),
            ],
            &files(&["app.rs", "auth/login.rs", "auth/notes.md"]),
        );
        assert_eq!(
            scope(&map, "README.md"),
            ["app.rs", "auth/login.rs", "auth/notes.md"]
        );
        assert_eq!(map.handoff_count(), 0);
    }

    #[test]
    fn an_opted_in_target_hands_off_its_whole_folder() {
        let map = ScopeMap::build(
            &docs(&["README.md", "auth/guide.md"]),
            &[link("README.md", "auth/guide.md")],
            &files(&AUTH),
        );
        assert_eq!(scope(&map, "README.md"), ["app.rs"]);
        assert_eq!(scope(&map, "auth/guide.md"), ["auth/login.rs"]);
    }

    #[test]
    fn same_folder_upward_and_sideways_references_are_not_handoffs() {
        let documents = docs(&["README.md", "guide.md", "a/README.md", "b/README.md"]);
        let map = ScopeMap::build(
            &documents,
            &[
                link("README.md", "guide.md"),
                link("guide.md", "README.md"),
                link("a/README.md", "README.md"),
                import("a/README.md", "b/README.md"),
                link("README.md", "README.md"),
            ],
            &files(&["x.rs", "a/y.rs", "b/z.rs"]),
        );
        assert_eq!(map.handoff_count(), 0);
        assert_eq!(scope(&map, "README.md"), ["a/y.rs", "b/z.rs", "x.rs"]);
        assert_eq!(scope(&map, "guide.md"), ["a/y.rs", "b/z.rs", "x.rs"]);
        assert_eq!(scope(&map, "a/README.md"), ["a/y.rs"]);
    }

    #[test]
    fn nested_and_overlapping_targets() {
        let documents = docs(&["README.md", "a/README.md", "a/notes.md", "a/b/guide.md"]);
        let sources = files(&["top.rs", "a/x.rs", "a/b/y.rs", "a/b/c/z.rs"]);
        // A deeper target alone removes only the deeper subtree.
        let map = ScopeMap::build(&documents, &[link("README.md", "a/b/guide.md")], &sources);
        assert_eq!(scope(&map, "README.md"), ["a/x.rs", "top.rs"]);
        // Both targets: the union is removed and both handoffs are recorded.
        let map = ScopeMap::build(
            &documents,
            &[
                link("README.md", "a/README.md"),
                link("README.md", "a/b/guide.md"),
                link("README.md", "a/notes.md"),
            ],
            &sources,
        );
        assert_eq!(scope(&map, "README.md"), ["top.rs"]);
        assert_eq!(map.handoffs_of(&id("README.md")).len(), 3);
        assert_eq!(map.handed_off_subtrees(&id("README.md")).len(), 2);
        // A same-folder pair covers the folder together; neither hands the
        // other anything.
        assert_eq!(
            scope(&map, "a/README.md"),
            ["a/b/c/z.rs", "a/b/y.rs", "a/x.rs"]
        );
        assert_eq!(
            scope(&map, "a/notes.md"),
            ["a/b/c/z.rs", "a/b/y.rs", "a/x.rs"]
        );
        assert_eq!(scope(&map, "a/b/guide.md"), ["a/b/c/z.rs", "a/b/y.rs"]);
    }

    #[test]
    fn handoffs_are_per_document() {
        // A root guide without the link still covers auth/.
        let map = ScopeMap::build(
            &docs(&["README.md", "guide.md", "auth/README.md"]),
            &[link("README.md", "auth/README.md")],
            &files(&AUTH),
        );
        assert_eq!(scope(&map, "README.md"), ["app.rs"]);
        assert_eq!(scope(&map, "guide.md"), ["app.rs", "auth/login.rs"]);
        assert_eq!(
            map.absent_handoffs(&id("guide.md")),
            vec![id("auth/README.md")]
        );
    }

    #[test]
    fn missing_root_leaves_sources_uncovered() {
        let map = ScopeMap::build(
            &docs(&["src/README.md"]),
            &[],
            &files(&["app.py", "src/a.py"]),
        );
        assert_eq!(map.uncovered(), files(&["app.py"]).as_slice());
        assert_eq!(
            map.covering(&ProjectPath::parse("src/a.py").unwrap()),
            &[id("src/README.md")]
        );
    }

    #[test]
    fn documents_are_never_sources() {
        let map = ScopeMap::build(
            &docs(&["README.md", "docs/guide.md"]),
            &[],
            &files(&["docs/guide.md", "docs/notes.md"]),
        );
        assert_eq!(scope(&map, "README.md"), ["docs/notes.md"]);
    }

    #[test]
    fn absent_handoffs_name_the_nearest_documents_only() {
        let map = ScopeMap::build(
            &docs(&["README.md", "a/README.md", "a/b/README.md", "c/README.md"]),
            &[link("README.md", "c/README.md")],
            &files(&["a/b/x.rs"]),
        );
        assert_eq!(
            map.absent_handoffs(&id("README.md")),
            vec![id("a/README.md")]
        );
        assert_eq!(
            map.absent_handoffs(&id("a/README.md")),
            vec![id("a/b/README.md")]
        );
        assert!(map.covers_dir(&id("README.md"), &DirPath::parse("a/b").unwrap()));
        assert!(!map.covers_dir(&id("README.md"), &DirPath::parse("c").unwrap()));
    }
}
