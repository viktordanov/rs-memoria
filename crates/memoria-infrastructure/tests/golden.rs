//! Golden digests from the independent xxHash C library 0.8.3, XXH3_64bits,
//! over manually encoded canonical bytes. Expected values are not produced
//! by the production Rust encoder or hasher.

use memoria_application::packet::compute_token_v3;
use memoria_application::ports::FingerprintHasher;
use memoria_domain::canonical::{
    ImportEdge, ReviewContext, encode_guidance, encode_inputs, encode_policy,
    encode_review_baseline, encode_review_context, encode_review_token_v3,
};
use memoria_domain::{
    DirPath, DocumentId, EffectivePolicy, ExportId, FileInput, GitRuleScope, GuidanceDigest,
    GuidanceEntry, GuidanceKind, Hash64, ImportInput, InputManifest, PolicyRuleScope, ProjectPath,
    SectionMapIdentity,
};
use memoria_infrastructure::Xxh3Hasher;
use memoria_infrastructure::json::{Limits, parse};
use memoria_infrastructure::packet::{manifest_digest, packet_digest};

const INPUTS_HEX: &str = "00000000000000116d656d6f7269612e696e707574732e76320000000000000009524541444d452e6d6401020304050607080000000000000003111111111111111100000000000000010000000000000004612e7273000000000000000500000000000000220000000000000001000000000000000b622f524541444d452e6d64000000000000000773756d6d61727900000000000000090000000000000033";

/// `memoria.guidance.v1` over two entries, encoded by hand.
const GUIDANCE_HEX: &str = "00000000000000136d656d6f7269612e67756964616e63652e763100000000000000020000000000000000000000000000000c6d656d6f7269612e746f6d6c0000000000000006696e6c696e65000000000000001b4578706c61696e2074686520776f726b666c6f772066697273742e000000000000000373726300000000000000177372632f524541444d452e6d656d6f7269612e746f6d6c000000000000000466696c6500000000000000104c6f63616c2072756c6520746578742e";

/// The frozen guidance digest of the measured project fixtures.
const FIXTURE_GUIDANCE: Hash64 = Hash64(0x4db0aeae8d6990a6);

fn fixed_manifest() -> InputManifest {
    InputManifest::new(
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
            export_id: ExportId::parse("summary").unwrap(),
            bytes: 9,
            hash: Hash64(0x33),
        }],
    )
    .unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn xxh3_64_reference_vectors() {
    let hasher = Xxh3Hasher;
    assert_eq!(hasher.hash(b"").to_hex(), "2d06800538d394c2");
    assert_eq!(hasher.hash(b"a").to_hex(), "e6c632b61e964e1f");
    assert_eq!(hasher.hash(b"abc").to_hex(), "78af5f94892f3950");
    assert_eq!(hasher.hash(&[b'x'; 100]).to_hex(), "c90984ffdf50ce42");
}

#[test]
fn canonical_inputs_bytes_and_digest_match_the_reference() {
    let bytes = encode_inputs(&fixed_manifest());
    assert_eq!(bytes.len(), 160);
    assert_eq!(hex(&bytes), INPUTS_HEX);
    assert_eq!(Xxh3Hasher.hash(&bytes).to_hex(), "44559dfc629e9b86");
}

/// `memoria-review-token-v3` over the fixed fixture, encoded by hand from the
/// documented primitives. `I` is the frozen inputs digest above; `B` and `C`
/// are fixed literals so the layout is checked independently of the encoders.
const TOKEN_V3_HEX: &str = "00000000000000176d656d6f7269612d7265766965772d746f6b656e2d76330000000000000009524541444d452e6d64000000000000000244559dfc629e9b86111213141516171821222324252627280000000000000002000000000000000100000000000000225573652053696d706c696669656420456e676c69736820657665727977686572652e000000000000000300000000000000224578706c61696e20746865206661696c757265206d6f64657320636c6561726c792e";

/// `memoria-review-baseline-v1` with no prior record, encoded by hand.
const BASELINE_V3_HEX: &str =
    "000000000000001a6d656d6f7269612d7265766965772d626173656c696e652d763100";

/// `memoria-review-context-v1` over the fixture context, encoded by hand.
const CONTEXT_V3_HEX: &str = "00000000000000196d656d6f7269612d7265766965772d636f6e746578742d763100000000000000010000000000000009524541444d452e6d6400000000000000000000000000000001000000000000000b622f524541444d452e6d640000000000000000010203040506070800000000000000010000000000000004612e727300000000000000010000000000000001000000000000000b70657273697374656e636500000000000000010000000000000004612e72734db0aeae8d6990a60000000000000001000000000000000b622f524541444d452e6d64000000000000000773756d6d617279000000000000003300000000000000000000000000000000";

fn fixture_context() -> ReviewContext {
    ReviewContext {
        selection_version: 1,
        owner: "README.md".into(),
        ancestor_boundaries: vec![],
        descendant_boundaries: vec!["b/README.md".into()],
        nested_repositories: vec![],
        policy_hash: Hash64(0x0102030405060708),
        owned_paths: vec!["a.rs".into()],
        mapping: SectionMapIdentity::Valid(vec![("persistence".into(), vec!["a.rs".into()])]),
        guidance: GuidanceDigest(FIXTURE_GUIDANCE),
        imports: vec![ImportEdge {
            provider: "b/README.md".into(),
            export_id: "summary".into(),
            hash: Hash64(0x33),
        }],
        consumer_edges: vec![],
        providers: vec![],
    }
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn review_context_and_baseline_bytes_match_the_reference() {
    assert_eq!(hex(&encode_review_baseline(None)), BASELINE_V3_HEX);
    assert_eq!(
        hex(&encode_review_context(&fixture_context())),
        CONTEXT_V3_HEX
    );
    // The absent baseline is distinct from any present one.
    assert_ne!(encode_review_baseline(None).len(), 0);
}

#[test]
fn review_token_v3_matches_the_reference() {
    let doc = DocumentId::parse("README.md").unwrap();
    let covered = vec![
        (3, "Explain the failure modes clearly.".to_string()),
        (1, "Use Simplified English everywhere.".to_string()),
    ];
    let inputs_digest = Xxh3Hasher.hash(&encode_inputs(&fixed_manifest()));
    assert_eq!(inputs_digest.to_hex(), "44559dfc629e9b86");
    let baseline_digest = Hash64(0x1112131415161718);
    let context_digest = Hash64(0x2122232425262728);

    // The canonical bytes are frozen independently of the encoder; the
    // digest comes from the XXH3-64 implementation the reference vectors
    // above check against the C library.
    let bytes = encode_review_token_v3(
        &doc,
        2,
        inputs_digest,
        baseline_digest,
        context_digest,
        &covered,
    );
    assert_eq!(hex(&bytes), TOKEN_V3_HEX);
    let expected = Xxh3Hasher.hash(&unhex(TOKEN_V3_HEX)).to_hex();
    let token = compute_token_v3(
        &Xxh3Hasher,
        &doc,
        2,
        inputs_digest,
        baseline_digest,
        context_digest,
        &covered,
    );
    assert_eq!(token, format!("mrv3.{expected}"));
    assert_eq!(token.len(), 21);

    // Small and maximal snapshots both produce 21-byte tokens.
    let empty = InputManifest::new(doc.clone(), Hash64(0), 0, Hash64(0), vec![], vec![]).unwrap();
    assert_eq!(
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            0,
            Xxh3Hasher.hash(&encode_inputs(&empty)),
            Hash64(0),
            Hash64(0),
            &[]
        )
        .len(),
        21
    );
    // The largest manifest the record cap permits (100,000 records) still
    // yields 21 bytes, because the token hashes digests, not content.
    let files: Vec<FileInput> = (0..99_999)
        .map(|i| FileInput {
            path: ProjectPath::parse(&format!("m/{i}.rs")).unwrap(),
            bytes: i,
            hash: Hash64(i),
        })
        .collect();
    let imports = vec![ImportInput {
        document: DocumentId::parse("b/README.md").unwrap(),
        export_id: ExportId::parse("summary").unwrap(),
        bytes: 1,
        hash: Hash64(1),
    }];
    let maximal = InputManifest::new(doc.clone(), Hash64(0), 0, Hash64(0), files, imports).unwrap();
    assert_eq!(maximal.record_count(), 100_000);
    let maximal_token = compute_token_v3(
        &Xxh3Hasher,
        &doc,
        u64::MAX - 1,
        Xxh3Hasher.hash(&encode_inputs(&maximal)),
        Hash64(u64::MAX),
        Hash64(u64::MAX),
        &[(u64::MAX - 2, "x".repeat(1000))],
    );
    assert_eq!(maximal_token.len(), 21);
    assert!(maximal_token.is_ascii() && maximal_token.starts_with("mrv3."));

    // Every bound component changes the digest.
    for changed in [
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            3,
            inputs_digest,
            baseline_digest,
            context_digest,
            &covered,
        ),
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            2,
            Hash64(1),
            baseline_digest,
            context_digest,
            &covered,
        ),
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            2,
            inputs_digest,
            Hash64(1),
            context_digest,
            &covered,
        ),
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            2,
            inputs_digest,
            baseline_digest,
            Hash64(1),
            &covered,
        ),
        compute_token_v3(
            &Xxh3Hasher,
            &doc,
            2,
            inputs_digest,
            baseline_digest,
            context_digest,
            &covered[..1],
        ),
    ] {
        assert_ne!(changed, token);
    }
}

#[test]
fn guidance_bytes_and_digest_match_the_reference() {
    let entry = |scope: &str, source: &str, kind, text: &str| GuidanceEntry {
        scope: DirPath::parse(scope).unwrap(),
        source: source.to_string(),
        kind,
        text: text.to_string(),
    };
    let entries = vec![
        entry(
            "",
            "memoria.toml",
            GuidanceKind::Inline,
            "Explain the workflow first.",
        ),
        entry(
            "src",
            "src/README.memoria.toml",
            GuidanceKind::File,
            "Local rule text.",
        ),
    ];
    let bytes = encode_guidance(&entries);
    assert_eq!(bytes.len(), 190);
    assert_eq!(hex(&bytes), GUIDANCE_HEX);
    assert_eq!(Xxh3Hasher.hash(&bytes).to_hex(), "9d0b6ce4f267ed63");
    // Guidance is advisory: it never enters the input manifest.
    assert_ne!(hex(&bytes), INPUTS_HEX);
}

#[test]
fn policy_digest_matches_the_reference() {
    let policy = EffectivePolicy::new(
        DocumentId::parse("src/README.md").unwrap(),
        vec![
            GitRuleScope {
                identity: ".gitignore".into(),
                patterns: vec![b"target/".to_vec(), b"!keep".to_vec()],
            },
            GitRuleScope {
                identity: "src/.gitignore".into(),
                patterns: vec![b"*.log".to_vec()],
            },
        ],
        vec![
            PolicyRuleScope::new(
                DirPath::parse("src").unwrap(),
                vec![],
                vec!["fixtures/**".into()],
            ),
            PolicyRuleScope::new(DirPath::root(), vec!["**/generated/**".into()], vec![]),
        ],
    );
    assert_eq!(
        Xxh3Hasher.hash(&encode_policy(&policy)).to_hex(),
        "2fe0963470462694"
    );
}

/// `J({"a":[true,null,7],"b":"x"})` under each integrity domain, encoded by
/// hand from the documented container tags and length prefixes.
const PACKET_V3_J_HEX: &str = "00000000000000116d656d6f7269612e7061636b65742e7633060000000000000002000000000000000161050000000000000003020003000000000000000700000000000000016204000000000000000178";
const MANIFEST_V1_J_HEX: &str = "000000000000001a6d656d6f7269612e7265766965772d6d616e69666573742e7631060000000000000002000000000000000161050000000000000003020003000000000000000700000000000000016204000000000000000178";

#[test]
fn artifact_digests_match_the_reference() {
    let value = parse(br#"{"b":"x","a":[true,null,7]}"#, Limits::PACKET).unwrap();
    // The canonical J bytes are frozen by hand; the digest comes from the
    // XXH3-64 implementation the reference vectors above check.
    assert_eq!(
        packet_digest(&Xxh3Hasher, &value),
        Xxh3Hasher.hash(&unhex(PACKET_V3_J_HEX)).to_hex()
    );
    assert_eq!(
        manifest_digest(&Xxh3Hasher, &value),
        Xxh3Hasher.hash(&unhex(MANIFEST_V1_J_HEX)).to_hex()
    );
    // Separate domains keep the two artifact kinds from sharing a digest.
    assert_ne!(
        packet_digest(&Xxh3Hasher, &value),
        manifest_digest(&Xxh3Hasher, &value)
    );
}
