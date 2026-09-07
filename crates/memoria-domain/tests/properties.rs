//! Property tests: canonical encodings are order-independent, ownership is
//! unique, and export edits reach only actual consumers.

use std::collections::{BTreeMap, BTreeSet};

use memoria_domain::canonical::{encode_inputs, encode_review_token};
use memoria_domain::{
    ByteRange, DirPath, Document, DocumentId, Export, ExportId, FileInput, Hash64, Import,
    ImportGraph, ImportInput, InputManifest, OwnershipTree, ProjectPath, SourceLocation,
};
use proptest::prelude::*;

fn path_strategy() -> impl Strategy<Value = ProjectPath> {
    prop::collection::vec("[a-z]{1,4}", 1..4)
        .prop_map(|parts| ProjectPath::parse(&format!("{}.rs", parts.join("/"))).unwrap())
}

fn file_strategy() -> impl Strategy<Value = FileInput> {
    (path_strategy(), any::<u64>(), any::<u64>()).prop_map(|(path, bytes, hash)| FileInput {
        path,
        bytes,
        hash: Hash64(hash),
    })
}

fn import_strategy() -> impl Strategy<Value = ImportInput> {
    (
        prop::collection::vec("[a-z]{1,3}", 1..3),
        "[a-z]{1,4}",
        any::<u64>(),
        any::<u64>(),
    )
        .prop_map(|(dirs, id, bytes, hash)| ImportInput {
            document: DocumentId::parse(&format!("{}/README.md", dirs.join("/"))).unwrap(),
            export_id: ExportId::parse(&id).unwrap(),
            bytes,
            hash: Hash64(hash),
        })
}

fn dedup_files(files: Vec<FileInput>) -> Vec<FileInput> {
    let mut seen = BTreeSet::new();
    files
        .into_iter()
        .filter(|f| seen.insert(f.path.clone()))
        .collect()
}

fn dedup_imports(imports: Vec<ImportInput>) -> Vec<ImportInput> {
    let mut seen = BTreeSet::new();
    imports
        .into_iter()
        .filter(|i| seen.insert((i.document.clone(), i.export_id.clone())))
        .collect()
}

fn pseudo_shuffle<T>(items: &mut [T], mut state: u64) {
    for i in (1..items.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

proptest! {
    #[test]
    fn permuting_inputs_does_not_change_canonical_bytes(
        files in prop::collection::vec(file_strategy(), 0..8),
        imports in prop::collection::vec(import_strategy(), 0..6),
        shuffle in any::<u64>(),
    ) {
        let files = dedup_files(files);
        let imports = dedup_imports(imports);
        let doc = DocumentId::parse("README.md").unwrap();
        let manifest = InputManifest::new(doc.clone(), Hash64(7), 3, Hash64(9), files.clone(), imports.clone()).unwrap();
        let mut shuffled_files = files.clone();
        let mut shuffled_imports = imports.clone();
        pseudo_shuffle(&mut shuffled_files, shuffle);
        pseudo_shuffle(&mut shuffled_imports, shuffle ^ 0x9e37_79b9);
        let permuted = InputManifest::new(doc.clone(), Hash64(7), 3, Hash64(9), shuffled_files, shuffled_imports).unwrap();
        prop_assert_eq!(encode_inputs(&manifest), encode_inputs(&permuted));
        let covered = vec![(2, "second reason text".to_string()), (1, "first reason text".to_string())];
        let reversed = vec![(1, "first reason text".to_string()), (2, "second reason text".to_string())];
        prop_assert_eq!(encode_review_token(&doc, 1, &manifest, &covered), encode_review_token(&doc, 1, &permuted, &reversed));
        prop_assert!(manifest.diff(&permuted).is_empty());
    }

    #[test]
    fn every_selected_file_has_exactly_one_owner_when_a_root_readme_exists(
        readme_dirs in prop::collection::btree_set(prop::collection::vec("[a-c]", 0..3).prop_map(|p| p.join("/")), 0..6),
        files in prop::collection::btree_set(path_strategy(), 0..12),
    ) {
        let mut documents: BTreeSet<DocumentId> = readme_dirs.iter().map(|d| DirPath::parse(d).unwrap().readme()).collect();
        documents.insert(DirPath::root().readme());
        let files: Vec<ProjectPath> = files.into_iter().collect();
        let tree = OwnershipTree::build(&documents, &files);
        prop_assert!(tree.unowned().is_empty());
        let mut counted = 0;
        for document in &documents {
            for file in tree.owned_by(document) {
                counted += 1;
                prop_assert_eq!(tree.owner_of(file), Some(document));
                let owner_dir = document.directory();
                prop_assert!(file.is_within(&owner_dir));
                for ancestor in file.directory().ancestors() {
                    if ancestor == owner_dir {
                        break;
                    }
                    prop_assert!(!documents.contains(&ancestor.readme()));
                }
            }
        }
        prop_assert_eq!(counted, files.len());
    }

    #[test]
    fn export_edit_reaches_only_actual_consumers(edges in prop::collection::vec((0usize..6, 0usize..6), 0..10), edited in 0usize..6) {
        let ids: Vec<DocumentId> = (0..6).map(|i| DocumentId::parse(&format!("d{i}/README.md")).unwrap()).collect();
        let mut documents: BTreeMap<DocumentId, Document> = BTreeMap::new();
        for id in &ids {
            documents.insert(id.clone(), Document {
                id: id.clone(),
                exports: vec![Export { id: ExportId::parse("summary").unwrap(), body: ByteRange::new(0, 0), location: SourceLocation { line: 1, column: 1 } }],
                imports: vec![],
                links: vec![],
            });
        }
        let mut seen = BTreeSet::new();
        for (consumer, provider) in edges {
            if consumer == provider || !seen.insert((consumer, provider)) {
                continue;
            }
            documents.get_mut(&ids[consumer]).unwrap().imports.push(Import {
                provider: ids[provider].clone(),
                export_id: ExportId::parse("summary").unwrap(),
                source_text: String::new(),
                body: ByteRange::new(0, 0),
                location: SourceLocation { line: 2, column: 1 },
            });
        }
        let Ok(graph) = ImportGraph::build(&documents) else { return Ok(()) };
        let hash_before = |_: &DocumentId| Hash64(1);
        let hash_after = |provider: &DocumentId| if provider == &ids[edited] { Hash64(2) } else { Hash64(1) };
        let manifest = |hash: &dyn Fn(&DocumentId) -> Hash64, doc: &DocumentId| {
            let imports = documents[doc].imports.iter().map(|i| ImportInput { document: i.provider.clone(), export_id: i.export_id.clone(), bytes: 1, hash: hash(&i.provider) }).collect();
            InputManifest::new(doc.clone(), Hash64(0), 0, Hash64(0), vec![], imports).unwrap()
        };
        let consumers = graph.consumers_of(&ids[edited]);
        for doc in &ids {
            let changed = manifest(&hash_before, doc).diff(&manifest(&hash_after, doc)).inputs_changed();
            prop_assert_eq!(changed, consumers.contains(doc), "{} changed={} consumers={:?}", doc, changed, consumers);
        }
        let position: BTreeMap<&DocumentId, usize> = graph.order().iter().enumerate().map(|(i, d)| (d, i)).collect();
        for (id, document) in &documents {
            for import in &document.imports {
                prop_assert!(position[&import.provider] < position[id]);
            }
        }
    }
}
