//! Deterministic builders for the frozen `memoria.lock` representation
//! vectors.
//!
//! These fixtures measure representation and size. They are not project
//! review records: `tests/fixtures/portable-project` holds real version 2
//! acknowledgements produced through the CLI.
//!
//! `measurement-base.json` is a frozen logical input copied from the measured
//! legacy state. Reading it here is a test convenience, not a migration path:
//! the product ships no version 1 state decoder.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use memoria_domain::{
    DirPath, DocumentId, ExportId, FileInput, GitContext, GuidanceDigest, Hash64, ImportInput,
    InputManifest, Invalidation, InvalidationScope, ProjectPath, Reason, ReviewNote, ReviewRecord,
    ReviewResult, ReviewState, ReviewerName, Timestamp,
};
use memoria_infrastructure::hash::xxh3_64;
use memoria_infrastructure::json::{self, Json, Limits};

/// The frozen guidance digest of the measured project, from the planning
/// measurements. The builders supply it explicitly instead of deriving it
/// from a configuration file that later changes.
pub const FROZEN_GUIDANCE: &str = "4db0aeae8d6990a6";

/// One logical fixture: the state and each review's guidance digest.
#[derive(Debug, Clone)]
pub struct Vector {
    pub name: &'static str,
    pub state: ReviewState,
}

fn field<'a>(value: &'a Json, key: &str) -> &'a Json {
    match value {
        Json::Object(map) => map.get(key).unwrap_or_else(|| panic!("missing {key}")),
        other => panic!("expected an object for {key}, found {other:?}"),
    }
}

fn text(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        other => panic!("expected a string, found {other:?}"),
    }
}

fn number(value: &Json) -> u64 {
    match value {
        Json::Number(n) => *n,
        other => panic!("expected a number, found {other:?}"),
    }
}

fn array(value: &Json) -> &[Json] {
    match value {
        Json::Array(items) => items,
        other => panic!("expected an array, found {other:?}"),
    }
}

fn hash(value: &Json) -> Hash64 {
    Hash64::parse(&text(value)).expect("sixteen lowercase hexadecimal digits")
}

/// The measured legacy state, as typed domain values with one guidance digest.
pub fn measurement_base() -> ReviewState {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/state-v2/measurement-base.json");
    let bytes = std::fs::read(&path).expect("the frozen measurement input exists");
    let value = json::parse(&bytes, Limits::STATE).expect("the frozen input is strict JSON");
    let guidance = GuidanceDigest::parse(FROZEN_GUIDANCE).expect("a valid frozen digest");
    let mut reviews = BTreeMap::new();
    let reviews_value = field(&value, "reviews");
    let Json::Object(entries) = reviews_value else {
        panic!("reviews must be an object");
    };
    for (document, record) in entries {
        let document = DocumentId::parse(document).expect("a README path");
        reviews.insert(document.clone(), record_from(&document, record, guidance));
    }
    ReviewState {
        revision: number(field(&value, "revision")),
        next_invalidation_id: number(field(&value, "next_invalidation_id")),
        reviews,
        invalidations: Vec::new(),
    }
}

fn record_from(document: &DocumentId, value: &Json, guidance: GuidanceDigest) -> ReviewRecord {
    let manifest_value = field(value, "input_manifest");
    let files: Vec<FileInput> = array(field(manifest_value, "files"))
        .iter()
        .map(|file| FileInput {
            path: ProjectPath::parse(&text(field(file, "path"))).expect("a project path"),
            bytes: number(field(file, "bytes")),
            hash: hash(field(file, "hash")),
        })
        .collect();
    let imports: Vec<ImportInput> = array(field(manifest_value, "imports"))
        .iter()
        .map(|import| ImportInput {
            document: DocumentId::parse(&text(field(import, "document"))).expect("a README path"),
            export_id: ExportId::parse(&text(field(import, "export_id"))).expect("an export id"),
            bytes: number(field(import, "bytes")),
            hash: hash(field(import, "hash")),
        })
        .collect();
    let manifest = InputManifest::new(
        document.clone(),
        hash(field(manifest_value, "policy_hash")),
        number(field(manifest_value, "document_bytes")),
        hash(field(manifest_value, "document_hash")),
        files,
        imports,
    )
    .expect("a valid manifest");
    let git_value = field(value, "git");
    let git = GitContext {
        base_commit: match field(git_value, "base_commit") {
            Json::Null => None,
            other => Some(text(other)),
        },
        worktree_dirty: matches!(field(git_value, "worktree_dirty"), Json::Bool(true)),
    };
    ReviewRecord {
        revision: number(field(value, "revision")),
        input_fingerprint: hash(field(value, "input_fingerprint")),
        manifest,
        token_digest: hash(field(value, "token_digest")),
        guidance,
        reviewed_at: Timestamp(text(field(value, "reviewed_at"))),
        reviewer: ReviewerName::from_stored(text(field(value, "reviewer"))),
        result: ReviewResult::parse(&text(field(value, "result"))).expect("a review result"),
        note: ReviewNote::from_stored(text(field(value, "note"))),
        git,
        acknowledged_invalidations: array(field(value, "acknowledged_invalidations"))
            .iter()
            .map(number)
            .collect(),
    }
}

fn hex64(bytes: &[u8]) -> String {
    format!("{:016x}", xxh3_64(bytes))
}

/// Epoch seconds of an exact `YYYY-MM-DDTHH:MM:SSZ` timestamp.
fn epoch(stamp: &Timestamp) -> i64 {
    memoria_infrastructure::lock_codec::parse_timestamp(&stamp.0).expect("a UTC timestamp")
}

fn stamp(seconds: i64) -> Timestamp {
    Timestamp(memoria_infrastructure::lock_codec::format_timestamp(seconds).expect("in range"))
}

/// The measured `tiny` fixture: one review with its first two files.
pub fn tiny() -> ReviewState {
    let base = measurement_base();
    let root = DirPath::root().readme();
    let record = base.reviews.get(&root).expect("the root review").clone();
    let manifest = InputManifest::new(
        root.clone(),
        record.manifest.policy_hash,
        record.manifest.document_bytes,
        record.manifest.document_hash,
        record.manifest.files()[..2].to_vec(),
        Vec::new(),
    )
    .expect("a valid manifest");
    let mut reviews = BTreeMap::new();
    reviews.insert(root, ReviewRecord { manifest, ..record });
    ReviewState {
        revision: base.revision,
        next_invalidation_id: base.next_invalidation_id,
        reviews,
        invalidations: Vec::new(),
    }
}

/// The measured scaled fixtures: `n` copies of the base project under new
/// owner prefixes. `varied` also changes hashes, tokens, notes, reviewers,
/// review times, and Git commits.
pub fn scaled(n: usize, varied: bool) -> ReviewState {
    let base = measurement_base();
    let frozen = GuidanceDigest::parse(FROZEN_GUIDANCE).expect("a valid frozen digest");
    let mut reviews = BTreeMap::new();
    for i in 0..n {
        let prefix = format!("project-{i:03}/");
        for (old_document, old) in &base.reviews {
            let new_path = format!("{prefix}{}", old_document.as_str());
            let document = DocumentId::parse(&new_path).expect("a README path");
            let old_manifest = &old.manifest;
            let policy_hash = Hash64::parse(&hex64(
                format!("{new_path}{}", old_manifest.policy_hash.to_hex()).as_bytes(),
            ))
            .expect("a hash");
            let files: Vec<FileInput> = old_manifest
                .files()
                .iter()
                .map(|file| {
                    let path = format!("{prefix}{}", file.path.as_str());
                    let hash = if varied && file.bytes != 0 {
                        Hash64::parse(&hex64(
                            format!("{new_path}:{}:{}", path, file.hash.to_hex()).as_bytes(),
                        ))
                        .expect("a hash")
                    } else {
                        file.hash
                    };
                    FileInput {
                        path: ProjectPath::parse(&path).expect("a project path"),
                        bytes: file.bytes,
                        hash,
                    }
                })
                .collect();
            let imports: Vec<ImportInput> = old_manifest
                .imports()
                .iter()
                .map(|import| {
                    let provider = format!("{prefix}{}", import.document.as_str());
                    let hash = if varied && import.bytes != 0 {
                        Hash64::parse(&hex64(
                            format!("{new_path}:{}:{}", provider, import.hash.to_hex()).as_bytes(),
                        ))
                        .expect("a hash")
                    } else {
                        import.hash
                    };
                    ImportInput {
                        document: DocumentId::parse(&provider).expect("a README path"),
                        export_id: import.export_id.clone(),
                        bytes: import.bytes,
                        hash,
                    }
                })
                .collect();
            let document_hash = if varied {
                Hash64::parse(&hex64(format!("readme:{new_path}").as_bytes())).expect("a hash")
            } else {
                old_manifest.document_hash
            };
            let manifest = InputManifest::new(
                document.clone(),
                policy_hash,
                old_manifest.document_bytes,
                document_hash,
                files,
                imports,
            )
            .expect("a valid manifest");
            let mut record = ReviewRecord {
                revision: old.revision,
                input_fingerprint: old.input_fingerprint,
                manifest,
                token_digest: Hash64::parse(&hex64(format!("token:{new_path}").as_bytes()))
                    .expect("a hash"),
                guidance: if varied {
                    GuidanceDigest(
                        Hash64::parse(&hex64(format!("guidance-{}", i % 4).as_bytes()))
                            .expect("a hash"),
                    )
                } else {
                    frozen
                },
                reviewed_at: stamp(epoch(&old.reviewed_at) + (i as i64) * 3600),
                reviewer: old.reviewer.clone(),
                result: old.result,
                note: old.note.clone(),
                git: old.git.clone(),
                acknowledged_invalidations: old.acknowledged_invalidations.clone(),
            };
            if varied {
                record.note = ReviewNote::from_stored(format!(
                    "Verified the selected source behavior for {new_path}."
                ));
                record.reviewer = ReviewerName::from_stored(
                    ["GPT-Astra 6", "Fixture author", "Review team"][i % 3].to_string(),
                );
                record.git.base_commit = Some(sha1_hex(format!("commit-{i}").as_bytes()));
            }
            reviews.insert(document, record);
        }
    }
    ReviewState {
        revision: (2 * n * base.reviews.len() + 1) as u64,
        next_invalidation_id: 2,
        reviews,
        invalidations: Vec::new(),
    }
}

/// The measured `mixed` fixture: two scaled copies with varied metadata, a
/// null commit, a SHA-256 commit, Unicode text, and two invalidations, one
/// of which retains a target that no longer exists.
pub fn mixed() -> ReviewState {
    let mut state = scaled(2, true);
    let documents: Vec<DocumentId> = state.reviews.keys().cloned().collect();
    state.revision = 30;
    state.next_invalidation_id = 5;
    if let Some(record) = state.reviews.get_mut(&documents[0]) {
        record.git = GitContext {
            base_commit: None,
            worktree_dirty: false,
        };
    }
    if let Some(record) = state.reviews.get_mut(&documents[1]) {
        record.git = GitContext {
            base_commit: Some(sha256_hex(b"commit")),
            worktree_dirty: true,
        };
    }
    if let Some(record) = state.reviews.get_mut(&documents[2]) {
        record.note = ReviewNote::from_stored(
            "The reviewer checked the café workflow and its failure cases.".to_string(),
        );
    }
    let missing = DocumentId::parse("project-000/retired/README.md").expect("a README path");
    let mut all_targets = documents.clone();
    all_targets.push(missing.clone());
    all_targets.sort();
    let mut all_pending: Vec<DocumentId> = documents
        .iter()
        .step_by(2)
        .cloned()
        .chain([missing])
        .collect();
    all_pending.sort();
    let subtree = DirPath::parse("project-001").expect("a directory");
    let mut subtree_targets: Vec<DocumentId> = documents
        .iter()
        .filter(|d| d.as_str().starts_with("project-001/"))
        .cloned()
        .collect();
    subtree_targets.sort();
    let subtree_pending: Vec<DocumentId> = subtree_targets[1..].to_vec();
    state.invalidations = vec![
        Invalidation {
            id: 3,
            scope: InvalidationScope::All,
            reason: Reason::from_stored(
                "The semantic review covers all active documents".to_string(),
            ),
            created_at: Timestamp("2026-09-08T08:00:00Z".to_string()),
            targets: all_targets,
            pending: all_pending,
        },
        Invalidation {
            id: 4,
            scope: InvalidationScope::Subtree(subtree),
            reason: Reason::from_stored(
                "The local workflow changed after the last review".to_string(),
            ),
            created_at: Timestamp("2026-09-08T08:01:00Z".to_string()),
            targets: subtree_targets,
            pending: subtree_pending,
        },
    ];
    state
}

/// The eight measured fixtures in Section 3.1 order.
pub fn all() -> Vec<Vector> {
    vec![
        Vector {
            name: "empty",
            state: ReviewState::empty(),
        },
        Vector {
            name: "tiny",
            state: tiny(),
        },
        Vector {
            name: "current",
            state: measurement_base(),
        },
        Vector {
            name: "mixed",
            state: mixed(),
        },
        Vector {
            name: "scaled-shared-600",
            state: scaled(100, false),
        },
        Vector {
            name: "scaled-varied-60",
            state: scaled(10, true),
        },
        Vector {
            name: "scaled-varied-600",
            state: scaled(100, true),
        },
        Vector {
            name: "scaled-varied-6000",
            state: scaled(1000, true),
        },
    ]
}

// --------------------------------------------------------------- test hashes

/// SHA-1 of `bytes`, as lowercase hexadecimal. Used only to reproduce the
/// frozen fixtures' synthetic Git commit identities.
pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64) * 8;
    message.push(0x80);
    while !message.len().is_multiple_of(64) || message.len() % 64 != 56 {
        if message.len() % 64 == 56 {
            break;
        }
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (index, chunk) in block.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// SHA-256 of `bytes`, as lowercase hexadecimal.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, chunk) in block.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for index in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for index in 0..8 {
            h[index] = h[index].wrapping_add(v[index]);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}
