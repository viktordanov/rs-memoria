//! The committed `memoria.lock` contract: frozen bytes, strict decoding, and
//! the measured representation vectors.

mod common;
#[path = "common/state_vectors.rs"]
mod state_vectors;

use common::*;

use std::path::{Path, PathBuf};

use memoria_infrastructure::lock_codec;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/state-v2")
        .join(name)
}

fn frozen(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("cannot read {name}: {e}"))
}

fn fixture_v3(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/state-v3")
        .join(name)
}

/// A committed format 3 golden vector. `MEMORIA_WRITE_VECTORS` rewrites it
/// from the given bytes; the assertion then still compares the two.
fn frozen_v3(name: &str, encoded: &[u8]) -> Vec<u8> {
    let path = fixture_v3(name);
    if std::env::var_os("MEMORIA_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, encoded).unwrap();
    }
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read state-v3/{name}: {e}"))
}

/// Frame a raw payload exactly as the writer does, with any format byte.
fn frame(format: u8, payload: &[u8]) -> Vec<u8> {
    let mut wire = Vec::new();
    wire.extend_from_slice(&lock_codec::MAGIC);
    wire.push(format);
    wire.push(lock_codec::CODEC_RAW);
    let mut length = payload.len() as u64;
    loop {
        let byte = (length & 0x7f) as u8;
        length >>= 7;
        if length == 0 {
            wire.push(byte);
            break;
        }
        wire.push(byte | 0x80);
    }
    wire.extend_from_slice(payload);
    let checksum = memoria_infrastructure::hash::xxh3_128(&wire);
    wire.extend_from_slice(&checksum);
    wire
}

#[test]
fn state_v2_golden_round_trip_is_deterministic() {
    for name in [
        "empty.lock",
        "tiny.lock",
        "current.lock",
        "mixed.lock",
        "merge-base.lock",
        "merge-left.lock",
        "merge-right.lock",
        "merge-same.lock",
        "documents.lock",
    ] {
        // The committed format 2 vectors are read-compatibility input.
        let bytes = frozen(name);
        let decoded = lock_codec::decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded.file_bytes, bytes.len() as u64, "{name}");
        assert_eq!(
            decoded.format_version,
            lock_codec::LEGACY_FORMAT_VERSION,
            "{name}"
        );
        // Format 2 stores no coverage evidence: every record is unrecorded.
        assert!(
            decoded
                .state
                .reviews
                .values()
                .all(|r| r.coverage == memoria_domain::CoverageEvidence::Unrecorded),
            "{name}"
        );
        assert_eq!(decoded.checksum.len(), 32, "{name}");
        assert!(
            decoded
                .checksum
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{name}: the checksum must be lowercase hexadecimal"
        );
        // Encoding the decoded state gives the committed format 3 vector
        // byte-exactly. The writer never writes format 2.
        let reencoded =
            lock_codec::encode(&decoded.state).unwrap_or_else(|e| panic!("{name} re-encode: {e}"));
        assert_eq!(
            reencoded,
            frozen_v3(name, &reencoded),
            "{name} is not byte-stable"
        );
        assert_eq!(reencoded[4], 3, "{name}: the writer always writes format 3");
        // Decoding that vector yields the same state, and it round-trips.
        let again = lock_codec::decode(&reencoded).unwrap();
        assert_eq!(again.format_version, lock_codec::FORMAT_VERSION, "{name}");
        assert_eq!(again.state, decoded.state, "{name}");
        assert_eq!(again.guidance, decoded.guidance, "{name}");
        assert_eq!(
            lock_codec::encode(&again.state).unwrap(),
            reencoded,
            "{name}"
        );
    }
}

#[test]
fn state_v3_size_vectors_match_the_measured_contract() {
    // Section 3.1 of the approved plan, re-measured for format 3 (plan
    // §17.5). Reviews / files / imports / invalidations, then the final lock
    // bytes and codec. The measured states predate evidence, so each review
    // row carries one coverage byte (`0`).
    let expected: [(&str, usize, usize, usize, usize, usize, u8); 8] = [
        ("empty", 0, 0, 0, 0, 23, lock_codec::CODEC_RAW),
        ("tiny", 1, 2, 0, 0, 276, lock_codec::CODEC_RAW),
        ("current", 6, 83, 6, 0, 2198, lock_codec::CODEC_ZSTD),
        ("mixed", 12, 166, 12, 2, 3379, lock_codec::CODEC_ZSTD),
        (
            "scaled-shared-600",
            600,
            8300,
            600,
            0,
            18463,
            lock_codec::CODEC_ZSTD,
        ),
        (
            "scaled-varied-60",
            60,
            830,
            60,
            0,
            11349,
            lock_codec::CODEC_ZSTD,
        ),
        (
            "scaled-varied-600",
            600,
            8300,
            600,
            0,
            100959,
            lock_codec::CODEC_ZSTD,
        ),
        (
            "scaled-varied-6000",
            6000,
            83000,
            6000,
            0,
            993262,
            lock_codec::CODEC_ZSTD,
        ),
    ];
    let vectors = state_vectors::all();
    assert_eq!(vectors.len(), expected.len());
    for (vector, (name, reviews, files, imports, invalidations, bytes, codec)) in
        vectors.iter().zip(expected)
    {
        assert_eq!(vector.name, name);
        assert_eq!(vector.state.reviews.len(), reviews, "{name}: reviews");
        let file_count: usize = vector
            .state
            .reviews
            .values()
            .map(|r| r.manifest.files().len())
            .sum();
        let import_count: usize = vector
            .state
            .reviews
            .values()
            .map(|r| r.manifest.imports().len())
            .sum();
        assert_eq!(file_count, files, "{name}: files");
        assert_eq!(import_count, imports, "{name}: imports");
        assert_eq!(
            vector.state.invalidations.len(),
            invalidations,
            "{name}: invalidations"
        );
        let encoded = lock_codec::encode(&vector.state)
            .unwrap_or_else(|e| panic!("{name} does not encode: {e}"));
        assert_eq!(encoded.len(), bytes, "{name}: final lock bytes");
        assert_eq!(encoded[5], codec, "{name}: codec choice");
        // Every vector decodes back to the same logical state.
        let decoded = lock_codec::decode(&encoded).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded.state.revision, vector.state.revision, "{name}");
        assert_eq!(
            decoded.state.reviews.len(),
            vector.state.reviews.len(),
            "{name}"
        );
    }
}

#[test]
fn measured_vectors_reproduce_the_frozen_committed_files() {
    // The four small vectors are also committed as fixtures, so the frozen
    // bytes and the builders cannot drift apart. The builders give the same
    // logical states as the format 2 files, and encode to the format 3 ones.
    for (name, file) in [
        ("empty", "empty.lock"),
        ("tiny", "tiny.lock"),
        ("current", "current.lock"),
        ("mixed", "mixed.lock"),
    ] {
        let vector = state_vectors::all()
            .into_iter()
            .find(|v| v.name == name)
            .expect("a measured vector");
        let encoded = lock_codec::encode(&vector.state).unwrap();
        assert_eq!(
            encoded,
            frozen_v3(file, &encoded),
            "{name} differs from {file}"
        );
        // Fingerprints are derived on decode, so the encoding is the identity.
        assert_eq!(
            lock_codec::encode(&lock_codec::decode(&frozen(file)).unwrap().state).unwrap(),
            encoded,
            "{name}: the format 2 file holds the same state"
        );
    }
}

#[test]
fn the_documents_vector_keeps_the_row_rule_with_wider_identities() {
    let state = state_vectors::documents();
    let encoded = lock_codec::encode(&state).unwrap();
    assert_eq!(
        encoded,
        frozen_v3("documents.lock", &encoded),
        "documents.lock is frozen"
    );
    // The committed format 2 file holds the same logical state.
    assert_eq!(
        lock_codec::encode(&lock_codec::decode(&frozen("documents.lock")).unwrap().state).unwrap(),
        encoded
    );
    let decoded = lock_codec::decode(&encoded).unwrap();
    assert_eq!(decoded.format_version, 3);
    // Fingerprints are derived on decode, so byte stability is the identity.
    assert_eq!(lock_codec::encode(&decoded.state).unwrap(), encoded);
    assert!(
        decoded
            .state
            .reviews
            .keys()
            .any(|id| id.as_str() == "docs/guide.md")
    );
    // A record's files still lie inside its document's folder: an opted-in
    // record that names a file outside it cannot be stored.
    let mut outside = state.clone();
    let guide = memoria_domain::DocumentId::parse("docs/guide.md").unwrap();
    let record = outside.reviews.get(&guide).unwrap().clone();
    let manifest = memoria_domain::InputManifest::new(
        guide.clone(),
        record.manifest.policy_hash,
        record.manifest.document_bytes,
        record.manifest.document_hash,
        vec![memoria_domain::FileInput {
            path: memoria_domain::ProjectPath::parse("Cargo.toml").unwrap(),
            bytes: 1,
            hash: memoria_domain::Hash64(1),
        }],
        Vec::new(),
    )
    .unwrap();
    outside
        .reviews
        .insert(guide, memoria_domain::ReviewRecord { manifest, ..record });
    assert!(lock_codec::encode(&outside).is_err());
}

#[test]
fn state_v2_rejects_corruption_before_mutation() {
    let good = frozen("current.lock");
    // Every single-byte flip in the header, body, and checksum is rejected.
    for index in [0, 1, 2, 3, 4, 5, 6, good.len() / 2, good.len() - 1] {
        let mut bytes = good.clone();
        bytes[index] ^= 0x01;
        let result = lock_codec::decode(&bytes);
        assert!(result.is_err(), "byte {index} was accepted");
    }
    // Every truncation point is rejected.
    for cut in 0..good.len() {
        assert!(
            lock_codec::decode(&good[..cut]).is_err(),
            "a {cut}-byte prefix was accepted"
        );
    }
    // Trailing bytes and concatenated frames are rejected.
    let mut extended = good.clone();
    extended.push(0);
    assert!(lock_codec::decode(&extended).is_err());
    let mut doubled = good.clone();
    doubled.extend_from_slice(&good);
    assert!(lock_codec::decode(&doubled).is_err());
    // An unsupported version and codec have their own diagnostics. Formats
    // 2 and 3 are read; every other format byte is unsupported.
    for format in [0u8, 1, 4, 255] {
        let mut version = good.clone();
        version[4] = format;
        match lock_codec::decode(&version) {
            Err(lock_codec::LockError::UnsupportedSchema(message)) => assert_eq!(
                message,
                format!(
                    "unsupported memoria.lock format version {format}; this release supports versions 2 and 3"
                )
            ),
            other => panic!("format {format}: {other:?}"),
        }
    }
    let mut codec = good.clone();
    codec[5] = 2;
    assert!(matches!(
        lock_codec::decode(&codec),
        Err(lock_codec::LockError::UnsupportedCodec(_))
    ));
    // A zero-byte file is corruption, not missing state.
    assert!(matches!(
        lock_codec::decode(&[]),
        Err(lock_codec::LockError::Corrupt(_))
    ));
}

#[test]
fn state_v2_rejects_invalid_normalization_with_valid_checksums() {
    // Each case rewrites the payload and recomputes the frame, so the
    // decoder reaches its canonical-form checks instead of the checksum.
    let base = lock_codec::decode(&frozen("tiny.lock")).unwrap();
    let payload = lock_codec::encode_payload(&base.state).unwrap();
    let reframe = |payload: &[u8]| -> Vec<u8> {
        let mut wire = Vec::new();
        wire.extend_from_slice(&lock_codec::MAGIC);
        wire.push(lock_codec::FORMAT_VERSION);
        wire.push(lock_codec::CODEC_RAW);
        let mut length = payload.len() as u64;
        loop {
            let byte = (length & 0x7f) as u8;
            length >>= 7;
            if length == 0 {
                wire.push(byte);
                break;
            }
            wire.push(byte | 0x80);
        }
        wire.extend_from_slice(payload);
        let checksum = memoria_infrastructure::hash::xxh3_128(&wire);
        wire.extend_from_slice(&checksum);
        wire
    };
    // The valid payload still decodes through this framing helper.
    assert!(lock_codec::decode(&reframe(&payload)).is_ok());
    // An overlong integer encoding of the revision.
    let mut overlong = vec![0x80, 0x00];
    overlong.extend_from_slice(&payload[1..]);
    assert!(
        lock_codec::decode(&reframe(&overlong)).is_err(),
        "an overlong integer was accepted"
    );
    // A varint that does not fit in 64 bits.
    let mut overflow = vec![0xff; 10];
    overflow.push(0x7f);
    overflow.extend_from_slice(&payload[1..]);
    assert!(lock_codec::decode(&reframe(&overflow)).is_err());
    // Truncated payloads with valid checksums.
    for cut in 1..payload.len() {
        assert!(
            lock_codec::decode(&reframe(&payload[..cut])).is_err(),
            "a {cut}-byte payload was accepted"
        );
    }
    // Trailing payload bytes.
    let mut trailing = payload.clone();
    trailing.push(0);
    assert!(lock_codec::decode(&reframe(&trailing)).is_err());
    // The full schema must not encode the initial empty state.
    let empty_full = {
        let mut out = vec![0u8, 1u8, 0u8];
        // Zero-length tables for all six table sections, then no records.
        out.extend_from_slice(&[0; 8]);
        out
    };
    assert!(
        lock_codec::decode(&reframe(&empty_full)).is_err(),
        "the initial empty state must use the empty payload"
    );
    // The empty payload is the only encoding of initial state.
    let empty = lock_codec::encode(&memoria_domain::ReviewState::empty()).unwrap();
    assert_eq!(empty.len(), 23);
    assert_eq!(
        lock_codec::decode(&empty).unwrap().state,
        memoria_domain::ReviewState::empty()
    );
}

#[test]
fn state_v2_enforces_identical_read_and_write_limits() {
    // A payload above the declared limit is refused before decompression.
    let mut oversized = Vec::new();
    oversized.extend_from_slice(&lock_codec::MAGIC);
    oversized.push(lock_codec::FORMAT_VERSION);
    oversized.push(lock_codec::CODEC_RAW);
    // A declared length above the 64 MiB payload limit.
    let mut declared = lock_codec::MAX_PAYLOAD_BYTES + 1;
    loop {
        let byte = (declared & 0x7f) as u8;
        declared >>= 7;
        if declared == 0 {
            oversized.push(byte);
            break;
        }
        oversized.push(byte | 0x80);
    }
    oversized.extend_from_slice(b"body");
    let checksum = memoria_infrastructure::hash::xxh3_128(&oversized);
    oversized.extend_from_slice(&checksum);
    assert!(matches!(
        lock_codec::decode(&oversized),
        Err(lock_codec::LockError::Limit(_))
    ));
    // A file above the encoded limit is refused before full allocation.
    let huge = vec![0u8; (lock_codec::MAX_FILE_BYTES + 1) as usize];
    assert!(matches!(
        lock_codec::decode(&huge),
        Err(lock_codec::LockError::Limit(_))
    ));
}

#[test]
fn state_v2_retains_review_contract_without_stored_input_hash() {
    // Decoding recomputes each input fingerprint from the reconstructed
    // manifest, so it can never disagree with the manifest it describes.
    let decoded = lock_codec::decode(&frozen("mixed.lock")).unwrap();
    assert_eq!(decoded.state.reviews.len(), 12);
    for (document, record) in &decoded.state.reviews {
        let expected = memoria_domain::Hash64(memoria_infrastructure::hash::xxh3_64(
            &memoria_domain::canonical::encode_inputs(&record.manifest),
        ));
        assert_eq!(record.input_fingerprint, expected, "{document}");
        assert_eq!(&record.manifest.document, document);
        // Guidance travels with the review, as a value, not a manifest field.
        assert_eq!(
            decoded.guidance.get(document).copied(),
            Some(record.guidance)
        );
    }
    // A null commit, a SHA-1 commit, and a SHA-256 commit all round trip.
    let commits: Vec<Option<usize>> = decoded
        .state
        .reviews
        .values()
        .map(|r| r.git.base_commit.as_ref().map(|c| c.len()))
        .collect();
    assert!(commits.contains(&None), "a null commit is represented");
    assert!(commits.contains(&Some(40)), "SHA-1 is represented");
    assert!(commits.contains(&Some(64)), "SHA-256 is represented");
    // Dormant targets survive: an invalidation keeps a target that no longer
    // has a review record.
    let dormant = decoded
        .state
        .invalidations
        .iter()
        .flat_map(|i| i.targets.iter())
        .find(|t| !decoded.state.reviews.contains_key(*t));
    assert!(dormant.is_some(), "a dormant target is retained");
    // Pending is a nonempty subset of targets.
    for invalidation in &decoded.state.invalidations {
        assert!(!invalidation.pending.is_empty());
        assert!(
            invalidation
                .pending
                .iter()
                .all(|d| invalidation.targets.contains(d))
        );
    }
}

#[test]
fn state_v2_profile_uses_the_pinned_bundled_library() {
    // The frozen vectors were produced against Zstandard 1.5.7 through the
    // documented profile. Reproducing them proves the bundled library and
    // the explicit parameters are both in force.
    let bytes = frozen("current.lock");
    assert_eq!(bytes[5], lock_codec::CODEC_ZSTD);
    let decoded = lock_codec::decode(&bytes).unwrap();
    let encoded = lock_codec::encode(&decoded.state).unwrap();
    assert_eq!(encoded[5], lock_codec::CODEC_ZSTD);
    assert_eq!(encoded, frozen_v3("current.lock", &encoded));
    // The frame carries no Zstandard content checksum and no dictionary id.
    // Its own trailer is the only checksum, and it covers every prior byte.
    let split = bytes.len() - 16;
    assert_eq!(
        memoria_infrastructure::hash::xxh3_128(&bytes[..split])[..],
        bytes[split..]
    );
}

#[test]
fn state_binary_merge_requires_an_intact_baseline() {
    // The artifact begins with NUL, so Git treats it as binary and refuses
    // a line merge. Recovery selects one intact baseline instead.
    let base = fixture("merge-base.lock");
    for (label, left, right) in [
        ("competing", "merge-left.lock", "merge-right.lock"),
        ("identical", "merge-same.lock", "merge-same.lock"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut merge = std::process::Command::new("git");
        merge
            .arg("merge-file")
            .arg("-p")
            .arg(fixture(left))
            .arg(&base)
            .arg(fixture(right));
        isolate_git(&mut merge, home.path());
        let output = merge.output().unwrap();
        if label == "competing" {
            assert_ne!(output.status.code(), Some(0), "{label}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("binary"),
                "{label}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    // Every conflicting version is independently intact, so a reviewer can
    // inspect each one and choose a baseline.
    for name in ["merge-base.lock", "merge-left.lock", "merge-right.lock"] {
        let decoded = lock_codec::decode(&frozen(name)).unwrap();
        assert!(!decoded.state.reviews.is_empty(), "{name}");
    }
    let base_state = lock_codec::decode(&frozen("merge-base.lock"))
        .unwrap()
        .state;
    let left_state = lock_codec::decode(&frozen("merge-left.lock"))
        .unwrap()
        .state;
    let right_state = lock_codec::decode(&frozen("merge-right.lock"))
        .unwrap()
        .state;
    assert_ne!(left_state, right_state, "the branches differ");
    assert_ne!(left_state, base_state);
}

#[test]
fn state_v2_shared_tables_cannot_amplify_past_limits() {
    // A long string referenced many times expands past the string-byte
    // limit even though the encoded payload stays small.
    let mut state = memoria_domain::ReviewState::empty();
    state.revision = 1;
    let long = "x".repeat(200_000);
    let mut files = Vec::new();
    for index in 0..400 {
        files.push(memoria_domain::FileInput {
            path: memoria_domain::ProjectPath::parse(&format!("{long}{index}.rs")).unwrap(),
            bytes: index,
            hash: memoria_domain::Hash64(index),
        });
    }
    let manifest = memoria_domain::InputManifest::new(
        memoria_domain::DirPath::root().readme(),
        memoria_domain::Hash64(1),
        1,
        memoria_domain::Hash64(2),
        files,
        vec![],
    )
    .unwrap();
    state.reviews.insert(
        memoria_domain::DirPath::root().readme(),
        memoria_domain::ReviewRecord {
            revision: 1,
            manifest,
            input_fingerprint: memoria_domain::Hash64(0),
            token_digest: memoria_domain::Hash64(0),
            guidance: memoria_domain::GuidanceDigest::default(),
            reviewed_at: memoria_domain::Timestamp("2026-09-08T00:00:00Z".into()),
            reviewer: memoria_domain::ReviewerName::from_stored("fixture".into()),
            result: memoria_domain::ReviewResult::NoUpdate,
            note: memoria_domain::ReviewNote::from_stored(
                "The current summary describes all reviewed inputs.".into(),
            ),
            git: memoria_domain::GitContext::default(),
            acknowledged_invalidations: vec![],
            coverage: memoria_domain::CoverageEvidence::Unrecorded,
        },
    );
    // Front coding keeps the encoded form small, so the serialized size
    // limit alone would not catch this. The writer must still refuse it,
    // because the reader cannot expand it.
    let unverified = lock_codec::encode_unverified(&state).unwrap();
    assert!(
        (unverified.len() as u64) < lock_codec::MAX_FILE_BYTES,
        "the encoded artifact stays small: {} bytes",
        unverified.len()
    );
    assert!(
        matches!(
            lock_codec::decode(&unverified),
            Err(lock_codec::LockError::Limit(_))
        ),
        "the reader rejects the expansion"
    );
    // The writer refuses the same state, with a message that names the
    // read-back failure. A successful acknowledgement can never replace
    // readable state with unreadable state.
    match lock_codec::encode(&state) {
        Err(lock_codec::LockError::Limit(message)) => {
            assert!(message.contains("cannot read back"), "{message}");
        }
        other => panic!("the writer accepted unreadable state: {other:?}"),
    }
}

#[test]
fn state_v2_charges_every_materialized_reference() {
    // Every reference that the reader rebuilds is charged, not only the file
    // rows: imports, review documents, invalidation scopes and their target
    // lists all materialize a table path or string.
    let long = "d".repeat(200_000);
    let provider = format!("{long}/README.md");
    let document = memoria_domain::DocumentId::parse(&provider).unwrap();

    // 1. Repeated imports of one long provider path.
    let mut imports = Vec::new();
    for index in 0..400u64 {
        imports.push(memoria_domain::ImportInput {
            document: document.clone(),
            export_id: memoria_domain::ExportId::parse(&format!("export-{index}")).unwrap(),
            bytes: index,
            hash: memoria_domain::Hash64(index),
        });
    }
    let mut state = review_state_with(memoria_domain::DirPath::root().readme(), vec![], imports);
    assert_refused_before_allocation(&state, "repeated imports");

    // 2. Repeated invalidation targets naming one long document path.
    state = review_state_with(memoria_domain::DirPath::root().readme(), vec![], vec![]);
    let mut targets = Vec::new();
    for index in 0..400u64 {
        targets
            .push(memoria_domain::DocumentId::parse(&format!("{long}{index}/README.md")).unwrap());
    }
    targets.sort();
    state.next_invalidation_id = 2;
    state.invalidations.push(memoria_domain::Invalidation {
        id: 1,
        scope: memoria_domain::InvalidationScope::All,
        reason: memoria_domain::Reason::from_stored(
            "The documentation policy changed for every boundary.".into(),
        ),
        created_at: memoria_domain::Timestamp("2026-09-08T00:00:00Z".into()),
        targets: targets.clone(),
        pending: targets,
    });
    assert_refused_before_allocation(&state, "invalidation targets");

    // 3. A review row whose own document path is long, repeated across rows.
    let mut state = memoria_domain::ReviewState::empty();
    state.revision = 1;
    for index in 0..400u64 {
        let document =
            memoria_domain::DocumentId::parse(&format!("{long}{index}/README.md")).unwrap();
        let record = review_state_with(document.clone(), vec![], vec![])
            .reviews
            .remove(&document)
            .unwrap();
        state.reviews.insert(document, record);
    }
    assert_refused_before_allocation(&state, "review documents");
}

/// One review row for `document`, with the given files and imports.
fn review_state_with(
    document: memoria_domain::DocumentId,
    files: Vec<memoria_domain::FileInput>,
    imports: Vec<memoria_domain::ImportInput>,
) -> memoria_domain::ReviewState {
    let mut state = memoria_domain::ReviewState::empty();
    state.revision = 1;
    let manifest = memoria_domain::InputManifest::new(
        document.clone(),
        memoria_domain::Hash64(1),
        1,
        memoria_domain::Hash64(2),
        files,
        imports,
    )
    .unwrap();
    state.reviews.insert(
        document,
        memoria_domain::ReviewRecord {
            revision: 1,
            manifest,
            input_fingerprint: memoria_domain::Hash64(0),
            token_digest: memoria_domain::Hash64(0),
            guidance: memoria_domain::GuidanceDigest::default(),
            reviewed_at: memoria_domain::Timestamp("2026-09-08T00:00:00Z".into()),
            reviewer: memoria_domain::ReviewerName::from_stored("fixture".into()),
            result: memoria_domain::ReviewResult::NoUpdate,
            note: memoria_domain::ReviewNote::from_stored(
                "The current summary describes all reviewed inputs.".into(),
            ),
            git: memoria_domain::GitContext::default(),
            acknowledged_invalidations: vec![],
            coverage: memoria_domain::CoverageEvidence::Unrecorded,
        },
    );
    state
}

/// The writer refuses this state before it builds a table, and the reader
/// refuses the same bytes if a fixture writes them anyway.
fn assert_refused_before_allocation(state: &memoria_domain::ReviewState, label: &str) {
    match lock_codec::encode(state) {
        Err(lock_codec::LockError::Limit(message)) => {
            assert!(
                message.contains("cannot read back") && message.contains("string bytes"),
                "{label}: {message}"
            );
        }
        other => panic!("{label}: the writer accepted an oversized expansion: {other:?}"),
    }
    // The same state, written by a fixture that skips the preflight, stays
    // small on the wire and is still refused by the reader.
    let unverified = lock_codec::encode_unverified(state).unwrap();
    assert!(
        (unverified.len() as u64) < lock_codec::MAX_FILE_BYTES,
        "{label}: the artifact is small: {} bytes",
        unverified.len()
    );
    assert!(
        matches!(
            lock_codec::decode(&unverified),
            Err(lock_codec::LockError::Limit(_))
        ),
        "{label}: the reader accepted the expansion"
    );
}

#[test]
fn every_successful_encode_decodes() {
    // Writer and reader symmetry, over the frozen vectors and over a state
    // built to sit just inside the expansion budget.
    for vector in state_vectors::all() {
        let bytes = lock_codec::encode(&vector.state)
            .unwrap_or_else(|e| panic!("{}: the writer refused a valid state: {e}", vector.name));
        let decoded = lock_codec::decode(&bytes)
            .unwrap_or_else(|e| panic!("{}: the reader refused written bytes: {e}", vector.name));
        // The stored fields round trip exactly. `input_fingerprint` is
        // derived on read, not stored, so re-encoding is the equality that
        // the format actually promises.
        assert_eq!(
            lock_codec::encode(&decoded.state).unwrap(),
            bytes,
            "{}",
            vector.name
        );
        assert_eq!(
            decoded.state.reviews.len(),
            vector.state.reviews.len(),
            "{}",
            vector.name
        );
    }
    // A state whose expansion sits under the limit encodes and decodes.
    let mut state = memoria_domain::ReviewState::empty();
    state.revision = 1;
    let long = "y".repeat(60_000);
    let files: Vec<memoria_domain::FileInput> = (0..100)
        .map(|index| memoria_domain::FileInput {
            path: memoria_domain::ProjectPath::parse(&format!("{long}{index}.rs")).unwrap(),
            bytes: index,
            hash: memoria_domain::Hash64(index),
        })
        .collect();
    let manifest = memoria_domain::InputManifest::new(
        memoria_domain::DirPath::root().readme(),
        memoria_domain::Hash64(1),
        1,
        memoria_domain::Hash64(2),
        files,
        vec![],
    )
    .unwrap();
    state.reviews.insert(
        memoria_domain::DirPath::root().readme(),
        memoria_domain::ReviewRecord {
            revision: 1,
            manifest,
            input_fingerprint: memoria_domain::Hash64(0),
            token_digest: memoria_domain::Hash64(0),
            guidance: memoria_domain::GuidanceDigest::default(),
            reviewed_at: memoria_domain::Timestamp("2026-09-08T00:00:00Z".into()),
            reviewer: memoria_domain::ReviewerName::from_stored("fixture".into()),
            result: memoria_domain::ReviewResult::NoUpdate,
            note: memoria_domain::ReviewNote::from_stored(
                "The current summary describes all reviewed inputs.".into(),
            ),
            git: memoria_domain::GitContext::default(),
            acknowledged_invalidations: vec![],
            coverage: memoria_domain::CoverageEvidence::Unrecorded,
        },
    );
    let bytes = lock_codec::encode(&state).expect("a state inside the budget encodes");
    let decoded = lock_codec::decode(&bytes).expect("the reader accepts written bytes");
    assert_eq!(lock_codec::encode(&decoded.state).unwrap(), bytes);
}

#[test]
fn an_oversize_write_preserves_the_previous_state_bytes() {
    // The store must refuse before replacement, leaving the committed file
    // exactly as it was.
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    let mut state = project.decoded_state();
    let long = "z".repeat(200_000);
    let mut files = Vec::new();
    for index in 0..500u64 {
        files.push(memoria_domain::FileInput {
            path: memoria_domain::ProjectPath::parse(&format!("{long}{index}.rs")).unwrap(),
            bytes: index,
            hash: memoria_domain::Hash64(index),
        });
    }
    let root = memoria_domain::DirPath::root().readme();
    let record = state.reviews.get(&root).expect("the root review").clone();
    let manifest = memoria_domain::InputManifest::new(
        root.clone(),
        record.manifest.policy_hash,
        record.manifest.document_bytes,
        record.manifest.document_hash,
        files,
        vec![],
    )
    .unwrap();
    state
        .reviews
        .insert(root, memoria_domain::ReviewRecord { manifest, ..record });
    let store = memoria_infrastructure::LockStateStore::new(project.root.clone());
    use memoria_application::ports::StateStore as _;
    let failure = store.save(&state, Some(&before)).unwrap_err();
    assert!(
        matches!(
            failure,
            memoria_application::ports::StateFailure::LimitExceeded(_)
        ),
        "{failure:?}"
    );
    assert_eq!(project.state(), before, "the previous state survives");
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "the project is still readable"
    );
}

#[test]
fn state_v2_keeps_partial_and_dormant_invalidations() {
    let decoded = lock_codec::decode(&frozen("mixed.lock")).unwrap();
    let reencoded = lock_codec::encode(&decoded.state).unwrap();
    let again = lock_codec::decode(&reencoded).unwrap();
    assert_eq!(again.state.invalidations, decoded.state.invalidations);
    // Partial clearing is representable: pending is a strict subset here.
    let partial = decoded
        .state
        .invalidations
        .iter()
        .find(|i| i.pending.len() < i.targets.len())
        .expect("a partially cleared invalidation");
    assert!(!partial.pending.is_empty());
    // Acknowledged identifiers survive without their reasons.
    assert!(decoded.state.reviews.values().all(|r| {
        r.acknowledged_invalidations
            .iter()
            .all(|id| *id < decoded.state.next_invalidation_id)
    }));
}

// ------------------------------------------------------- CLI-level contracts

#[test]
fn state_inspect_is_read_only_inside_and_outside_git() {
    let project = Project::seed();
    project.baseline();
    let before = project.tree_snapshot();
    // Default inspection of the selected project.
    let (code, value) = project.json(&["state", "inspect"]);
    assert_eq!(code, 0, "{value:?}");
    assert_eq!(get_str(&value, &["data", "path"]), "memoria.lock");
    assert_eq!(get_u64(&value, &["data", "format_version"]), 3);
    let checksum = get_str(&value, &["data", "checksum"]).to_string();
    assert_eq!(checksum.len(), 32);
    assert!(
        checksum
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let codec = get_str(&value, &["data", "codec"]).to_string();
    assert!(codec == "raw" || codec == "zstd-v1", "{codec}");
    // The `data` object has exactly the documented keys.
    let memoria_infrastructure::json::Json::Object(data) = get(&value, &["data"]) else {
        panic!()
    };
    let mut keys: Vec<String> = data.keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "checksum".to_string(),
            "codec".into(),
            "file_bytes".into(),
            "format_version".into(),
            "path".into(),
            "payload_bytes".into(),
            "state".into(),
        ]
    );
    assert_eq!(get_u64(&value, &["data", "state", "schema_version"]), 2);
    assert_eq!(project.tree_snapshot(), before, "inspection wrote nothing");

    // An explicit file works outside Git, without configuration.
    let outside = tempfile::tempdir().unwrap();
    let copy = outside.path().join("copied.lock");
    std::fs::write(&copy, project.state()).unwrap();
    let output = std::process::Command::new(memoria_bin())
        .current_dir(outside.path())
        .args(["state", "inspect", "--file"])
        .arg(&copy)
        .args(["--format", "json"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", project.home.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let explicit = parse_json(&output.stdout);
    assert_eq!(
        get_str(&explicit, &["data", "checksum"]),
        checksum,
        "the same bytes decode to the same framing outside Git"
    );

    // A missing file is `state_missing`, not an invented artifact.
    let (code, missing) = project.json(&["state", "inspect", "--file", "no-such.lock"]);
    assert_eq!(code, 4, "{missing:?}");
    assert_eq!(diagnostic_codes(&missing), vec!["state_missing"]);
    // A symlink is refused rather than followed.
    let target = outside.path().join("secret.lock");
    std::fs::write(&target, project.state()).unwrap();
    let link = project.root.join("linked.lock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (code, refused) = project.json(&["state", "inspect", "--file", "linked.lock"]);
    assert_eq!(code, 4, "{refused:?}");
    // A malformed explicit file reports corruption and repairs nothing.
    let broken = outside.path().join("broken.lock");
    std::fs::write(&broken, b"MML\0\x02\x00\x01x").unwrap();
    let output = std::process::Command::new(memoria_bin())
        .current_dir(outside.path())
        .args(["state", "inspect", "--file"])
        .arg(&broken)
        .args(["--format", "json"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", project.home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["state_corrupt"]
    );
    assert_eq!(std::fs::read(&broken).unwrap(), b"MML\0\x02\x00\x01x");
    // Inspection makes no freshness claim.
    let human = project.run(&["state", "inspect"]);
    assert!(
        stdout(&human).contains("memoria status"),
        "inspection names the freshness command instead of claiming currency"
    );
}

#[test]
fn state_binary_attributes_preserve_clone_bytes() {
    let project = Project::seed();
    // Every file is forced as text with LF endings, and the root rule
    // excludes the state artifact explicitly.
    // Git applies the last matching line, so the specific rule follows the
    // broad one.
    project.write(".gitattributes", "* text eol=lf\n/memoria.lock binary\n");
    project.baseline();
    // The artifact begins with NUL, so Git classifies it as binary.
    assert_eq!(&project.state()[..4], b"MML\0");
    project.commit_all("attributes and state");
    let attributes = project.git(&["check-attr", "text", "--", "memoria.lock"]);
    assert!(
        String::from_utf8_lossy(&attributes.stdout).contains("text: unset"),
        "the root rule marks the artifact binary: {}",
        String::from_utf8_lossy(&attributes.stdout)
    );
    let numstat = project.git(&["diff", "--numstat", "HEAD~1", "HEAD", "--", "memoria.lock"]);
    let numstat = String::from_utf8_lossy(&numstat.stdout).into_owned();
    assert!(
        numstat.is_empty() || numstat.starts_with("-\t-\t"),
        "Git reports no line counts for the artifact: {numstat}"
    );
    // Cloning with newline conversion enabled preserves the exact bytes.
    let committed = project.state();
    let clone_dir = tempfile::tempdir().unwrap();
    let clone_root = clone_dir.path().join("clone");
    let output = std::process::Command::new("git")
        .args(["-c", "core.autocrlf=true", "clone", "-q"])
        .arg(&project.root)
        .arg(&clone_root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        std::fs::read(clone_root.join("memoria.lock")).unwrap(),
        committed,
        "newline conversion never touched the artifact"
    );
    let home = tempfile::tempdir().unwrap();
    let check = std::process::Command::new(memoria_bin())
        .current_dir(&clone_root)
        .args(["check", "--format", "json"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn read_only_commands_never_create_locks_or_rewrite_state() {
    let project = Project::seed();
    project.baseline();
    let state = project.state();
    // The private write lock must not exist after read-only work.
    let lock = project.write_lock();
    if lock.exists() {
        std::fs::remove_file(&lock).unwrap();
    }
    let before = project.tree_snapshot();
    for args in [
        vec!["status"],
        vec!["status", "--summary"],
        vec!["guidance"],
        vec!["guidance", "src/corpus/README.md"],
        vec!["review"],
        vec!["lint"],
        vec!["check"],
        vec!["graph"],
        vec!["state", "inspect"],
        vec!["init"],
        vec!["render", "--dry-run"],
    ] {
        let output = project.run(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stdout(&output)
        );
        assert_eq!(project.state(), state, "{args:?}: state was rewritten");
        assert!(!lock.exists(), "{args:?}: a write lock was created");
        assert!(
            !project.exists(".memoria"),
            "{args:?}: the legacy directory was recreated"
        );
    }
    assert_eq!(project.tree_snapshot(), before, "the worktree is unchanged");
}

#[test]
fn worktree_locks_are_private_and_independent() {
    let project = Project::seed();
    project.baseline();
    // A mutation creates the lock under Git metadata, never in the worktree.
    assert_eq!(
        project
            .json(&[
                "invalidate",
                "all",
                "--reason",
                "Check the private lock path."
            ])
            .0,
        0
    );
    assert!(project.write_lock().exists(), "the private lock exists");
    assert!(
        !project.exists(".memoria/write.lock"),
        "no lock inside the worktree"
    );
    // A linked worktree receives its own lock.
    project.commit_all("state");
    let linked_dir = tempfile::tempdir().unwrap();
    let linked = linked_dir.path().join("linked");
    project.git(&[
        "worktree",
        "add",
        "-q",
        linked.to_str().unwrap(),
        "-b",
        "linked-branch",
    ]);
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(memoria_bin())
        .current_dir(&linked)
        .args(["invalidate", "all", "--reason", "Linked worktree mutation."])
        .args(["--format", "json"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    // The linked worktree's lock lives under its own private Git path.
    let linked_lock = project.git_in(
        &linked,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "memoria/write.lock",
        ],
    );
    let linked_lock = String::from_utf8_lossy(&linked_lock.stdout)
        .trim()
        .to_string();
    assert!(std::path::Path::new(&linked_lock).exists());
    assert_ne!(
        linked_lock,
        project.write_lock().display().to_string(),
        "linked worktrees receive separate locks"
    );
    project.git(&["worktree", "remove", "--force", linked.to_str().unwrap()]);
}

/// Compress with an explicit checksum or dictionary-id choice, so the tests
/// can build frames the frozen profile must reject.
fn compress_with(payload: &[u8], checksum: bool, content_size: bool) -> Vec<u8> {
    use zstd_safe::{CCtx, CParameter};
    let mut context = CCtx::create();
    context
        .set_parameter(CParameter::CompressionLevel(3))
        .unwrap();
    context
        .set_parameter(CParameter::ChecksumFlag(checksum))
        .unwrap();
    context
        .set_parameter(CParameter::ContentSizeFlag(!content_size))
        .unwrap();
    if !content_size {
        context
            .set_pledged_src_size(Some(payload.len() as u64))
            .unwrap();
    }
    let mut out = Vec::with_capacity(zstd_safe::compress_bound(payload.len()) + 64);
    context.compress2(&mut out, payload).unwrap();
    out
}

/// Whether a body decompresses as a valid Zstandard frame, ignoring the
/// frozen Memoria profile.
fn zstd_decompresses(body: &[u8]) -> bool {
    let mut context = zstd_safe::DCtx::create();
    let mut out = Vec::with_capacity(1 << 20);
    context.decompress(&mut out, body).is_ok()
}

#[test]
fn state_v2_rejects_unsafe_compression_frames() {
    // Build a frame around an arbitrary compressed body, so each case
    // reaches the decompression constraints with a valid outer checksum.
    let frame = |codec: u8, declared: u64, body: &[u8]| -> Vec<u8> {
        let mut wire = Vec::new();
        wire.extend_from_slice(&lock_codec::MAGIC);
        wire.push(lock_codec::FORMAT_VERSION);
        wire.push(codec);
        let mut length = declared;
        loop {
            let byte = (length & 0x7f) as u8;
            length >>= 7;
            if length == 0 {
                wire.push(byte);
                break;
            }
            wire.push(byte | 0x80);
        }
        wire.extend_from_slice(body);
        let checksum = memoria_infrastructure::hash::xxh3_128(&wire);
        wire.extend_from_slice(&checksum);
        wire
    };
    let good = frozen("current.lock");
    let decoded = lock_codec::decode(&good).unwrap();
    let payload = lock_codec::encode_payload(&decoded.state).unwrap();
    // The compressed body of the frozen vector, without its own framing.
    let body = &good[6..good.len() - 16 - 1];
    let _ = body;

    // A declared length that disagrees with the frame header.
    let real_body = {
        let mut header_len = 0usize;
        let mut index = 6;
        loop {
            header_len += 1;
            let byte = good[index];
            index += 1;
            if byte < 0x80 {
                break;
            }
        }
        let _ = header_len;
        good[index..good.len() - 16].to_vec()
    };
    assert!(
        lock_codec::decode(&frame(
            lock_codec::CODEC_ZSTD,
            payload.len() as u64 + 1,
            &real_body
        ))
        .is_err(),
        "a wrong declared length was accepted"
    );

    // A body that is not a Zstandard frame at all.
    assert!(lock_codec::decode(&frame(lock_codec::CODEC_ZSTD, 8, b"not zstd")).is_err());

    // A frame that carries its own content checksum is rejected before
    // decompression, even though it decompresses correctly.
    let checksummed = compress_with(&payload, true, false);
    assert!(
        zstd_decompresses(&checksummed),
        "the fixture is a valid Zstandard frame"
    );
    match lock_codec::decode(&frame(
        lock_codec::CODEC_ZSTD,
        payload.len() as u64,
        &checksummed,
    )) {
        Err(lock_codec::LockError::Corrupt(message)) => {
            assert!(message.contains("content checksum"), "{message}");
        }
        other => panic!("a checksummed frame was accepted: {other:?}"),
    }

    // A valid frame followed by an empty frame is two frames, not one.
    let empty_frame = compress_with(b"", false, false);
    let mut concatenated_frames = real_body.clone();
    concatenated_frames.extend_from_slice(&empty_frame);
    match lock_codec::decode(&frame(
        lock_codec::CODEC_ZSTD,
        payload.len() as u64,
        &concatenated_frames,
    )) {
        Err(lock_codec::LockError::Corrupt(message)) => {
            assert!(message.contains("exactly one frame"), "{message}");
        }
        other => panic!("a concatenated empty frame was accepted: {other:?}"),
    }

    // Trailing padding after a complete frame is rejected too.
    let mut padded = real_body.clone();
    padded.extend_from_slice(&[0, 0, 0, 0]);
    assert!(
        lock_codec::decode(&frame(
            lock_codec::CODEC_ZSTD,
            payload.len() as u64,
            &padded
        ))
        .is_err(),
        "trailing padding was accepted"
    );
    // A skippable Zstandard frame is not an ordinary frame.
    let mut skippable = vec![0x50, 0x2a, 0x4d, 0x18, 0x04, 0x00, 0x00, 0x00];
    skippable.extend_from_slice(&[0, 0, 0, 0]);
    match lock_codec::decode(&frame(lock_codec::CODEC_ZSTD, 4, &skippable)) {
        Err(lock_codec::LockError::Corrupt(message)) => {
            assert!(message.contains("ordinary Zstandard frame"), "{message}");
        }
        other => panic!("a skippable frame was accepted: {other:?}"),
    }
    // Concatenated frames: the trailing bytes are rejected.
    let mut concatenated = real_body.clone();
    concatenated.extend_from_slice(&real_body);
    assert!(
        lock_codec::decode(&frame(
            lock_codec::CODEC_ZSTD,
            payload.len() as u64,
            &concatenated
        ))
        .is_err(),
        "concatenated frames were accepted"
    );
    // A raw body whose length disagrees with the declared length.
    assert!(
        lock_codec::decode(&frame(
            lock_codec::CODEC_RAW,
            payload.len() as u64 + 1,
            &payload
        ))
        .is_err()
    );
    // The checksum is verified before decompression: a corrupted compressed
    // body with the original trailer is rejected as corruption, not as a
    // decompression failure.
    let mut tampered = good.clone();
    let middle = 6 + (good.len() - 22) / 2;
    tampered[middle] ^= 0xff;
    match lock_codec::decode(&tampered) {
        Err(lock_codec::LockError::Corrupt(message)) => {
            assert!(message.contains("checksum"), "{message}");
        }
        other => panic!("expected a checksum failure, got {other:?}"),
    }
}

#[test]
fn canonical_v2_vectors_bind_guidance_only_in_review_context() {
    // Guidance changes the token and the stored advisory digest. It never
    // changes the manifest or the policy hash.
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let (packet, token) = project.review_full("src/execution/README.md");
    let first = parse_json(&std::fs::read(&packet).unwrap());
    let manifest = get(&first, &["data", "manifest"]).clone();
    let guidance_before = get_str(&first, &["data", "context", "guidance", "digest"]).to_string();

    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "\"Use short sentences.\",",
            "\"Use very short sentences.\",",
        ),
    );
    let (packet, fresh_token) = project.review_full("src/execution/README.md");
    let second = parse_json(&std::fs::read(&packet).unwrap());
    assert_ne!(
        get_str(&second, &["data", "context", "guidance", "digest"]),
        guidance_before,
        "the guidance digest tracks the wording"
    );
    assert_ne!(token, fresh_token, "the token binds the guidance digest");
    assert_eq!(
        get(&second, &["data", "manifest"]),
        &manifest,
        "guidance never enters the manifest or the policy hash"
    );
}

#[test]
fn the_private_write_lock_refuses_substituted_paths() {
    // The lock lives under Git metadata, outside the worktree. A symlinked
    // directory or lock file must never place it somewhere else.
    for (label, substitute) in [
        ("directory symlink", "dir"),
        ("lock file symlink", "file"),
        ("lock is a directory", "special"),
    ] {
        let project = Project::seed();
        project.baseline();
        let outside = tempfile::tempdir().unwrap();
        let private = project.root.join(".git/memoria");
        let before = project.state();
        match substitute {
            "dir" => {
                let _ = std::fs::remove_dir_all(&private);
                let _ = std::fs::remove_file(&private);
                std::os::unix::fs::symlink(outside.path(), &private).unwrap();
            }
            "file" => {
                std::fs::create_dir_all(&private).unwrap();
                let target = outside.path().join("write.lock");
                std::fs::write(&target, b"").unwrap();
                let link = private.join("write.lock");
                let _ = std::fs::remove_file(&link);
                std::os::unix::fs::symlink(&target, &link).unwrap();
            }
            _ => {
                // A directory where the lock file belongs.
                let lock = private.join("write.lock");
                let _ = std::fs::remove_file(&lock);
                std::fs::create_dir_all(&lock).unwrap();
            }
        }
        // A mutation must refuse rather than lock an outside path.
        let (code, value) = project.json(&[
            "invalidate",
            "all",
            "--reason",
            "This mutation must refuse the substituted lock.",
        ]);
        assert_eq!(code, 4, "{label}: {value:?}");
        assert_eq!(project.state(), before, "{label}: no state was written");
        assert!(
            !outside.path().join("write.lock").exists()
                || std::fs::metadata(outside.path().join("write.lock"))
                    .map(|m| m.len() == 0)
                    .unwrap_or(false),
            "{label}: nothing was written outside the project"
        );
        // Read-only commands still work.
        assert_eq!(project.json(&["status"]).0, 0, "{label}");
    }

    // A clean private path still locks, in an ordinary and a linked worktree.
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        project
            .json(&[
                "invalidate",
                "all",
                "--reason",
                "An ordinary mutation succeeds."
            ])
            .0,
        0
    );
    assert!(project.write_lock().exists());
    project.commit_all("state");
    let linked_dir = tempfile::tempdir().unwrap();
    let linked = linked_dir.path().join("linked");
    project.git(&[
        "worktree",
        "add",
        "-q",
        linked.to_str().unwrap(),
        "-b",
        "lock-branch",
    ]);
    let output = project
        .isolated_command(memoria_bin())
        .current_dir(&linked)
        .args([
            "invalidate",
            "all",
            "--reason",
            "A linked worktree mutation.",
        ])
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    project.git(&["worktree", "remove", "--force", linked.to_str().unwrap()]);
}

/// One review of `document` with a distinctive token and the given evidence.
fn evidence_state(
    document: &str,
    coverage: memoria_domain::CoverageEvidence,
) -> memoria_domain::ReviewState {
    let document = memoria_domain::DocumentId::parse(document).unwrap();
    let mut state = review_state_with(document.clone(), vec![], vec![]);
    let record = state.reviews.get_mut(&document).unwrap();
    record.token_digest = memoria_domain::Hash64(0x5eed_5eed_5eed_5eed);
    record.coverage = coverage;
    state
}

/// The payload offset of the coverage field: right after the token digest
/// and the one-byte acknowledged-invalidations vector index.
fn coverage_offset(payload: &[u8]) -> usize {
    let token = 0x5eed_5eed_5eed_5eedu64.to_be_bytes();
    let starts: Vec<usize> = payload
        .windows(8)
        .enumerate()
        .filter(|(_, window)| *window == token)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(starts.len(), 1, "the token digest occurs once");
    starts[0] + 8 + 1
}

fn folders(raw: &[&str]) -> memoria_domain::CoverageEvidence {
    memoria_domain::CoverageEvidence::Recorded(
        raw.iter()
            .map(|f| memoria_domain::DirPath::parse(f).unwrap())
            .collect(),
    )
}

#[test]
fn state_v3_evidence_vector_is_frozen_and_round_trips() {
    // Recorded nested folders, Recorded(∅), in one committed vector.
    let state = state_vectors::evidence();
    let encoded = lock_codec::encode(&state).unwrap();
    assert_eq!(
        encoded,
        frozen_v3("evidence.lock", &encoded),
        "evidence.lock is frozen"
    );
    assert_eq!(encoded[4], 3);
    let decoded = lock_codec::decode(&encoded).unwrap();
    assert_eq!(lock_codec::encode(&decoded.state).unwrap(), encoded);
    let root = memoria_domain::DirPath::root().readme();
    let guide = memoria_domain::DocumentId::parse("docs/guide.md").unwrap();
    assert_eq!(
        decoded.state.reviews[&root].coverage,
        folders(&["crates", "crates/memoria-domain", "docs"])
    );
    assert_eq!(decoded.state.reviews[&guide].coverage, folders(&[]));
    // Unrecorded, Recorded(∅) and Recorded(X) are three distinct encodings,
    // and each one is its own single canonical byte sequence.
    let mut seen = std::collections::BTreeSet::new();
    for coverage in [
        memoria_domain::CoverageEvidence::Unrecorded,
        folders(&[]),
        folders(&["a"]),
        folders(&["a", "a/b"]),
    ] {
        let state = evidence_state("README.md", coverage.clone());
        let bytes = lock_codec::encode(&state).unwrap();
        let decoded = lock_codec::decode(&bytes).unwrap().state;
        assert_eq!(decoded.reviews.values().next().unwrap().coverage, coverage);
        assert_eq!(lock_codec::encode(&decoded).unwrap(), bytes);
        assert_eq!(lock_codec::encode(&state).unwrap(), bytes);
        assert!(seen.insert(bytes), "{coverage:?} shares bytes");
        let payload = lock_codec::encode_payload(&state).unwrap();
        let tag = payload[coverage_offset(&payload)];
        match coverage {
            memoria_domain::CoverageEvidence::Unrecorded => assert_eq!(tag, 0),
            _ => assert!(tag >= 1),
        }
    }
}

#[test]
fn state_v3_rejects_malformed_coverage_evidence() {
    let corrupt = |bytes: &[u8], label: &str| match lock_codec::decode(bytes) {
        Err(lock_codec::LockError::Corrupt(message)) => message,
        other => panic!("{label}: {other:?}"),
    };
    let state = evidence_state("svc/README.md", folders(&["svc/a", "svc/a/b"]));
    let payload = lock_codec::encode_payload(&state).unwrap();
    assert!(lock_codec::decode(&frame(3, &payload)).is_ok());
    let at = coverage_offset(&payload);

    // A coverage index beyond the vectors table.
    let mut beyond = payload.clone();
    beyond[at] = 0x7f;
    let message = corrupt(&frame(3, &beyond), "index out of range");
    assert!(message.contains("coverage evidence vector"), "{message}");

    // Unrecorded while the evidence vector and its folder rows stay: both
    // are unused table rows.
    let mut unused = payload.clone();
    unused[at] = 0;
    let message = corrupt(&frame(3, &unused), "unused rows");
    assert!(message.contains("never referenced"), "{message}");

    // The same format 3 payload read as format 2 has one byte too many in
    // every review row, so it is rejected, never silently misread.
    assert!(lock_codec::decode(&frame(2, &payload)).is_err());

    // The evidence vector [i, j] follows the empty acknowledged vector in
    // section 6 as `02 00 02 i (j - i)`.
    let marker = payload
        .windows(3)
        .enumerate()
        .filter(|(_, w)| *w == [2, 0, 2])
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(marker.len(), 1, "one vectors section header");
    let first = marker[0] + 3;
    // Ids not strictly increasing: a zero delta repeats a path id.
    let mut repeated = payload.clone();
    repeated[first + 1] = 0;
    let message = corrupt(&frame(3, &repeated), "repeated id");
    assert!(message.contains("repeats a value"), "{message}");
    // A path id beyond the path table.
    let mut far = payload.clone();
    far[first + 1] = 0x7f;
    let message = corrupt(&frame(3, &far), "path id out of range");
    assert!(message.contains("path index"), "{message}");

    // A folder outside the document folder, or the document folder itself,
    // written by a fixture that skips the writer's checks.
    for (outside, label) in [
        (folders(&["other"]), "outside"),
        (folders(&["svc"]), "the document folder itself"),
    ] {
        let state = evidence_state("svc/README.md", outside);
        let bytes = lock_codec::encode_unverified(&state).unwrap();
        let message = corrupt(&bytes, label);
        assert!(
            message.contains("not a folder strictly inside svc/"),
            "{label}: {message}"
        );
        assert!(
            lock_codec::encode(&state).is_err(),
            "{label}: the writer refuses it"
        );
    }
}

#[test]
fn state_inspect_reports_evidence_and_the_old_format() {
    // Human output names the recorded folders, or says it is not recorded;
    // JSON carries `coverage_evidence` as null or the folder list.
    let project = Project::seed();
    project.write("old.lock", frozen("documents.lock"));
    let (code, v2) = project.json(&["state", "inspect", "--file", "old.lock"]);
    assert_eq!(code, 0, "{v2:?}");
    assert_eq!(get_u64(&v2, &["data", "format_version"]), 2);
    assert!(matches!(
        get(
            &v2,
            &["data", "state", "reviews", "README.md", "coverage_evidence"]
        ),
        memoria_infrastructure::json::Json::Null
    ));
    let human = stdout(&project.run(&["state", "inspect", "--file", "old.lock"]));
    assert!(human.contains("coverage evidence: not recorded"), "{human}");

    project.write(
        "new.lock",
        lock_codec::encode(&state_vectors::evidence()).unwrap(),
    );
    let human = stdout(&project.run(&["state", "inspect", "--file", "new.lock"]));
    assert!(
        human.contains("coverage evidence: crates/, crates/memoria-domain/, docs/"),
        "{human}"
    );
    assert!(
        human.contains("coverage evidence: none handed off"),
        "{human}"
    );
    let (code, v3) = project.json(&["state", "inspect", "--file", "new.lock"]);
    assert_eq!(code, 0, "{v3:?}");
    assert_eq!(get_u64(&v3, &["data", "format_version"]), 3);
    assert_eq!(
        strings(get(
            &v3,
            &["data", "state", "reviews", "README.md", "coverage_evidence"]
        )),
        ["crates", "crates/memoria-domain", "docs"]
    );
    assert!(
        strings(get(
            &v3,
            &[
                "data",
                "state",
                "reviews",
                "docs/guide.md",
                "coverage_evidence"
            ]
        ))
        .is_empty()
    );
    // `state diff` reports the evidence difference between the two files.
    let (code, diff) = project.json(&["state", "diff", "old.lock", "new.lock"]);
    assert_eq!(code, 0, "{diff:?}");
    let memoria_infrastructure::json::Json::Array(changes) = get(&diff, &["data", "changes"])
    else {
        panic!()
    };
    let paths: Vec<Vec<String>> = changes.iter().map(|c| strings(get(c, &["path"]))).collect();
    for document in ["README.md", "docs/guide.md"] {
        assert!(
            paths.contains(&vec![
                "reviews".to_string(),
                document.to_string(),
                "coverage_evidence".to_string()
            ]),
            "{paths:?}"
        );
    }
}
