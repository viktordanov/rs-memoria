//! Nearest-README ownership.

use std::collections::{BTreeMap, BTreeSet};

use crate::path::{DocumentId, ProjectPath};

/// Ownership of selected source files by their nearest README.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnershipTree {
    owners: BTreeMap<ProjectPath, DocumentId>,
    owned: BTreeMap<DocumentId, Vec<ProjectPath>>,
    unowned: Vec<ProjectPath>,
    parents: BTreeMap<DocumentId, Option<DocumentId>>,
}

impl OwnershipTree {
    /// Assign every selected file to the nearest README above it.
    pub fn build(documents: &BTreeSet<DocumentId>, files: &[ProjectPath]) -> OwnershipTree {
        let mut tree = OwnershipTree::default();
        for document in documents {
            tree.owned.insert(document.clone(), Vec::new());
            let parent = document
                .directory()
                .parent()
                .and_then(|dir| nearest(documents, &dir.ancestors()));
            tree.parents.insert(document.clone(), parent);
        }
        let mut sorted: Vec<ProjectPath> = files.to_vec();
        sorted.sort();
        sorted.dedup();
        for file in sorted {
            match nearest(documents, &file.directory().ancestors()) {
                Some(owner) => {
                    tree.owned
                        .entry(owner.clone())
                        .or_default()
                        .push(file.clone());
                    tree.owners.insert(file, owner);
                }
                None => tree.unowned.push(file),
            }
        }
        tree
    }

    pub fn owner_of(&self, file: &ProjectPath) -> Option<&DocumentId> {
        self.owners.get(file)
    }

    /// Files owned by a document in sorted order.
    pub fn owned_by(&self, document: &DocumentId) -> &[ProjectPath] {
        self.owned.get(document).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn unowned(&self) -> &[ProjectPath] {
        &self.unowned
    }

    /// The nearest README above a document, if any.
    pub fn parent_of(&self, document: &DocumentId) -> Option<&DocumentId> {
        self.parents.get(document).and_then(Option::as_ref)
    }

    pub fn documents(&self) -> impl Iterator<Item = &DocumentId> {
        self.owned.keys()
    }
}

fn nearest(
    documents: &BTreeSet<DocumentId>,
    ancestors: &[crate::path::DirPath],
) -> Option<DocumentId> {
    ancestors
        .iter()
        .map(|dir| dir.readme())
        .find(|candidate| documents.contains(candidate))
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

    #[test]
    fn nearest_readme_owns_files() {
        let documents = docs(&[
            "README.md",
            "src/retrieval/README.md",
            "src/retrieval/naive/README.md",
        ]);
        let tree = OwnershipTree::build(
            &documents,
            &files(&[
                "app.py",
                "src/common.py",
                "src/retrieval/engine.py",
                "src/retrieval/helpers/rank.py",
                "src/retrieval/naive/search.py",
            ]),
        );
        let root = DocumentId::parse("README.md").unwrap();
        let retrieval = DocumentId::parse("src/retrieval/README.md").unwrap();
        let naive = DocumentId::parse("src/retrieval/naive/README.md").unwrap();
        assert_eq!(
            tree.owned_by(&root),
            files(&["app.py", "src/common.py"]).as_slice()
        );
        assert_eq!(
            tree.owned_by(&retrieval),
            files(&["src/retrieval/engine.py", "src/retrieval/helpers/rank.py"]).as_slice()
        );
        assert_eq!(
            tree.owned_by(&naive),
            files(&["src/retrieval/naive/search.py"]).as_slice()
        );
        assert!(tree.unowned().is_empty());
        assert_eq!(tree.parent_of(&naive), Some(&retrieval));
        assert_eq!(tree.parent_of(&retrieval), Some(&root));
        assert_eq!(tree.parent_of(&root), None);
    }

    #[test]
    fn missing_root_leaves_files_unowned() {
        let documents = docs(&["src/README.md"]);
        let tree = OwnershipTree::build(&documents, &files(&["app.py", "src/a.py"]));
        assert_eq!(tree.unowned(), files(&["app.py"]).as_slice());
        assert_eq!(
            tree.owner_of(&ProjectPath::parse("src/a.py").unwrap())
                .unwrap()
                .as_str(),
            "src/README.md"
        );
    }

    #[test]
    fn new_boundary_moves_ownership() {
        let before =
            OwnershipTree::build(&docs(&["README.md"]), &files(&["src/x/a.py", "src/y/b.py"]));
        let after = OwnershipTree::build(
            &docs(&["README.md", "src/x/README.md"]),
            &files(&["src/x/a.py", "src/y/b.py"]),
        );
        let root = DocumentId::parse("README.md").unwrap();
        assert_eq!(before.owned_by(&root).len(), 2);
        assert_eq!(after.owned_by(&root), files(&["src/y/b.py"]).as_slice());
    }
}
