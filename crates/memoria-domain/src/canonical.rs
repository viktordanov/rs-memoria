//! Canonical byte encoding `C` for fingerprints and review tokens.
//!
//! Primitives: an unsigned integer is eight big-endian bytes; a byte string
//! is its length followed by its bytes; a list is its count followed by its
//! elements; an option is one byte (`0` or `1`) followed by the value when
//! present. Every hashed structure begins with a length-prefixed
//! domain-separation string.

use crate::guidance::{GuidanceDigest, GuidanceEntry};
use crate::manifest::{Hash64, InputManifest};
use crate::path::DocumentId;
use crate::policy::EffectivePolicy;
use crate::review::ReviewRecord;
use crate::section::SectionMapIdentity;

pub const POLICY_DOMAIN: &str = "memoria.policy.v2";
pub const INPUTS_DOMAIN: &str = "memoria.inputs.v2";
pub const REVIEW_TOKEN_DOMAIN: &str = "memoria.review-token.v2";
pub const PACKET_DOMAIN: &str = "memoria.packet.v2";
pub const GUIDANCE_DOMAIN: &str = "memoria.guidance.v1";

/// Integrity domain of a full v3 export envelope.
pub const PACKET_V3_DOMAIN: &str = "memoria.packet.v3";
/// Integrity domain of a small review manifest artifact.
pub const REVIEW_MANIFEST_DOMAIN: &str = "memoria.review-manifest.v1";
/// `B`: the complete prior review record a focused review would reuse.
pub const REVIEW_BASELINE_DOMAIN: &str = "memoria-review-baseline-v1";
/// `C`: the ownership, selection, mapping, guidance, and graph context.
pub const REVIEW_CONTEXT_DOMAIN: &str = "memoria-review-context-v1";
/// `T`: the v3 review token.
pub const REVIEW_TOKEN_V3_DOMAIN: &str = "memoria-review-token-v3";

pub const SELECTION_ALGORITHM: &str = "git-worktree-v2";
/// Repository-only, case-sensitive ignore inventory. Host ignore settings
/// decide actual Git eligibility; they never enter this policy.
pub const REPOSITORY_IGNORE_ALGORITHM: &str = "repository-ignore-v1";
pub const OWNERSHIP_ALGORITHM: &str = "nearest-readme-v1";
pub const FINGERPRINT_ALGORITHM: &str = "raw-v1";
pub const HASH_ALGORITHM: &str = "xxh3-64-seed0";
pub const NEWLINE_POLICY: &str = "exact-newlines";

/// Incremental canonical encoder.
#[derive(Debug, Default, Clone)]
pub struct Encoder {
    buffer: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder::default()
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.buffer.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.u64(value.len() as u64);
        self.buffer.extend_from_slice(value);
        self
    }

    pub fn str(&mut self, value: &str) -> &mut Self {
        self.bytes(value.as_bytes())
    }

    pub fn list_len(&mut self, count: usize) -> &mut Self {
        self.u64(count as u64)
    }

    pub fn option<T>(&mut self, value: Option<T>, encode: impl FnOnce(&mut Self, T)) -> &mut Self {
        match value {
            None => self.buffer.push(0),
            Some(inner) => {
                self.buffer.push(1);
                encode(self, inner);
            }
        }
        self
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buffer.extend_from_slice(bytes);
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.buffer
    }
}

/// `memoria.policy.v1` canonical bytes.
pub fn encode_policy(policy: &EffectivePolicy) -> Vec<u8> {
    let mut e = Encoder::new();
    e.str(POLICY_DOMAIN)
        .str(policy.owner.as_str())
        .str(SELECTION_ALGORITHM)
        .str(REPOSITORY_IGNORE_ALGORITHM)
        .str(OWNERSHIP_ALGORITHM)
        .str(FINGERPRINT_ALGORITHM)
        .str(HASH_ALGORITHM)
        .str(NEWLINE_POLICY);
    e.list_len(policy.git_scopes.len());
    for scope in &policy.git_scopes {
        e.str(&scope.identity);
        e.list_len(scope.patterns.len());
        for pattern in &scope.patterns {
            // Same framing as `str`: valid UTF-8 rules hash exactly as before.
            e.bytes(pattern);
        }
    }
    e.list_len(policy.memoria_scopes.len());
    for scope in &policy.memoria_scopes {
        e.str(scope.scope.as_str());
        e.list_len(scope.ignore.len());
        for pattern in &scope.ignore {
            e.str(pattern);
        }
        e.list_len(scope.include.len());
        for pattern in &scope.include {
            e.str(pattern);
        }
    }
    e.finish()
}

/// `memoria.inputs.v1` canonical bytes.
pub fn encode_inputs(manifest: &InputManifest) -> Vec<u8> {
    let mut e = Encoder::new();
    encode_inputs_into(&mut e, manifest);
    e.finish()
}

fn encode_inputs_into(e: &mut Encoder, manifest: &InputManifest) {
    e.str(INPUTS_DOMAIN)
        .str(manifest.document.as_str())
        .u64(manifest.policy_hash.0)
        .u64(manifest.document_bytes)
        .u64(manifest.document_hash.0);
    e.list_len(manifest.files().len());
    for file in manifest.files() {
        e.str(file.path.as_str()).u64(file.bytes).u64(file.hash.0);
    }
    e.list_len(manifest.imports().len());
    for import in manifest.imports() {
        e.str(import.document.as_str())
            .str(import.export_id.as_str())
            .u64(import.bytes)
            .u64(import.hash.0);
    }
}

/// `memoria.guidance.v1` canonical bytes: the domain, the entry count, and
/// each entry's four length-prefixed UTF-8 fields in authored order.
pub fn encode_guidance(entries: &[GuidanceEntry]) -> Vec<u8> {
    let mut e = Encoder::new();
    e.str(GUIDANCE_DOMAIN);
    e.list_len(entries.len());
    for entry in entries {
        e.str(entry.scope.as_str())
            .str(&entry.source)
            .str(entry.kind.as_str())
            .str(&entry.text);
    }
    e.finish()
}

/// `memoria.review-token.v2` canonical bytes. The guidance digest follows the
/// manifest, before the sorted covered invalidations. Invalidations must be
/// sorted by id; the encoder sorts defensively.
pub fn encode_review_token(
    document: &DocumentId,
    review_revision: u64,
    manifest: &InputManifest,
    guidance: GuidanceDigest,
    invalidations: &[(u64, String)],
) -> Vec<u8> {
    let mut sorted: Vec<&(u64, String)> = invalidations.iter().collect();
    sorted.sort_by_key(|entry| entry.0);
    let mut e = Encoder::new();
    e.str(REVIEW_TOKEN_DOMAIN)
        .str(document.as_str())
        .u64(review_revision);
    encode_inputs_into(&mut e, manifest);
    e.u64(guidance.0.0);
    e.list_len(sorted.len());
    for (id, reason) in sorted {
        e.u64(*id).str(reason);
    }
    e.finish()
}

/// One resolved import edge: the provider, its export, and the export's
/// current body hash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportEdge {
    pub provider: String,
    pub export_id: String,
    pub hash: Hash64,
}

/// One direct consumer of this document's export.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConsumerEdge {
    pub export_id: String,
    pub consumer: String,
}

/// One provider in the target's transitive provider closure.
///
/// The closure is deliberately conservative. A provider edit can invalidate a
/// consumer token even when the imported export body stays equal. It does not
/// make that consumer stale by itself; freshness and scheduling are unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProviderDescriptor {
    pub document: String,
    pub inputs_digest: Hash64,
    pub guidance_digest: Hash64,
    pub review_revision: u64,
    /// `(id, exact reason)`, sorted by id.
    pub active_invalidations: Vec<(u64, String)>,
    /// The provider's own resolved import edges, sorted.
    pub imports: Vec<ImportEdge>,
}

/// The complete review context `C` binds beyond the input manifest.
///
/// Every collection is sorted by its owner before encoding, so authored order
/// and traversal order never change the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewContext {
    pub selection_version: u64,
    /// Ownership: the owner and the boundaries that delimit its coverage.
    pub owner: String,
    pub ancestor_boundaries: Vec<String>,
    pub descendant_boundaries: Vec<String>,
    /// Nested-repository boundaries inside the owner's directory.
    pub nested_repositories: Vec<String>,
    /// Selection: the effective policy hash and the actual selected owned set.
    pub policy_hash: Hash64,
    pub owned_paths: Vec<String>,
    /// Mapping: validity state and the sorted ID-to-source associations.
    pub mapping: SectionMapIdentity,
    /// Guidance: the complete effective ordered guidance digest.
    pub guidance: GuidanceDigest,
    /// Imports and graph.
    pub imports: Vec<ImportEdge>,
    pub consumer_edges: Vec<ConsumerEdge>,
    pub providers: Vec<ProviderDescriptor>,
}

fn encode_import_edges(e: &mut Encoder, edges: &[ImportEdge]) {
    let mut sorted: Vec<&ImportEdge> = edges.iter().collect();
    sorted.sort();
    e.list_len(sorted.len());
    for edge in sorted {
        e.str(&edge.provider).str(&edge.export_id).u64(edge.hash.0);
    }
}

fn encode_sorted_strings(e: &mut Encoder, values: &[String]) {
    let mut sorted: Vec<&String> = values.iter().collect();
    sorted.sort();
    e.list_len(sorted.len());
    for value in sorted {
        e.str(value);
    }
}

/// `memoria-review-baseline-v1` canonical bytes for `B`.
///
/// Every stored field of the prior record enters the encoding, reusing the
/// existing manifest encoding. Nothing here depends on JSON.
pub fn encode_review_baseline(record: Option<&ReviewRecord>) -> Vec<u8> {
    let mut e = Encoder::new();
    e.str(REVIEW_BASELINE_DOMAIN);
    e.option(record, |e, record| {
        e.u64(record.revision);
        encode_inputs_into(e, &record.manifest);
        e.u64(record.input_fingerprint.0)
            .u64(record.token_digest.0)
            .u64(record.guidance.0.0)
            .str(&record.reviewed_at.0)
            .str(record.reviewer.as_str())
            .str(record.result.as_str())
            .str(record.note.as_str());
        e.option(record.git.base_commit.as_deref(), |e, commit| {
            e.str(commit);
        });
        e.u64(u64::from(record.git.worktree_dirty));
        let mut acknowledged = record.acknowledged_invalidations.clone();
        acknowledged.sort_unstable();
        e.list_len(acknowledged.len());
        for id in acknowledged {
            e.u64(id);
        }
    });
    e.finish()
}

/// `memoria-review-context-v1` canonical bytes for `C`.
pub fn encode_review_context(context: &ReviewContext) -> Vec<u8> {
    let mut e = Encoder::new();
    e.str(REVIEW_CONTEXT_DOMAIN).u64(context.selection_version);
    // Ownership.
    e.str(&context.owner);
    encode_sorted_strings(&mut e, &context.ancestor_boundaries);
    encode_sorted_strings(&mut e, &context.descendant_boundaries);
    encode_sorted_strings(&mut e, &context.nested_repositories);
    // Selection.
    e.u64(context.policy_hash.0);
    encode_sorted_strings(&mut e, &context.owned_paths);
    // Mapping. The tag separates absent, valid, and invalid mapping states.
    e.u64(context.mapping.tag());
    let pairs = context.mapping.pairs();
    e.list_len(pairs.len());
    for (id, sources) in pairs {
        e.str(id);
        e.list_len(sources.len());
        for source in sources {
            e.str(source);
        }
    }
    // Guidance.
    e.u64(context.guidance.0.0);
    // Imports and graph.
    encode_import_edges(&mut e, &context.imports);
    let mut consumers: Vec<&ConsumerEdge> = context.consumer_edges.iter().collect();
    consumers.sort();
    e.list_len(consumers.len());
    for edge in consumers {
        e.str(&edge.export_id).str(&edge.consumer);
    }
    let mut providers: Vec<&ProviderDescriptor> = context.providers.iter().collect();
    providers.sort();
    e.list_len(providers.len());
    for provider in providers {
        e.str(&provider.document)
            .u64(provider.inputs_digest.0)
            .u64(provider.guidance_digest.0)
            .u64(provider.review_revision);
        let mut invalidations: Vec<&(u64, String)> = provider.active_invalidations.iter().collect();
        invalidations.sort_by_key(|entry| entry.0);
        e.list_len(invalidations.len());
        for (id, reason) in invalidations {
            e.u64(*id).str(reason);
        }
        encode_import_edges(&mut e, &provider.imports);
    }
    e.finish()
}

/// `memoria-review-token-v3` canonical bytes.
///
/// `I`, `B`, and `C` are the caller's fingerprints of the input manifest, the
/// baseline record, and the review context. Invalidations must be sorted by
/// id; the encoder sorts defensively.
pub fn encode_review_token_v3(
    document: &DocumentId,
    review_revision: u64,
    inputs_digest: Hash64,
    baseline_digest: Hash64,
    context_digest: Hash64,
    invalidations: &[(u64, String)],
) -> Vec<u8> {
    let mut sorted: Vec<&(u64, String)> = invalidations.iter().collect();
    sorted.sort_by_key(|entry| entry.0);
    let mut e = Encoder::new();
    e.str(REVIEW_TOKEN_V3_DOMAIN)
        .str(document.as_str())
        .u64(review_revision)
        .u64(inputs_digest.0)
        .u64(baseline_digest.0)
        .u64(context_digest.0);
    e.list_len(sorted.len());
    for (id, reason) in sorted {
        e.u64(*id).str(reason);
    }
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileInput, Hash64, ImportInput};
    use crate::path::ProjectPath;

    fn document() -> DocumentId {
        DocumentId::parse("container/README.md").unwrap()
    }

    fn context() -> ReviewContext {
        ReviewContext {
            selection_version: 1,
            owner: "container/README.md".into(),
            ancestor_boundaries: vec!["README.md".into()],
            descendant_boundaries: vec!["container/inner/README.md".into()],
            nested_repositories: vec![],
            policy_hash: Hash64(0x44),
            owned_paths: vec!["container/service.go".into(), "container/handle.go".into()],
            mapping: SectionMapIdentity::Valid(vec![(
                "persistence".into(),
                vec!["container/service.go".into()],
            )]),
            guidance: GuidanceDigest(Hash64(0x55)),
            imports: vec![ImportEdge {
                provider: "other/README.md".into(),
                export_id: "summary".into(),
                hash: Hash64(0x66),
            }],
            consumer_edges: vec![ConsumerEdge {
                export_id: "mine".into(),
                consumer: "app/README.md".into(),
            }],
            providers: vec![ProviderDescriptor {
                document: "other/README.md".into(),
                inputs_digest: Hash64(0x77),
                guidance_digest: Hash64(0x88),
                review_revision: 3,
                active_invalidations: vec![
                    (2, "second reason here".into()),
                    (1, "first reason here".into()),
                ],
                imports: vec![],
            }],
        }
    }

    #[test]
    fn context_encoding_is_order_independent_but_field_sensitive() {
        let base = encode_review_context(&context());
        // Author and traversal order never change the token.
        let mut reordered = context();
        reordered.owned_paths.reverse();
        reordered.providers[0].active_invalidations.reverse();
        assert_eq!(base, encode_review_context(&reordered));
        // Every bound component changes it.
        let mut changed = context();
        changed.selection_version = 2;
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.policy_hash = Hash64(0x45);
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.owned_paths.push("container/extra.go".into());
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.descendant_boundaries.clear();
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.guidance = GuidanceDigest(Hash64(0x56));
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.imports[0].hash = Hash64(0x67);
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.consumer_edges.clear();
        assert_ne!(base, encode_review_context(&changed));
        // A provider edit with an equal export body still changes the context.
        let mut changed = context();
        changed.providers[0].inputs_digest = Hash64(0x78);
        assert_ne!(base, encode_review_context(&changed));
        let mut changed = context();
        changed.providers[0].review_revision = 4;
        assert_ne!(base, encode_review_context(&changed));
    }

    #[test]
    fn mapping_states_are_distinct_in_the_context() {
        let mut absent = context();
        absent.mapping = SectionMapIdentity::Absent;
        let mut invalid = context();
        invalid.mapping = SectionMapIdentity::Invalid;
        let mut empty = context();
        empty.mapping = SectionMapIdentity::Valid(vec![]);
        let encodings = [
            encode_review_context(&absent),
            encode_review_context(&invalid),
            encode_review_context(&empty),
            encode_review_context(&context()),
        ];
        for (i, a) in encodings.iter().enumerate() {
            for b in encodings.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        // A changed association is a different context.
        let mut moved = context();
        moved.mapping = SectionMapIdentity::Valid(vec![(
            "persistence".into(),
            vec!["container/handle.go".into()],
        )]);
        assert_ne!(
            encode_review_context(&context()),
            encode_review_context(&moved)
        );
    }

    #[test]
    fn baseline_encoding_distinguishes_absence_from_every_stored_field() {
        use crate::manifest::Hash64 as H;
        use crate::review::{GitContext, ReviewNote, ReviewResult, ReviewerName, Timestamp};
        let manifest = InputManifest::new(document(), H(1), 2, H(3), vec![], vec![]).unwrap();
        let record = ReviewRecord {
            revision: 7,
            manifest,
            input_fingerprint: H(4),
            token_digest: H(5),
            guidance: GuidanceDigest(H(6)),
            reviewed_at: Timestamp("2026-09-16T00:00:00Z".into()),
            reviewer: ReviewerName::from_stored("prior-reviewer".into()),
            result: ReviewResult::NoUpdate,
            note: ReviewNote::from_stored("The description still matches.".into()),
            git: GitContext {
                base_commit: Some("abc123".into()),
                worktree_dirty: false,
            },
            acknowledged_invalidations: vec![2, 1],
        };
        let base = encode_review_baseline(Some(&record));
        assert_ne!(base, encode_review_baseline(None));
        // Stored order of acknowledged ids is not part of the identity.
        let mut reordered = record.clone();
        reordered.acknowledged_invalidations = vec![1, 2];
        assert_eq!(base, encode_review_baseline(Some(&reordered)));
        for mutate in [
            (|r: &mut ReviewRecord| r.revision = 8) as fn(&mut ReviewRecord),
            |r| r.input_fingerprint = H(40),
            |r| r.token_digest = H(50),
            |r| r.guidance = GuidanceDigest(H(60)),
            |r| r.reviewed_at = Timestamp("2026-09-17T00:00:00Z".into()),
            |r| r.reviewer = ReviewerName::from_stored("someone-else".into()),
            |r| r.result = ReviewResult::Updated,
            |r| r.note = ReviewNote::from_stored("A different conclusion.".into()),
            |r| r.git.base_commit = None,
            |r| r.git.worktree_dirty = true,
            |r| r.acknowledged_invalidations = vec![1],
        ] {
            let mut changed = record.clone();
            mutate(&mut changed);
            assert_ne!(base, encode_review_baseline(Some(&changed)));
        }
    }

    #[test]
    fn v3_token_binds_every_component() {
        let doc = document();
        let covered = [(1u64, "one reason here".to_string())];
        let base = encode_review_token_v3(&doc, 7, Hash64(1), Hash64(2), Hash64(3), &covered);
        assert_eq!(
            base,
            encode_review_token_v3(&doc, 7, Hash64(1), Hash64(2), Hash64(3), &covered)
        );
        for changed in [
            encode_review_token_v3(&doc, 8, Hash64(1), Hash64(2), Hash64(3), &covered),
            encode_review_token_v3(&doc, 7, Hash64(9), Hash64(2), Hash64(3), &covered),
            encode_review_token_v3(&doc, 7, Hash64(1), Hash64(9), Hash64(3), &covered),
            encode_review_token_v3(&doc, 7, Hash64(1), Hash64(2), Hash64(9), &covered),
            encode_review_token_v3(&doc, 7, Hash64(1), Hash64(2), Hash64(3), &[]),
            encode_review_token_v3(
                &DocumentId::parse("other/README.md").unwrap(),
                7,
                Hash64(1),
                Hash64(2),
                Hash64(3),
                &covered,
            ),
        ] {
            assert_ne!(base, changed);
        }
        // Covered invalidations are sorted defensively.
        let a = encode_review_token_v3(
            &doc,
            7,
            Hash64(1),
            Hash64(2),
            Hash64(3),
            &[(2, "two words here".into()), (1, "one reason here".into())],
        );
        let b = encode_review_token_v3(
            &doc,
            7,
            Hash64(1),
            Hash64(2),
            Hash64(3),
            &[(1, "one reason here".into()), (2, "two words here".into())],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn v3_golden_vectors_are_frozen() {
        // Freezing the exact bytes locks primitive order, option tags, and
        // sorted collection order for the release.
        let doc = DocumentId::parse("README.md").unwrap();
        assert_eq!(
            hex(&encode_review_token_v3(
                &doc,
                1,
                Hash64(0x0102030405060708),
                Hash64(0x1112131415161718),
                Hash64(0x2122232425262728),
                &[(1, "one reason here".into())],
            )),
            "0000000000000017\
6d656d6f7269612d7265766965772d746f6b656e2d7633\
0000000000000009\
524541444d452e6d64\
0000000000000001\
0102030405060708\
1112131415161718\
2122232425262728\
0000000000000001\
0000000000000001\
000000000000000f\
6f6e6520726561736f6e2068657265"
        );
        assert_eq!(
            hex(&encode_review_baseline(None)),
            "000000000000001a\
6d656d6f7269612d7265766965772d626173656c696e652d763100"
        );
        let minimal = ReviewContext {
            selection_version: 1,
            owner: "README.md".into(),
            ancestor_boundaries: vec![],
            descendant_boundaries: vec![],
            nested_repositories: vec![],
            policy_hash: Hash64(0),
            owned_paths: vec![],
            mapping: SectionMapIdentity::Absent,
            guidance: GuidanceDigest(Hash64(0)),
            imports: vec![],
            consumer_edges: vec![],
            providers: vec![],
        };
        assert_eq!(
            hex(&encode_review_context(&minimal)),
            "0000000000000019\
6d656d6f7269612d7265766965772d636f6e746578742d7631\
0000000000000001\
0000000000000009\
524541444d452e6d64\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000\
0000000000000000"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn primitive_layout_is_fixed() {
        let mut e = Encoder::new();
        e.u64(1).str("ab").list_len(0).option(Some(7u64), |e, v| {
            e.u64(v);
        });
        e.option::<u64>(None, |_, _| {});
        let bytes = e.finish();
        let expected: Vec<u8> = [
            vec![0, 0, 0, 0, 0, 0, 0, 1],
            vec![0, 0, 0, 0, 0, 0, 0, 2, b'a', b'b'],
            vec![0, 0, 0, 0, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 0, 0, 0, 0, 7],
            vec![0],
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn inputs_encoding_matches_manual_layout() {
        let manifest = InputManifest::new(
            DocumentId::parse("README.md").unwrap(),
            Hash64(0x0102030405060708),
            3,
            Hash64(0x1111111111111111),
            vec![FileInput {
                path: ProjectPath::parse("a.rs").unwrap(),
                bytes: 5,
                hash: Hash64(0x22),
            }],
            vec![ImportInput {
                document: DocumentId::parse("b/README.md").unwrap(),
                export_id: crate::document::ExportId::parse("summary").unwrap(),
                bytes: 9,
                hash: Hash64(0x33),
            }],
        )
        .unwrap();
        let bytes = encode_inputs(&manifest);
        let mut expected = Vec::new();
        let push_str = |v: &mut Vec<u8>, s: &str| {
            v.extend_from_slice(&(s.len() as u64).to_be_bytes());
            v.extend_from_slice(s.as_bytes());
        };
        push_str(&mut expected, "memoria.inputs.v2");
        push_str(&mut expected, "README.md");
        expected.extend_from_slice(&0x0102030405060708u64.to_be_bytes());
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(&0x1111111111111111u64.to_be_bytes());
        expected.extend_from_slice(&1u64.to_be_bytes());
        push_str(&mut expected, "a.rs");
        expected.extend_from_slice(&5u64.to_be_bytes());
        expected.extend_from_slice(&0x22u64.to_be_bytes());
        expected.extend_from_slice(&1u64.to_be_bytes());
        push_str(&mut expected, "b/README.md");
        push_str(&mut expected, "summary");
        expected.extend_from_slice(&9u64.to_be_bytes());
        expected.extend_from_slice(&0x33u64.to_be_bytes());
        assert_eq!(bytes, expected);
    }

    #[test]
    fn field_boundaries_do_not_collide() {
        // "ab" + "c" must differ from "a" + "bc" because lengths are encoded.
        let mut a = Encoder::new();
        a.str("ab").str("c");
        let mut b = Encoder::new();
        b.str("a").str("bc");
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn review_token_sorts_invalidations() {
        let manifest = InputManifest::new(
            DocumentId::parse("README.md").unwrap(),
            Hash64(0),
            0,
            Hash64(0),
            vec![],
            vec![],
        )
        .unwrap();
        let doc = DocumentId::parse("README.md").unwrap();
        let none = GuidanceDigest::default();
        let a = encode_review_token(
            &doc,
            1,
            &manifest,
            none,
            &[(2, "two words here".into()), (1, "one reason here".into())],
        );
        let b = encode_review_token(
            &doc,
            1,
            &manifest,
            none,
            &[(1, "one reason here".into()), (2, "two words here".into())],
        );
        assert_eq!(a, b);
        let c = encode_review_token(
            &doc,
            2,
            &manifest,
            none,
            &[(1, "one reason here".into()), (2, "two words here".into())],
        );
        assert_ne!(a, c);
        // The guidance digest binds the reviewed context into the token.
        let d = encode_review_token(
            &doc,
            1,
            &manifest,
            GuidanceDigest(Hash64(7)),
            &[(1, "one reason here".into()), (2, "two words here".into())],
        );
        assert_ne!(a, d);
    }

    #[test]
    fn guidance_encoding_separates_every_field() {
        use crate::guidance::{GuidanceEntry, GuidanceKind};
        use crate::path::DirPath;
        let entry = |source: &str, text: &str| GuidanceEntry {
            scope: DirPath::root(),
            source: source.to_string(),
            kind: GuidanceKind::Inline,
            text: text.to_string(),
        };
        assert_ne!(
            encode_guidance(&[entry("memoria.toml", "ab")]),
            encode_guidance(&[entry("memoria.tomla", "b")])
        );
        // Order is authored order, never a sorted set.
        assert_ne!(
            encode_guidance(&[entry("a", "one"), entry("b", "two")]),
            encode_guidance(&[entry("b", "two"), entry("a", "one")])
        );
        assert_eq!(encode_guidance(&[]), {
            let mut e = Encoder::new();
            e.str(GUIDANCE_DOMAIN).list_len(0);
            e.finish()
        });
    }
}
