//! Golden digests from the independent xxHash C library 0.8.3, XXH3_64bits,
//! over manually encoded canonical bytes. Expected values are not produced
//! by the production Rust encoder or hasher.

use memoria_application::packet::compute_token;
use memoria_application::ports::FingerprintHasher;
use memoria_domain::canonical::{encode_inputs, encode_policy, encode_review_token};
use memoria_domain::{
    DirPath, DocumentId, EffectivePolicy, ExportId, FileInput, GitRuleScope, Hash64, ImportInput,
    InputManifest, PolicyRuleScope, ProjectPath,
};
use memoria_infrastructure::Xxh3Hasher;
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
    assert_eq!(Xxh3Hasher.hash(&bytes).to_hex(), "4186e94d17acc21f");
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
    assert_eq!(Xxh3Hasher.hash(&bytes).to_hex(), "a5553af69e6fde75");
    let token = compute_token(&Xxh3Hasher, &doc, 2, &manifest, &covered);
    assert_eq!(token, "mrv1.a5553af69e6fde75");
    assert_eq!(token.len(), 21);
    // Small and maximal manifests both produce 21-byte tokens.
    let empty = InputManifest::new(doc.clone(), Hash64(0), 0, Hash64(0), vec![], vec![]).unwrap();
    assert_eq!(compute_token(&Xxh3Hasher, &doc, 0, &empty, &[]).len(), 21);
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
        compute_token(&Xxh3Hasher, &doc, u64::MAX, &large, &[]).len(),
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
        &Xxh3Hasher,
        &doc,
        u64::MAX - 1,
        &maximal,
        &[(u64::MAX - 2, "x".repeat(1000))],
    );
    assert_eq!(maximal_token.len(), 21);
    assert!(maximal_token.is_ascii() && maximal_token.starts_with("mrv1."));
    // Different revisions and covered sets change the digest.
    assert_ne!(
        compute_token(&Xxh3Hasher, &doc, 3, &manifest, &covered),
        token
    );
    assert_ne!(
        compute_token(&Xxh3Hasher, &doc, 2, &manifest, &covered[..1]),
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
        Xxh3Hasher.hash(&encode_policy(&policy)).to_hex(),
        "979842422fc80b8b"
    );
}

#[test]
fn packet_digest_matches_the_reference() {
    let value = parse(br#"{"b":"x","a":[true,null,7]}"#, Limits::PACKET).unwrap();
    assert_eq!(packet_digest(&Xxh3Hasher, &value), "463e3c32ba062a9d");
}
