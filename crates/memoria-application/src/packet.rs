//! Review artifacts, their limits, and the deterministic v3 token.

use memoria_domain::canonical;
use memoria_domain::{DocumentId, GitContext, Hash64, InputManifest, ReviewRecord};

use crate::error::{Detail, DetailMap};
use crate::guidance::EffectiveGuidance;
use crate::ports::FingerprintHasher;

/// Full offline exports. Version 2 packets are not accepted: packets are
/// ephemeral, so an old one is regenerated rather than converted.
pub const PACKET_VERSION: u64 = 3;
/// The version of every CLI JSON envelope. The release has one clean
/// cutover: `schema_version: 3` for envelopes, manifests, and packets alike.
pub const ENVELOPE_SCHEMA_VERSION: u64 = 3;
pub const TOKEN_PREFIX: &str = "mrv3.";
pub const TOKEN_LENGTH: usize = 21;
/// Token prefixes this release recognizes but refuses, so the diagnostic can
/// name the release that produced them.
pub const RETIRED_TOKEN_PREFIXES: [&str; 2] = ["mrv1.", "mrv2."];

/// Default raw-input budget: 8 MiB.
pub const DEFAULT_RAW_INPUT_LIMIT: u64 = 8 * 1024 * 1024;
/// Maximum raw-input budget: 32 MiB.
pub const MAX_RAW_INPUT_LIMIT: u64 = 32 * 1024 * 1024;
/// Serialized packet hard cap: 64 MiB.
pub const MAX_SERIALIZED_BYTES: u64 = 64 * 1024 * 1024;
/// Decoded content hard cap: 32 MiB.
pub const MAX_DECODED_BYTES: u64 = 32 * 1024 * 1024;
/// Total array elements across the envelope.
pub const MAX_RECORDS: u64 = 100_000;
/// Simultaneous JSON containers.
pub const MAX_DEPTH: u64 = 32;

/// The measured size of one complete JSON envelope exactly as it would be
/// emitted: every array element, the deepest container, and the serialized
/// byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeSize {
    pub records: u64,
    pub depth: u64,
    pub serialized_bytes: u64,
}

impl EnvelopeSize {
    /// Whether the envelope satisfies every hard output limit.
    pub fn within_hard_limits(&self) -> bool {
        self.records <= MAX_RECORDS
            && self.depth <= MAX_DEPTH
            && self.serialized_bytes <= MAX_SERIALIZED_BYTES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentEncoding {
    Utf8,
    Base64,
}

impl ContentEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentEncoding::Utf8 => "utf8",
            ContentEncoding::Base64 => "base64",
        }
    }

    pub fn for_bytes(bytes: &[u8]) -> ContentEncoding {
        if std::str::from_utf8(bytes).is_ok() {
            ContentEncoding::Utf8
        } else {
            ContentEncoding::Base64
        }
    }
}

/// Content of one owned file or the README.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub path: String,
    pub bytes: u64,
    pub hash: Hash64,
    pub encoding: ContentEncoding,
    pub body: Vec<u8>,
}

/// Content of one imported export body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportContent {
    pub document: String,
    pub export_id: String,
    pub bytes: u64,
    pub hash: Hash64,
    pub encoding: ContentEncoding,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketContent {
    pub readme: FileContent,
    pub files: Vec<FileContent>,
    pub imports: Vec<ImportContent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportEntry {
    pub id: String,
    pub bytes: u64,
    pub hash: Hash64,
    pub consumers: Vec<String>,
}

/// One changed input identity compared with the previous review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    /// `file`, `import`, `document`, or `policy`.
    pub kind: String,
    /// `added`, `removed`, or `changed`.
    pub change: String,
    pub identity: String,
    pub before_bytes: Option<u64>,
    pub before_hash: Option<Hash64>,
    pub after_bytes: Option<u64>,
    pub after_hash: Option<Hash64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub identity: String,
    /// `available`, `unavailable`, `binary`, `too_large`, `added`, or `removed`.
    pub status: String,
    pub reason: Option<String>,
    pub old_encoding: Option<ContentEncoding>,
    pub old_body: Option<Vec<u8>>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketContext {
    pub previous_review: Option<ReviewRecord>,
    pub git: GitContext,
    /// Effective project documentation guidance, before the owned evidence.
    pub guidance: EffectiveGuidance,
    pub exports: Vec<ExportEntry>,
    pub consumers: Vec<String>,
    pub changes: Vec<ChangeEntry>,
    pub diffs: Vec<DiffEntry>,
}

/// The canonical descriptors a full export carries so an offline reader can
/// recompute the token's `C` and `B` without the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketBinding {
    pub context: memoria_domain::ReviewContext,
    /// The prior record used for `B`, repeated here so the binding is
    /// self-sufficient.
    pub baseline: Option<ReviewRecord>,
}

/// The complete snapshot handed to a reviewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedReviewPacket {
    pub document: DocumentId,
    pub review_revision: u64,
    pub manifest: InputManifest,
    /// `(id, exact reason)` sorted by id.
    pub covered_invalidations: Vec<(u64, String)>,
    pub token: String,
    /// Filled by the codec on encode; verified on decode.
    pub packet_digest: String,
    pub content: PacketContent,
    pub context: PacketContext,
    pub raw_input_bytes: u64,
    pub record_count: u64,
    /// The same review requirements a small manifest would state.
    pub requirements: crate::review::ReviewManifest,
    /// Canonical context descriptors for offline token validation.
    pub binding: PacketBinding,
}

/// One decoded acknowledgement-capable artifact.
///
/// Both kinds bind the same snapshot and carry the same token. The small
/// manifest states requirements; the full export adds the bytes. Neither
/// narrows what acknowledgement revalidates against the live repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewArtifact {
    Manifest(Box<crate::review::ReviewManifest>),
    Full(Box<FocusedReviewPacket>),
}

impl ReviewArtifact {
    pub fn document(&self) -> &DocumentId {
        match self {
            ReviewArtifact::Manifest(m) => &m.document,
            ReviewArtifact::Full(p) => &p.document,
        }
    }

    pub fn token(&self) -> &str {
        match self {
            ReviewArtifact::Manifest(m) => &m.token,
            ReviewArtifact::Full(p) => &p.token,
        }
    }

    pub fn review_revision(&self) -> u64 {
        match self {
            ReviewArtifact::Manifest(m) => m.review_revision,
            ReviewArtifact::Full(p) => p.review_revision,
        }
    }

    pub fn covered_invalidations(&self) -> &[(u64, String)] {
        match self {
            ReviewArtifact::Manifest(m) => &m.covered_invalidations,
            ReviewArtifact::Full(p) => &p.covered_invalidations,
        }
    }

    /// The review requirements both representations state.
    pub fn requirements(&self) -> &crate::review::ReviewManifest {
        match self {
            ReviewArtifact::Manifest(m) => m,
            ReviewArtifact::Full(p) => &p.requirements,
        }
    }

    /// The complete reviewed input manifest, when the artifact carries one.
    /// A small manifest deliberately does not copy every unchanged hash.
    pub fn manifest(&self) -> Option<&InputManifest> {
        match self {
            ReviewArtifact::Manifest(_) => None,
            ReviewArtifact::Full(p) => Some(&p.manifest),
        }
    }

    pub fn full(&self) -> Option<&FocusedReviewPacket> {
        match self {
            ReviewArtifact::Manifest(_) => None,
            ReviewArtifact::Full(p) => Some(p),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ReviewArtifact::Manifest(_) => crate::review::MANIFEST_KIND,
            ReviewArtifact::Full(_) => "focused_review",
        }
    }
}

/// Compute the v3 token from the three bound digests.
///
/// `I`, `B`, and `C` are the complete input manifest, the complete prior
/// review record, and the complete review context. The same stable snapshot
/// produces the same token in the small and the full representation.
#[allow(clippy::too_many_arguments)]
pub fn compute_token_v3(
    hasher: &dyn FingerprintHasher,
    document: &DocumentId,
    review_revision: u64,
    inputs_digest: Hash64,
    baseline_digest: Hash64,
    context_digest: Hash64,
    covered: &[(u64, String)],
) -> String {
    let bytes = canonical::encode_review_token_v3(
        document,
        review_revision,
        inputs_digest,
        baseline_digest,
        context_digest,
        covered,
    );
    format!("{TOKEN_PREFIX}{}", hasher.hash(&bytes).to_hex())
}

/// Validate the fixed 21-byte token grammar `^mrv3\.[0-9a-f]{16}$`.
pub fn validate_token_text(token: &str) -> Result<Hash64, String> {
    if token.len() != TOKEN_LENGTH {
        return Err(format!(
            "token must be exactly {TOKEN_LENGTH} ASCII bytes; got {} bytes",
            token.len()
        ));
    }
    if !token.is_ascii() {
        return Err("token must be ASCII".to_string());
    }
    let Some(hex) = token.strip_prefix(TOKEN_PREFIX) else {
        // Old tokens are refused with regeneration instructions. Nothing is
        // converted, reinterpreted, or silently upgraded.
        if RETIRED_TOKEN_PREFIXES
            .iter()
            .any(|prefix| token.starts_with(prefix))
        {
            return Err(format!(
                "token {token:?} comes from an earlier release; run `memoria review` again and use the new {TOKEN_PREFIX} token"
            ));
        }
        return Err(format!("token must start with {TOKEN_PREFIX:?}"));
    };
    Hash64::parse(hex)
        .map_err(|_| "token digest must be 16 lowercase hexadecimal digits".to_string())
}

pub fn manifest_detail(manifest: &InputManifest) -> Detail {
    DetailMap::default()
        .number("version", 2)
        .text("document", manifest.document.as_str())
        .text("policy_hash", manifest.policy_hash.to_hex())
        .number("document_bytes", manifest.document_bytes)
        .text("document_hash", manifest.document_hash.to_hex())
        .with(
            "files",
            Detail::list(manifest.files().iter().map(|f| {
                DetailMap::default()
                    .text("path", f.path.as_str())
                    .number("bytes", f.bytes)
                    .text("hash", f.hash.to_hex())
                    .build()
            })),
        )
        .with(
            "imports",
            Detail::list(manifest.imports().iter().map(|i| {
                DetailMap::default()
                    .text("document", i.document.as_str())
                    .text("export_id", i.export_id.as_str())
                    .number("bytes", i.bytes)
                    .text("hash", i.hash.to_hex())
                    .build()
            })),
        )
        .build()
}

pub fn review_record_detail(record: &ReviewRecord) -> Detail {
    DetailMap::default()
        .number("revision", record.revision)
        .with("input_manifest", manifest_detail(&record.manifest))
        .text("input_fingerprint", record.input_fingerprint.to_hex())
        .text("token_digest", record.token_digest.to_hex())
        .text("guidance_digest", record.guidance.to_hex())
        .text("reviewed_at", record.reviewed_at.0.clone())
        .text("reviewer", record.reviewer.as_str())
        .text("result", record.result.as_str())
        .text("note", record.note.as_str())
        .with(
            "git",
            DetailMap::default()
                .with(
                    "base_commit",
                    Detail::option_text(record.git.base_commit.clone()),
                )
                .bool("worktree_dirty", record.git.worktree_dirty)
                .build(),
        )
        .with(
            "acknowledged_invalidations",
            Detail::list(
                record
                    .acknowledged_invalidations
                    .iter()
                    .map(|id| Detail::Number(*id)),
            ),
        )
        .build()
}

pub(crate) fn body_detail(encoding: ContentEncoding, body: &[u8]) -> Detail {
    match encoding {
        ContentEncoding::Utf8 => Detail::Text(String::from_utf8_lossy(body).into_owned()),
        ContentEncoding::Base64 => Detail::Text(base64_encode(body)),
    }
}

/// Standard base64 with padding, kept here so DTO conversion needs no adapter.
pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 63] as char);
        out.push(TABLE[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

impl FocusedReviewPacket {
    /// The `data` object of the review envelope.
    pub fn to_detail(&self) -> Detail {
        let file_content = |f: &FileContent| {
            DetailMap::default()
                .text("path", f.path.clone())
                .number("bytes", f.bytes)
                .text("hash", f.hash.to_hex())
                .text("encoding", f.encoding.as_str())
                .with("body", body_detail(f.encoding, &f.body))
                .build()
        };
        let context = &self.context;
        DetailMap::default()
            .text("kind", "focused_review")
            .number("packet_version", PACKET_VERSION)
            .text("document", self.document.as_str())
            .number("review_revision", self.review_revision)
            .with("manifest", manifest_detail(&self.manifest))
            .with(
                "covered_invalidations",
                Detail::list(self.covered_invalidations.iter().map(|(id, reason)| {
                    DetailMap::default()
                        .number("id", *id)
                        .text("reason", reason.clone())
                        .build()
                })),
            )
            .text("token", self.token.clone())
            .text("packet_digest", self.packet_digest.clone())
            .with(
                "content",
                DetailMap::default()
                    .with("readme", file_content(&self.content.readme))
                    .with(
                        "files",
                        Detail::list(self.content.files.iter().map(file_content)),
                    )
                    .with(
                        "imports",
                        Detail::list(self.content.imports.iter().map(|i| {
                            DetailMap::default()
                                .text("document", i.document.clone())
                                .text("export_id", i.export_id.clone())
                                .number("bytes", i.bytes)
                                .text("hash", i.hash.to_hex())
                                .text("encoding", i.encoding.as_str())
                                .with("body", body_detail(i.encoding, &i.body))
                                .build()
                        })),
                    )
                    .build(),
            )
            .with(
                "context",
                DetailMap::default()
                    .with(
                        "previous_review",
                        context
                            .previous_review
                            .as_ref()
                            .map(review_record_detail)
                            .unwrap_or(Detail::Null),
                    )
                    .with(
                        "git",
                        DetailMap::default()
                            .with(
                                "base_commit",
                                Detail::option_text(context.git.base_commit.clone()),
                            )
                            .bool("worktree_dirty", context.git.worktree_dirty)
                            .build(),
                    )
                    .with("guidance", context.guidance.to_detail())
                    .with(
                        "exports",
                        Detail::list(context.exports.iter().map(|e| {
                            DetailMap::default()
                                .text("id", e.id.clone())
                                .number("bytes", e.bytes)
                                .text("hash", e.hash.to_hex())
                                .with("consumers", Detail::texts(e.consumers.clone()))
                                .build()
                        })),
                    )
                    .with("consumers", Detail::texts(context.consumers.clone()))
                    .with(
                        "changes",
                        Detail::list(context.changes.iter().map(|c| {
                            DetailMap::default()
                                .text("kind", c.kind.clone())
                                .text("change", c.change.clone())
                                .text("identity", c.identity.clone())
                                .with(
                                    "before_bytes",
                                    c.before_bytes.map(Detail::Number).unwrap_or(Detail::Null),
                                )
                                .with(
                                    "before_hash",
                                    Detail::option_text(c.before_hash.map(|h| h.to_hex())),
                                )
                                .with(
                                    "after_bytes",
                                    c.after_bytes.map(Detail::Number).unwrap_or(Detail::Null),
                                )
                                .with(
                                    "after_hash",
                                    Detail::option_text(c.after_hash.map(|h| h.to_hex())),
                                )
                                .build()
                        })),
                    )
                    .with(
                        "diffs",
                        Detail::list(context.diffs.iter().map(|d| {
                            DetailMap::default()
                                .text("identity", d.identity.clone())
                                .text("status", d.status.clone())
                                .with("reason", Detail::option_text(d.reason.clone()))
                                .with(
                                    "old_encoding",
                                    Detail::option_text(
                                        d.old_encoding.map(|e| e.as_str().to_string()),
                                    ),
                                )
                                .with(
                                    "old_body",
                                    match (&d.old_body, d.old_encoding) {
                                        (Some(body), Some(encoding)) => body_detail(encoding, body),
                                        _ => Detail::Null,
                                    },
                                )
                                .with("text", Detail::option_text(d.text.clone()))
                                .build()
                        })),
                    )
                    .build(),
            )
            .with(
                // The complete manifest-v1 data object, minus its own
                // artifact digest: the full `packet_digest` already covers it.
                "requirements",
                self.requirements.requirements_detail(),
            )
            .with(
                "binding",
                DetailMap::default()
                    .with(
                        "context",
                        crate::review_context::context_detail(&self.binding.context),
                    )
                    .with(
                        "baseline",
                        self.binding
                            .baseline
                            .as_ref()
                            .map(review_record_detail)
                            .unwrap_or(Detail::Null),
                    )
                    .build(),
            )
            .with(
                "size",
                DetailMap::default()
                    .number("raw_input_bytes", self.raw_input_bytes)
                    .number("record_count", self.record_count)
                    .build(),
            )
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_grammar() {
        assert!(validate_token_text("mrv3.ef46db3751d8e999").is_ok());
        assert!(validate_token_text("mrv3.EF46DB3751D8E999").is_err());
        assert!(validate_token_text("mrv3.ef46db3751d8e99").is_err());
        assert!(validate_token_text("mrv3.ef46db3751d8e9999").is_err());
        assert!(validate_token_text("mrv3.ef46db3751d8e99 ").is_err());
        assert!(validate_token_text("mrv3.ef46db3751d8e99é").is_err());
        assert!(validate_token_text(&"a".repeat(256)).is_err());
        assert!(validate_token_text(&"a".repeat(257)).is_err());
    }

    #[test]
    fn retired_tokens_ask_for_a_fresh_review() {
        // Old tokens are refused with regeneration instructions, never
        // converted and never silently upgraded.
        for retired in ["mrv1.ef46db3751d8e999", "mrv2.ef46db3751d8e999"] {
            let message = validate_token_text(retired).unwrap_err();
            assert!(message.contains("earlier release"), "{message}");
            assert!(message.contains("memoria review"), "{message}");
        }
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0x00, 0xfe]), "/wD+");
    }
}
