//! Golden canonical bytes and digests computed by an independent pure-Python
//! xxHash64 reference implementation (see the implementation ledger). None of
//! these expected values were produced by the production encoder.

use memoria_application::packet::compute_token;
use memoria_application::ports::FingerprintHasher;
use memoria_domain::canonical::{encode_inputs, encode_policy, encode_review_token};
use memoria_domain::{
    DirPath, DocumentId, EffectivePolicy, ExportId, FileInput, GitRuleScope, Hash64, ImportInput,
    InputManifest, PolicyRuleScope, ProjectPath,
};
use memoria_infrastructure::Xxh64Hasher;
use memoria_infrastructure::json::{Limits, parse};
use memoria_infrastructure::packet::packet_digest;

const INPUTS_HEX: &str = "00000000000000116d656d6f7269612e696e707574732e76310000000000000009524541444d452e6d6401020304050607080000000000000003111111111111111100000000000000010000000000000004612e7273000000000000000500000000000000220000000000000001000000000000000b622f524541444d452e6d64000000000000000773756d6d61727900000000000000090000000000000033";

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
fn published_xxh64_vectors() {
    let hasher = Xxh64Hasher;
    assert_eq!(hasher.hash(b"").to_hex(), "ef46db3751d8e999");
    assert_eq!(hasher.hash(b"a").to_hex(), "d24ec4f1a98c6e5b");
    assert_eq!(hasher.hash(b"abc").to_hex(), "44bc2cf5ad770999");
    assert_eq!(hasher.hash(&[b'x'; 100]).to_hex(), "92f0de5a88a3c094");
}

#[test]
fn canonical_inputs_bytes_and_digest_match_the_reference() {
    let bytes = encode_inputs(&fixed_manifest());
    assert_eq!(bytes.len(), 160);
    assert_eq!(hex(&bytes), INPUTS_HEX);
    assert_eq!(Xxh64Hasher.hash(&bytes).to_hex(), "ce65094913480e2e");
}

#[test]
fn review_token_matches_the_reference() {
    let manifest = fixed_manifest();
    let doc = DocumentId::parse("README.md").unwrap();
    let covered = vec![
        (3, "Explain the failure modes clearly.".to_string()),
        (1, "Use Simplified English everywhere.".to_string()),
    ];
    let bytes = encode_review_token(&doc, 2, &manifest, &covered);
    assert_eq!(Xxh64Hasher.hash(&bytes).to_hex(), "f44c810a61601b86");
    let token = compute_token(&Xxh64Hasher, &doc, 2, &manifest, &covered);
    assert_eq!(token, "mrv1.f44c810a61601b86");
    assert_eq!(token.len(), 21);
    // Small and maximal manifests both produce 21-byte tokens.
    let empty = InputManifest::new(doc.clone(), Hash64(0), 0, Hash64(0), vec![], vec![]).unwrap();
    assert_eq!(compute_token(&Xxh64Hasher, &doc, 0, &empty, &[]).len(), 21);
    let files: Vec<FileInput> = (0..5000)
        .map(|i| FileInput {
            path: ProjectPath::parse(&format!("f{i}.rs")).unwrap(),
            bytes: i,
            hash: Hash64(i),
        })
        .collect();
    let large = InputManifest::new(
        doc.clone(),
        Hash64(u64::MAX),
        u64::MAX,
        Hash64(u64::MAX),
        files,
        vec![],
    )
    .unwrap();
    assert_eq!(
        compute_token(&Xxh64Hasher, &doc, u64::MAX, &large, &[]).len(),
        21
    );
    // The largest manifest the packet record cap permits (100,000 records) still yields 21 bytes.
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
    let maximal_token = compute_token(
        &Xxh64Hasher,
        &doc,
        u64::MAX - 1,
        &maximal,
        &[(u64::MAX - 2, "x".repeat(1000))],
    );
    assert_eq!(maximal_token.len(), 21);
    assert!(maximal_token.is_ascii() && maximal_token.starts_with("mrv1."));
    // Different revisions and covered sets change the digest.
    assert_ne!(
        compute_token(&Xxh64Hasher, &doc, 3, &manifest, &covered),
        token
    );
    assert_ne!(
        compute_token(&Xxh64Hasher, &doc, 2, &manifest, &covered[..1]),
        token
    );
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
                identity: "global".into(),
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
        Xxh64Hasher.hash(&encode_policy(&policy)).to_hex(),
        "e229c70cda829f1b"
    );
}

#[test]
fn packet_digest_matches_the_reference() {
    let value = parse(br#"{"b":"x","a":[true,null,7]}"#, Limits::PACKET).unwrap();
    assert_eq!(packet_digest(&Xxh64Hasher, &value), "a984d359db157f42");
}
