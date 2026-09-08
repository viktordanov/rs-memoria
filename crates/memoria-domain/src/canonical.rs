//! Canonical byte encoding `C` for fingerprints and review tokens.
//!
//! Primitives: an unsigned integer is eight big-endian bytes; a byte string
//! is its length followed by its bytes; a list is its count followed by its
//! elements; an option is one byte (`0` or `1`) followed by the value when
//! present. Every hashed structure begins with a length-prefixed
//! domain-separation string.

use crate::guidance::{GuidanceDigest, GuidanceEntry};
use crate::manifest::InputManifest;
use crate::path::DocumentId;
use crate::policy::EffectivePolicy;

pub const POLICY_DOMAIN: &str = "memoria.policy.v2";
pub const INPUTS_DOMAIN: &str = "memoria.inputs.v2";
pub const REVIEW_TOKEN_DOMAIN: &str = "memoria.review-token.v2";
pub const PACKET_DOMAIN: &str = "memoria.packet.v2";
pub const GUIDANCE_DOMAIN: &str = "memoria.guidance.v1";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileInput, Hash64, ImportInput};
    use crate::path::ProjectPath;

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
