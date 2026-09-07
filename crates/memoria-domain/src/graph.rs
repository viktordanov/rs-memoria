//! Import dependency graph and reader navigation graph.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::document::{Document, ExportId, SourceLocation};
use crate::path::DocumentId;

/// One import edge inside a reported cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleEdge {
    pub importer: DocumentId,
    pub provider: DocumentId,
    pub export_id: ExportId,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    MissingDocument {
        importer: DocumentId,
        location: SourceLocation,
        target: DocumentId,
        source_text: String,
    },
    MissingExport {
        importer: DocumentId,
        location: SourceLocation,
        provider: DocumentId,
        export_id: ExportId,
    },
    SelfImport {
        importer: DocumentId,
        location: SourceLocation,
    },
    DuplicateImport {
        importer: DocumentId,
        location: SourceLocation,
        provider: DocumentId,
        export_id: ExportId,
    },
    Cycle {
        /// Closed path: the first and last entries are the same document.
        path: Vec<DocumentId>,
        edges: Vec<CycleEdge>,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::MissingDocument {
                importer,
                location,
                source_text,
                ..
            } => write!(
                f,
                "{importer}:{location}: import {source_text:?} refers to a README that is not discovered"
            ),
            GraphError::MissingExport {
                importer,
                location,
                provider,
                export_id,
            } => write!(
                f,
                "{importer}:{location}: import refers to missing export {export_id:?} in {provider}"
            ),
            GraphError::SelfImport { importer, location } => {
                write!(f, "{importer}:{location}: a document cannot import itself")
            }
            GraphError::DuplicateImport {
                importer,
                location,
                provider,
                export_id,
            } => write!(
                f,
                "{importer}:{location}: duplicate import of {provider}#{export_id}"
            ),
            GraphError::Cycle { path, .. } => {
                let names: Vec<&str> = path.iter().map(DocumentId::as_str).collect();
                write!(f, "import cycle: {}", names.join(" -> "))
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// Validated import dependencies with a stable dependency order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportGraph {
    providers: BTreeMap<DocumentId, Vec<(DocumentId, ExportId)>>,
    consumers: BTreeMap<DocumentId, BTreeSet<DocumentId>>,
    order: Vec<DocumentId>,
}

impl ImportGraph {
    /// Validate references and build the provider-before-consumer order.
    pub fn build(
        documents: &BTreeMap<DocumentId, Document>,
    ) -> Result<ImportGraph, Vec<GraphError>> {
        let mut errors = Vec::new();
        let mut providers: BTreeMap<DocumentId, Vec<(DocumentId, ExportId)>> = BTreeMap::new();
        let mut consumers: BTreeMap<DocumentId, BTreeSet<DocumentId>> = BTreeMap::new();
        let mut edge_locations: BTreeMap<(DocumentId, DocumentId), (ExportId, SourceLocation)> =
            BTreeMap::new();

        for (id, document) in documents {
            providers.entry(id.clone()).or_default();
            consumers.entry(id.clone()).or_default();
            let mut seen: BTreeSet<(DocumentId, ExportId)> = BTreeSet::new();
            for import in &document.imports {
                if &import.provider == id {
                    errors.push(GraphError::SelfImport {
                        importer: id.clone(),
                        location: import.location,
                    });
                    continue;
                }
                if !seen.insert((import.provider.clone(), import.export_id.clone())) {
                    errors.push(GraphError::DuplicateImport {
                        importer: id.clone(),
                        location: import.location,
                        provider: import.provider.clone(),
                        export_id: import.export_id.clone(),
                    });
                    continue;
                }
                match documents.get(&import.provider) {
                    None => errors.push(GraphError::MissingDocument {
                        importer: id.clone(),
                        location: import.location,
                        target: import.provider.clone(),
                        source_text: import.source_text.clone(),
                    }),
                    Some(provider) => {
                        if provider.export(&import.export_id).is_none() {
                            errors.push(GraphError::MissingExport {
                                importer: id.clone(),
                                location: import.location,
                                provider: import.provider.clone(),
                                export_id: import.export_id.clone(),
                            });
                        }
                        providers
                            .entry(id.clone())
                            .or_default()
                            .push((import.provider.clone(), import.export_id.clone()));
                        consumers
                            .entry(import.provider.clone())
                            .or_default()
                            .insert(id.clone());
                        edge_locations
                            .entry((id.clone(), import.provider.clone()))
                            .or_insert((import.export_id.clone(), import.location));
                    }
                }
            }
        }

        let cycles = find_cycles(&providers, &edge_locations);
        errors.extend(cycles);
        if !errors.is_empty() {
            return Err(errors);
        }

        let order = topological_order(&providers);
        Ok(ImportGraph {
            providers,
            consumers,
            order,
        })
    }

    /// Documents in dependency order: providers before consumers, ties by
    /// document path byte order.
    pub fn order(&self) -> &[DocumentId] {
        &self.order
    }

    /// Direct providers imported by a document, in declaration order.
    pub fn providers_of(&self, document: &DocumentId) -> &[(DocumentId, ExportId)] {
        self.providers
            .get(document)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Distinct direct provider documents in sorted order.
    pub fn provider_documents(&self, document: &DocumentId) -> Vec<DocumentId> {
        let set: BTreeSet<DocumentId> = self
            .providers_of(document)
            .iter()
            .map(|(p, _)| p.clone())
            .collect();
        set.into_iter().collect()
    }

    /// Documents that import from `document`, in sorted order.
    pub fn consumers_of(&self, document: &DocumentId) -> Vec<DocumentId> {
        self.consumers
            .get(document)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Consumers that import a specific export.
    pub fn consumers_of_export(
        &self,
        document: &DocumentId,
        export_id: &ExportId,
    ) -> Vec<DocumentId> {
        self.consumers_of(document)
            .into_iter()
            .filter(|consumer| {
                self.providers_of(consumer)
                    .iter()
                    .any(|(provider, id)| provider == document && id == export_id)
            })
            .collect()
    }
}

fn topological_order(
    providers: &BTreeMap<DocumentId, Vec<(DocumentId, ExportId)>>,
) -> Vec<DocumentId> {
    let mut remaining: BTreeMap<DocumentId, BTreeSet<DocumentId>> = providers
        .iter()
        .map(|(doc, deps)| (doc.clone(), deps.iter().map(|(p, _)| p.clone()).collect()))
        .collect();
    let mut order = Vec::with_capacity(remaining.len());
    let mut emitted: BTreeSet<DocumentId> = BTreeSet::new();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .find(|(_, deps)| deps.iter().all(|dep| emitted.contains(dep)))
            .map(|(doc, _)| doc.clone());
        match next {
            Some(doc) => {
                remaining.remove(&doc);
                emitted.insert(doc.clone());
                order.push(doc);
            }
            None => break, // unreachable after cycle validation
        }
    }
    order
}

fn find_cycles(
    providers: &BTreeMap<DocumentId, Vec<(DocumentId, ExportId)>>,
    edge_locations: &BTreeMap<(DocumentId, DocumentId), (ExportId, SourceLocation)>,
) -> Vec<GraphError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Grey,
        Black,
    }
    let mut color: BTreeMap<&DocumentId, Color> =
        providers.keys().map(|k| (k, Color::White)).collect();
    let mut cycles = Vec::new();
    let mut reported: BTreeSet<Vec<DocumentId>> = BTreeSet::new();

    fn visit<'a>(
        node: &'a DocumentId,
        providers: &'a BTreeMap<DocumentId, Vec<(DocumentId, ExportId)>>,
        color: &mut BTreeMap<&'a DocumentId, Color>,
        stack: &mut Vec<&'a DocumentId>,
        edge_locations: &BTreeMap<(DocumentId, DocumentId), (ExportId, SourceLocation)>,
        cycles: &mut Vec<GraphError>,
        reported: &mut BTreeSet<Vec<DocumentId>>,
    ) {
        color.insert(node, Color::Grey);
        stack.push(node);
        let mut deps: Vec<&DocumentId> = providers
            .get(node)
            .map(|d| d.iter().map(|(p, _)| p).collect())
            .unwrap_or_default();
        deps.sort();
        deps.dedup();
        for dep in deps {
            match color.get(dep).copied().unwrap_or(Color::Black) {
                Color::White => visit(
                    dep,
                    providers,
                    color,
                    stack,
                    edge_locations,
                    cycles,
                    reported,
                ),
                Color::Grey => {
                    let start = stack.iter().position(|d| *d == dep).unwrap_or(0);
                    let mut path: Vec<DocumentId> =
                        stack[start..].iter().map(|d| (*d).clone()).collect();
                    path.push(dep.clone());
                    // Rotate so the smallest document comes first for stable reporting.
                    let closed = &path[..path.len() - 1];
                    let min_index = closed
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.cmp(b.1))
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    let mut rotated: Vec<DocumentId> = closed[min_index..].to_vec();
                    rotated.extend_from_slice(&closed[..min_index]);
                    rotated.push(rotated[0].clone());
                    if reported.insert(rotated.clone()) {
                        let edges = rotated
                            .windows(2)
                            .map(|pair| {
                                let (export_id, location) = edge_locations
                                    .get(&(pair[0].clone(), pair[1].clone()))
                                    .cloned()
                                    .expect("edge location recorded for every import edge");
                                CycleEdge {
                                    importer: pair[0].clone(),
                                    provider: pair[1].clone(),
                                    export_id,
                                    location,
                                }
                            })
                            .collect();
                        cycles.push(GraphError::Cycle {
                            path: rotated,
                            edges,
                        });
                    }
                }
                Color::Black => {}
            }
        }
        stack.pop();
        color.insert(node, Color::Black);
    }

    let nodes: Vec<&DocumentId> = providers.keys().collect();
    for node in nodes {
        if color.get(node) == Some(&Color::White) {
            let mut stack = Vec::new();
            visit(
                node,
                providers,
                &mut color,
                &mut stack,
                edge_locations,
                &mut cycles,
                &mut reported,
            );
        }
    }
    cycles
}

/// Reader navigation: normal local README links plus declared imports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigationGraph {
    edges: BTreeMap<DocumentId, BTreeSet<DocumentId>>,
}

impl NavigationGraph {
    pub fn build(documents: &BTreeMap<DocumentId, Document>) -> NavigationGraph {
        let mut edges: BTreeMap<DocumentId, BTreeSet<DocumentId>> = BTreeMap::new();
        for (id, document) in documents {
            let entry = edges.entry(id.clone()).or_default();
            for link in &document.links {
                if link != id && documents.contains_key(link) {
                    entry.insert(link.clone());
                }
            }
            for import in &document.imports {
                if &import.provider != id && documents.contains_key(&import.provider) {
                    entry.insert(import.provider.clone());
                }
            }
        }
        NavigationGraph { edges }
    }

    pub fn targets_of(&self, document: &DocumentId) -> Vec<DocumentId> {
        self.edges
            .get(document)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Documents reachable from `root`, including the root itself.
    pub fn reachable_from(&self, root: &DocumentId) -> BTreeSet<DocumentId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root.clone()];
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            if let Some(targets) = self.edges.get(&node) {
                for target in targets {
                    if !seen.contains(target) {
                        stack.push(target.clone());
                    }
                }
            }
        }
        seen
    }

    /// Discovered documents that no navigation path from the root reaches.
    pub fn disconnected(&self, root: &DocumentId) -> Vec<DocumentId> {
        let reachable = self.reachable_from(root);
        self.edges
            .keys()
            .filter(|doc| !reachable.contains(doc))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{ByteRange, Export, Import};

    fn doc(id: &str, imports: &[&str], links: &[&str]) -> (DocumentId, Document) {
        let id = DocumentId::parse(id).unwrap();
        let document = Document {
            id: id.clone(),
            exports: vec![Export {
                id: ExportId::parse("summary").unwrap(),
                body: ByteRange::new(0, 0),
                location: SourceLocation { line: 1, column: 1 },
            }],
            imports: imports
                .iter()
                .enumerate()
                .map(|(i, target)| Import {
                    provider: DocumentId::parse(target).unwrap(),
                    export_id: ExportId::parse("summary").unwrap(),
                    source_text: format!("{target}#summary"),
                    body: ByteRange::new(0, 0),
                    location: SourceLocation {
                        line: i + 2,
                        column: 1,
                    },
                })
                .collect(),
            links: links
                .iter()
                .map(|l| DocumentId::parse(l).unwrap())
                .collect(),
        };
        (id, document)
    }

    fn graph(docs: Vec<(DocumentId, Document)>) -> Result<ImportGraph, Vec<GraphError>> {
        ImportGraph::build(&docs.into_iter().collect())
    }

    #[test]
    fn orders_providers_before_consumers_with_lexical_ties() {
        let g = graph(vec![
            doc(
                "README.md",
                &["src/retrieval/README.md", "src/execution/README.md"],
                &["src/corpus/README.md"],
            ),
            doc(
                "src/retrieval/README.md",
                &["src/retrieval/naive/README.md"],
                &[],
            ),
            doc(
                "src/retrieval/naive/README.md",
                &["src/execution/README.md"],
                &[],
            ),
            doc("src/execution/README.md", &[], &[]),
            doc("src/corpus/README.md", &[], &[]),
            doc("src/disconnected/README.md", &[], &[]),
        ])
        .unwrap();
        let order: Vec<&str> = g.order().iter().map(DocumentId::as_str).collect();
        assert_eq!(
            order,
            vec![
                "src/corpus/README.md",
                "src/disconnected/README.md",
                "src/execution/README.md",
                "src/retrieval/naive/README.md",
                "src/retrieval/README.md",
                "README.md",
            ]
        );
        let consumers: Vec<String> = g
            .consumers_of(&DocumentId::parse("src/execution/README.md").unwrap())
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        assert_eq!(
            consumers,
            vec!["README.md", "src/retrieval/naive/README.md"]
        );
    }

    #[test]
    fn reports_cycles_with_locations() {
        let err = graph(vec![
            doc("README.md", &["a/README.md"], &[]),
            doc("a/README.md", &["b/README.md"], &[]),
            doc("b/README.md", &["a/README.md"], &[]),
        ])
        .unwrap_err();
        assert_eq!(err.len(), 1);
        match &err[0] {
            GraphError::Cycle { path, edges } => {
                let names: Vec<&str> = path.iter().map(DocumentId::as_str).collect();
                assert_eq!(names, vec!["a/README.md", "b/README.md", "a/README.md"]);
                assert_eq!(edges.len(), 2);
                assert_eq!(edges[0].location.line, 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn reports_missing_targets_self_imports_and_duplicates() {
        let err = graph(vec![
            doc(
                "README.md",
                &[
                    "missing/README.md",
                    "README.md",
                    "a/README.md",
                    "a/README.md",
                ],
                &[],
            ),
            doc("a/README.md", &[], &[]),
        ])
        .unwrap_err();
        assert!(matches!(err[0], GraphError::MissingDocument { .. }));
        assert!(matches!(err[1], GraphError::SelfImport { .. }));
        assert!(matches!(err[2], GraphError::DuplicateImport { .. }));
        assert_eq!(err.len(), 3);
    }

    #[test]
    fn missing_export_is_reported() {
        let (id, mut a) = doc("a/README.md", &[], &[]);
        a.exports.clear();
        let err = graph(vec![doc("README.md", &["a/README.md"], &[]), (id, a)]).unwrap_err();
        assert!(matches!(err[0], GraphError::MissingExport { .. }));
    }

    #[test]
    fn navigation_reachability_uses_links_and_imports() {
        let docs: BTreeMap<DocumentId, Document> = vec![
            doc("README.md", &["a/README.md"], &["b/README.md"]),
            doc("a/README.md", &[], &[]),
            doc("b/README.md", &[], &[]),
            doc("c/README.md", &[], &["README.md"]),
        ]
        .into_iter()
        .collect();
        let nav = NavigationGraph::build(&docs);
        let root = DocumentId::parse("README.md").unwrap();
        let disconnected: Vec<String> = nav
            .disconnected(&root)
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        assert_eq!(disconnected, vec!["c/README.md"]);
    }
}
