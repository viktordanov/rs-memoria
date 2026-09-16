//! Bounded packet transport and strict envelope codec with integrity digest.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use base64::Engine as _;
use memoria_application::error::{Detail, Diagnostic};
use memoria_application::guidance::EffectiveGuidance;
use memoria_application::packet::{
    ChangeEntry, ContentEncoding, DiffEntry, ENVELOPE_SCHEMA_VERSION, EnvelopeSize, ExportEntry,
    FileContent, FocusedReviewPacket, ImportContent, MAX_DECODED_BYTES, MAX_DEPTH, MAX_RECORDS,
    MAX_SERIALIZED_BYTES, PACKET_VERSION, PacketBinding, PacketContent, PacketContext,
    ReviewArtifact,
};
use memoria_application::ports::{
    AdapterError, FileKind, FingerprintHasher, HashStream, PacketFailure, PacketInput,
    PacketSource, ReviewPacketCodec,
};
use memoria_application::review::{
    BaselineInfo, EvidenceStatus, FallbackReason, GuidanceReference, InputEntry, InputRole,
    MANIFEST_KIND, MANIFEST_VERSION, ReviewManifest, ReviewMode, SectionSuggestion,
    SnapshotDigests, WORKFLOW_STEPS,
};
use memoria_domain::canonical::{
    ConsumerEdge, ImportEdge, PACKET_V3_DOMAIN, ProviderDescriptor, REVIEW_MANIFEST_DOMAIN,
    ReviewContext,
};
use memoria_domain::section::SELECTION_VERSION;
use memoria_domain::{
    DirPath, DocumentId, GitContext, GuidanceDigest, GuidanceEntry, GuidanceKind, Hash64,
    ProjectPath, SectionMapIdentity,
};

use crate::fs::{kind_of, read_bounded};
use crate::json::{self, Json, JsonError, Limits, ObjectReader, expect_string, from_detail};
use crate::state::{manifest_from_json, record_from_json};

/// Reads packets from a regular file or stdin with the serialized byte cap.
#[derive(Debug, Default)]
pub struct FsPacketInput;

fn limit_error() -> PacketFailure {
    PacketFailure::Invalid {
        code: "packet_limit_exceeded",
        message: format!("the packet exceeds the serialized cap of {MAX_SERIALIZED_BYTES} bytes"),
    }
}

impl PacketInput for FsPacketInput {
    fn read(&self, source: &PacketSource) -> Result<Vec<u8>, PacketFailure> {
        match source {
            PacketSource::Stdin => {
                let stdin = io::stdin();
                let mut lock = stdin.lock();
                match read_bounded(&mut lock, MAX_SERIALIZED_BYTES) {
                    Ok(Ok(bytes)) => Ok(bytes),
                    Ok(Err(_)) => Err(limit_error()),
                    Err(err) => Err(PacketFailure::Io(AdapterError::new(
                        "read",
                        Some("<stdin>".into()),
                        err.to_string(),
                    ))),
                }
            }
            PacketSource::File(path) => {
                let path = PathBuf::from(path);
                match kind_of(&path) {
                    Ok(FileKind::Regular) => {}
                    Ok(FileKind::Missing) => {
                        return Err(PacketFailure::Io(AdapterError::new(
                            "open",
                            Some(path.display().to_string()),
                            "packet file does not exist",
                        )));
                    }
                    Ok(other) => {
                        return Err(PacketFailure::Invalid {
                            code: "packet_source_invalid",
                            message: format!(
                                "--packet must name a regular file, found {other:?}: {}",
                                path.display()
                            ),
                        });
                    }
                    Err(err) => {
                        return Err(PacketFailure::Io(AdapterError::new(
                            "stat",
                            Some(path.display().to_string()),
                            err.to_string(),
                        )));
                    }
                }
                let mut file = std::fs::File::open(&path).map_err(|e| {
                    PacketFailure::Io(AdapterError::new(
                        "open",
                        Some(path.display().to_string()),
                        e.to_string(),
                    ))
                })?;
                match read_bounded(&mut file, MAX_SERIALIZED_BYTES) {
                    Ok(Ok(bytes)) => Ok(bytes),
                    Ok(Err(_)) => Err(limit_error()),
                    Err(err) => Err(PacketFailure::Io(AdapterError::new(
                        "read",
                        Some(path.display().to_string()),
                        err.to_string(),
                    ))),
                }
            }
        }
    }
}

/// Canonical integrity encoding `J`, streamed into the hasher.
pub fn j_encode(value: &Json, sink: &mut dyn HashStream) {
    fn u64_bytes(sink: &mut dyn HashStream, value: u64) {
        sink.update(&value.to_be_bytes());
    }
    fn str_bytes(sink: &mut dyn HashStream, value: &str) {
        u64_bytes(sink, value.len() as u64);
        sink.update(value.as_bytes());
    }
    match value {
        Json::Null => sink.update(&[0]),
        Json::Bool(false) => sink.update(&[1]),
        Json::Bool(true) => sink.update(&[2]),
        Json::Number(n) => {
            sink.update(&[3]);
            u64_bytes(sink, *n);
        }
        Json::String(s) => {
            sink.update(&[4]);
            str_bytes(sink, s);
        }
        Json::Array(items) => {
            sink.update(&[5]);
            u64_bytes(sink, items.len() as u64);
            for item in items {
                j_encode(item, sink);
            }
        }
        Json::Object(map) => {
            sink.update(&[6]);
            u64_bytes(sink, map.len() as u64);
            for (key, item) in map {
                str_bytes(sink, key);
                j_encode(item, sink);
            }
        }
    }
}

/// `hex(H(C(domain) || J(envelope)))`.
fn digest_with(
    hasher: &dyn FingerprintHasher,
    domain: &str,
    envelope_without_digest: &Json,
) -> String {
    let mut sink = hasher.stream();
    sink.update(&(domain.len() as u64).to_be_bytes());
    sink.update(domain.as_bytes());
    j_encode(envelope_without_digest, &mut *sink);
    sink.finish().to_hex()
}

/// Integrity digest of a full v3 export.
pub fn packet_digest(hasher: &dyn FingerprintHasher, envelope_without_digest: &Json) -> String {
    digest_with(hasher, PACKET_V3_DOMAIN, envelope_without_digest)
}

/// Integrity digest of a small review manifest.
///
/// Both digests detect accidental modification. Neither is a signature, and
/// neither proves who authored the artifact.
pub fn manifest_digest(hasher: &dyn FingerprintHasher, envelope_without_digest: &Json) -> String {
    digest_with(hasher, REVIEW_MANIFEST_DOMAIN, envelope_without_digest)
}

pub fn diagnostic_to_json(diagnostic: &Diagnostic) -> Json {
    let mut map = BTreeMap::new();
    map.insert("code".into(), Json::String(diagnostic.code.clone()));
    map.insert(
        "severity".into(),
        Json::String(diagnostic.severity.as_str().into()),
    );
    map.insert("message".into(), Json::String(diagnostic.message.clone()));
    map.insert(
        "path".into(),
        diagnostic
            .path
            .clone()
            .map(Json::String)
            .unwrap_or(Json::Null),
    );
    map.insert(
        "line".into(),
        diagnostic
            .line
            .map(|l| Json::Number(l as u64))
            .unwrap_or(Json::Null),
    );
    map.insert(
        "column".into(),
        diagnostic
            .column
            .map(|c| Json::Number(c as u64))
            .unwrap_or(Json::Null),
    );
    map.insert("details".into(), from_detail(&diagnostic.details));
    Json::Object(map)
}

pub struct JsonPacketCodec<'a> {
    hasher: &'a dyn FingerprintHasher,
}

impl<'a> JsonPacketCodec<'a> {
    pub fn new(hasher: &'a dyn FingerprintHasher) -> JsonPacketCodec<'a> {
        JsonPacketCodec { hasher }
    }
}

fn invalid(code: &'static str, message: impl Into<String>) -> PacketFailure {
    PacketFailure::Invalid {
        code,
        message: message.into(),
    }
}

fn from_json_error(err: JsonError) -> PacketFailure {
    match err.code {
        "json_depth" | "json_records" => invalid("packet_limit_exceeded", err.message),
        _ => invalid("packet_schema_invalid", err.message),
    }
}

fn remove_digest(envelope: &mut Json) -> Option<Json> {
    if let Json::Object(map) = envelope
        && let Some(Json::Object(data)) = map.get_mut("data")
    {
        return data
            .remove("packet_digest")
            .or_else(|| data.remove("artifact_digest"));
    }
    None
}

/// Read one unsigned number from the envelope without consuming it.
fn peek_u64(envelope: &Json, path: &[&str]) -> Option<u64> {
    let mut current = envelope;
    for key in path {
        let Json::Object(map) = current else {
            return None;
        };
        current = map.get(*key)?;
    }
    match current {
        Json::Number(n) => Some(*n),
        _ => None,
    }
}

/// Refuse an artifact from an earlier release before any integrity check.
///
/// An artifact produced by an older Memoria was hashed under that release's
/// integrity domain, so the current digest never matches it. Checking the
/// digest first would call an intact old artifact corrupt. The version header
/// is therefore inspected first, and an unsupported version is refused with
/// instructions to produce a new artifact.
///
/// Nothing here decodes an old body. There is no converter, no migration, and
/// no automatic upgrade: review artifacts are ephemeral, so regeneration is
/// the whole procedure.
fn unsupported_version(envelope: &Json, manifest_kind: bool) -> Option<PacketFailure> {
    let regenerate = if manifest_kind {
        "Run `memoria review PATH --format json` again to produce a current artifact"
    } else {
        "Run `memoria review PATH --full --format json` again to produce a current artifact"
    };
    // A malformed or absent header is not a version problem. It falls through
    // to the ordinary strict schema path, which names the exact field.
    if let Some(schema) = peek_u64(envelope, &["schema_version"])
        && schema != ENVELOPE_SCHEMA_VERSION
    {
        return Some(invalid(
            "packet_schema_invalid",
            format!(
                "envelope.schema_version is {schema}; this release accepts {ENVELOPE_SCHEMA_VERSION} only. {regenerate}; old artifacts are not converted"
            ),
        ));
    }
    let (key, expected) = if manifest_kind {
        ("manifest_version", MANIFEST_VERSION)
    } else {
        ("packet_version", PACKET_VERSION)
    };
    if let Some(version) = peek_u64(envelope, &["data", key])
        && version != expected
    {
        return Some(invalid(
            "packet_schema_invalid",
            format!(
                "data.{key} is {version}; this release accepts {expected} only. {regenerate}; old artifacts are not converted"
            ),
        ));
    }
    None
}

/// Whether an envelope's `data.kind` names the small manifest.
fn is_manifest(envelope: &Json) -> bool {
    matches!(envelope, Json::Object(map)
        if matches!(map.get("data"), Some(Json::Object(data))
            if data.get("kind") == Some(&Json::String(MANIFEST_KIND.into()))))
}

fn set_record_count(envelope: &mut Json, records: u64) {
    if let Json::Object(map) = envelope
        && let Some(Json::Object(data)) = map.get_mut("data")
        && let Some(Json::Object(size)) = data.get_mut("size")
    {
        size.insert("record_count".into(), Json::Number(records));
    }
}

fn insert_digest(envelope: &mut Json, key: &str, digest: String) {
    if let Json::Object(map) = envelope
        && let Some(Json::Object(data)) = map.get_mut("data")
    {
        data.insert(key.into(), Json::String(digest));
    }
}

fn envelope_of(packet: &FocusedReviewPacket, diagnostics: &[Diagnostic]) -> Json {
    generic_envelope("review", true, &packet.to_detail(), diagnostics)
}

/// The JSON envelope shared by every command and every outcome
/// (`schema_version`, `command`, `ok`, `data`, `diagnostics`). The
/// presentation serializes exactly this value, so measuring it here is
/// measuring the output.
pub fn generic_envelope(
    command: &str,
    ok: bool,
    data: &Detail,
    diagnostics: &[Diagnostic],
) -> Json {
    let mut map = BTreeMap::new();
    map.insert(
        "schema_version".into(),
        Json::Number(ENVELOPE_SCHEMA_VERSION),
    );
    map.insert("command".into(), Json::String(command.into()));
    map.insert("ok".into(), Json::Bool(ok));
    map.insert("data".into(), from_detail(data));
    map.insert(
        "diagnostics".into(),
        Json::Array(diagnostics.iter().map(diagnostic_to_json).collect()),
    );
    Json::Object(map)
}

/// Measure a generic envelope as the presentation would emit it.
pub fn measure_envelope(envelope: &Json) -> EnvelopeSize {
    EnvelopeSize {
        records: json::count_records(envelope),
        depth: json::max_depth(envelope) as u64,
        serialized_bytes: json::to_pretty(envelope).len() as u64,
    }
}

impl ReviewPacketCodec for JsonPacketCodec<'_> {
    fn complete_record_count(
        &self,
        packet: &FocusedReviewPacket,
        diagnostics: &[Diagnostic],
    ) -> u64 {
        json::count_records(&envelope_of(packet, diagnostics))
    }

    fn envelope_size(
        &self,
        command: &str,
        ok: bool,
        data: &Detail,
        diagnostics: &[Diagnostic],
    ) -> EnvelopeSize {
        measure_envelope(&generic_envelope(command, ok, data, diagnostics))
    }

    fn encode(
        &self,
        packet: &FocusedReviewPacket,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<u8>, PacketFailure> {
        let mut envelope = envelope_of(packet, diagnostics);
        remove_digest(&mut envelope);
        // Publish the complete-envelope record count (data and diagnostics,
        // every array element once) before the digest covers it.
        let records = json::count_records(&envelope);
        set_record_count(&mut envelope, records);
        let digest = packet_digest(self.hasher, &envelope);
        insert_digest(&mut envelope, "packet_digest", digest);
        if records > MAX_RECORDS {
            return Err(invalid(
                "packet_too_large",
                format!("the packet has {records} records, above the cap of {MAX_RECORDS}"),
            ));
        }
        let depth = json::max_depth(&envelope) as u64;
        if depth > MAX_DEPTH {
            return Err(invalid(
                "packet_too_large",
                format!("the packet nests {depth} containers, above the cap of {MAX_DEPTH}"),
            ));
        }
        let text = json::to_pretty(&envelope);
        if text.len() as u64 > MAX_SERIALIZED_BYTES {
            return Err(invalid(
                "packet_too_large",
                format!(
                    "the serialized packet is {} bytes, above the cap of {MAX_SERIALIZED_BYTES}",
                    text.len()
                ),
            ));
        }
        Ok(text.into_bytes())
    }

    fn encode_manifest(
        &self,
        manifest: &ReviewManifest,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<u8>, PacketFailure> {
        let mut envelope = generic_envelope("review", true, &manifest.to_detail(), diagnostics);
        remove_digest(&mut envelope);
        let digest = manifest_digest(self.hasher, &envelope);
        insert_digest(&mut envelope, "artifact_digest", digest);
        let records = json::count_records(&envelope);
        if records > MAX_RECORDS {
            return Err(invalid(
                "packet_too_large",
                format!("the manifest has {records} records, above the cap of {MAX_RECORDS}"),
            ));
        }
        let depth = json::max_depth(&envelope) as u64;
        if depth > MAX_DEPTH {
            return Err(invalid(
                "packet_too_large",
                format!("the manifest nests {depth} containers, above the cap of {MAX_DEPTH}"),
            ));
        }
        let text = json::to_pretty(&envelope);
        if text.len() as u64 > MAX_SERIALIZED_BYTES {
            return Err(invalid(
                "packet_too_large",
                format!(
                    "the serialized manifest is {} bytes, above the cap of {MAX_SERIALIZED_BYTES}",
                    text.len()
                ),
            ));
        }
        Ok(text.into_bytes())
    }

    fn decode(&self, bytes: &[u8]) -> Result<ReviewArtifact, PacketFailure> {
        if bytes.len() as u64 > MAX_SERIALIZED_BYTES {
            return Err(limit_error());
        }
        let mut envelope = json::parse(bytes, Limits::PACKET).map_err(from_json_error)?;
        let manifest_kind = is_manifest(&envelope);
        // The version header decides acceptance before the integrity domain is
        // chosen, so an intact artifact from an earlier release is refused as
        // an old version rather than as corruption.
        if let Some(failure) = unsupported_version(&envelope, manifest_kind) {
            return Err(failure);
        }
        let (digest_key, domain) = if manifest_kind {
            ("artifact_digest", REVIEW_MANIFEST_DOMAIN)
        } else {
            ("packet_digest", PACKET_V3_DOMAIN)
        };
        let Some(Json::String(claimed)) = remove_digest(&mut envelope) else {
            return Err(invalid(
                "packet_schema_invalid",
                format!("data.{digest_key} must be a string"),
            ));
        };
        Hash64::parse(&claimed)
            .map_err(|e| invalid("packet_schema_invalid", format!("data.{digest_key}: {e}")))?;
        let actual = digest_with(self.hasher, domain, &envelope);
        if actual != claimed {
            return Err(invalid(
                "packet_integrity_failed",
                format!(
                    "{digest_key} does not match the artifact content; it was modified after `memoria review` produced it"
                ),
            ));
        }
        let records = json::count_records(&envelope);
        if manifest_kind {
            let manifest = decode_manifest_envelope(envelope, claimed).map_err(from_json_error)?;
            return Ok(ReviewArtifact::Manifest(Box::new(manifest)));
        }
        let packet = decode_envelope(envelope, claimed).map_err(from_json_error)?;
        if packet.record_count != records {
            return Err(invalid(
                "packet_schema_invalid",
                format!(
                    "size.record_count is {} but the envelope contains {records} array elements",
                    packet.record_count
                ),
            ));
        }
        Ok(ReviewArtifact::Full(Box::new(packet)))
    }
}

/// Validate the shared envelope frame and hand back the `data` reader.
fn envelope_frame(envelope: Json) -> Result<ObjectReader, JsonError> {
    let schema = |m: String| JsonError {
        code: "json_schema",
        message: m,
    };
    let mut reader = ObjectReader::new(envelope, "envelope")?;
    // Envelopes from earlier releases are refused here, at the transport
    // boundary, so the clean cutover stays visible. Nothing is converted.
    let envelope_schema = reader.take_u64("schema_version")?;
    if envelope_schema != ENVELOPE_SCHEMA_VERSION {
        return Err(schema(format!(
            "envelope.schema_version must be {ENVELOPE_SCHEMA_VERSION}; this release does not accept version {envelope_schema} envelopes. Run `memoria review` again to produce a current artifact; old artifacts are not converted"
        )));
    }
    if reader.take_string("command")? != "review" {
        return Err(schema("envelope.command must be \"review\"".into()));
    }
    if !reader.take_bool("ok")? {
        return Err(schema(
            "envelope.ok must be true; failed or refused output is not an acknowledgement artifact"
                .into(),
        ));
    }
    for (index, item) in reader.take_array("diagnostics")?.into_iter().enumerate() {
        let ctx = format!("envelope.diagnostics[{index}]");
        let mut diagnostic = ObjectReader::new(item, &ctx)?;
        for key in ["code", "severity", "message"] {
            diagnostic.take_string(key)?;
        }
        diagnostic.take("path")?;
        diagnostic.take("line")?;
        diagnostic.take("column")?;
        diagnostic.take("details")?;
        diagnostic.finish()?;
    }
    let data = reader.take_object("data")?;
    reader.finish()?;
    Ok(data)
}

fn hash_of(reader: &mut ObjectReader, key: &str, ctx: &str) -> Result<Hash64, JsonError> {
    Hash64::parse(&reader.take_string(key)?).map_err(|e| JsonError {
        code: "json_schema",
        message: format!("{ctx}.{key}: {e}"),
    })
}

/// Decode the complete manifest-v1 data object.
///
/// `data` must already have its digest field removed. Used both for a small
/// manifest artifact and for a full export's embedded `requirements`.
fn decode_requirements(mut data: ObjectReader, ctx: &str) -> Result<ReviewManifest, JsonError> {
    let schema = |m: String| JsonError {
        code: "json_schema",
        message: m,
    };
    if data.take_string("kind")? != MANIFEST_KIND {
        return Err(schema(format!("{ctx}.kind must be {MANIFEST_KIND:?}")));
    }
    let version = data.take_u64("manifest_version")?;
    if version != MANIFEST_VERSION {
        return Err(schema(format!(
            "{ctx}.manifest_version must be {MANIFEST_VERSION}; this release does not accept version {version} manifests. Run `memoria review` again"
        )));
    }
    let document = DocumentId::parse(&data.take_string("document")?)
        .map_err(|e| schema(format!("{ctx}.document: {e}")))?;
    let review_revision = data.take_u64("review_revision")?;
    let token = data.take_string("token")?;

    let mut snapshot_reader = data.take_object("snapshot")?;
    let snapshot_ctx = format!("{ctx}.snapshot");
    let snapshot = SnapshotDigests {
        inputs_digest: hash_of(&mut snapshot_reader, "inputs_digest", &snapshot_ctx)?,
        context_digest: hash_of(&mut snapshot_reader, "context_digest", &snapshot_ctx)?,
        guidance_digest: hash_of(&mut snapshot_reader, "guidance_digest", &snapshot_ctx)?,
        baseline_digest: hash_of(&mut snapshot_reader, "baseline_digest", &snapshot_ctx)?,
        selection_version: snapshot_reader.take_u64("selection_version")?,
    };
    snapshot_reader.finish()?;
    // The selection version names the rules that produced this advice. An
    // unsupported value would describe requirements this release cannot check.
    if snapshot.selection_version != SELECTION_VERSION {
        return Err(schema(format!(
            "{snapshot_ctx}.selection_version is {}; this release accepts {SELECTION_VERSION} only",
            snapshot.selection_version
        )));
    }

    let baseline = match data.take("baseline")? {
        Json::Null => None,
        value => {
            let baseline_ctx = format!("{ctx}.baseline");
            let mut reader = ObjectReader::new(value, &baseline_ctx)?;
            let revision = reader.take_u64("revision")?;
            let token_digest = hash_of(&mut reader, "token_digest", &baseline_ctx)?;
            let reviewer = reader.take_string("reviewer")?;
            let result = reader.take_string("result")?;
            if result != "updated" && result != "no-update" {
                return Err(schema(format!(
                    "{baseline_ctx}.result must be `updated` or `no-update`"
                )));
            }
            let recorded_commit = reader.take_optional_string("recorded_commit")?;
            let evidence_status = match reader.take_string("evidence_status")?.as_str() {
                "verified" => EvidenceStatus::Verified,
                "partial" => EvidenceStatus::Partial,
                "unavailable" => EvidenceStatus::Unavailable,
                other => {
                    return Err(schema(format!(
                        "{baseline_ctx}.evidence_status {other:?} must be verified, partial, or unavailable"
                    )));
                }
            };
            reader.finish()?;
            Some(BaselineInfo {
                revision,
                token_digest,
                reviewer,
                result,
                recorded_commit,
                evidence_status,
            })
        }
    };

    let mut changes = Vec::new();
    for (index, item) in data.take_array("changes")?.into_iter().enumerate() {
        let entry_ctx = format!("{ctx}.changes[{index}]");
        let mut entry = ObjectReader::new(item, &entry_ctx)?;
        let kind = entry.take_string("kind")?;
        let change = entry.take_string("change")?;
        let identity = entry.take_string("identity")?;
        let before_bytes = optional_u64(entry.take("before_bytes")?, &entry_ctx)?;
        let before_hash = optional_hash(entry.take("before_hash")?, &entry_ctx)?;
        let after_bytes = optional_u64(entry.take("after_bytes")?, &entry_ctx)?;
        let after_hash = optional_hash(entry.take("after_hash")?, &entry_ctx)?;
        entry.finish()?;
        changes.push(ChangeEntry {
            kind,
            change,
            identity,
            before_bytes,
            before_hash,
            after_bytes,
            after_hash,
        });
    }

    let mut review = data.take_object("review")?;
    let mode = match review.take_string("mode")?.as_str() {
        "focused_candidate" => ReviewMode::FocusedCandidate,
        "full_baseline" => ReviewMode::FullBaseline,
        other => {
            return Err(schema(format!(
                "{ctx}.review.mode {other:?} must be focused_candidate or full_baseline"
            )));
        }
    };
    let mut sections = Vec::new();
    for (index, item) in review.take_array("sections")?.into_iter().enumerate() {
        let section_ctx = format!("{ctx}.review.sections[{index}]");
        let mut entry = ObjectReader::new(item, &section_ctx)?;
        let id = entry.take_string("id")?;
        memoria_domain::SectionId::parse(&id)
            .map_err(|e| schema(format!("{section_ctx}.id: {e}")))?;
        let heading = entry.take_string("heading")?;
        let lines = entry.take_array("lines")?;
        if lines.len() != 2 {
            return Err(schema(format!(
                "{section_ctx}.lines must be a two-element [start, end] array"
            )));
        }
        let mut bounds = [0usize; 2];
        for (i, value) in lines.into_iter().enumerate() {
            let Json::Number(n) = value else {
                return Err(schema(format!("{section_ctx}.lines[{i}] must be a number")));
            };
            bounds[i] = n as usize;
        }
        if bounds[0] == 0 || bounds[1] < bounds[0] {
            return Err(schema(format!(
                "{section_ctx}.lines must be a 1-based inclusive range"
            )));
        }
        let mut sources = Vec::new();
        for (i, value) in entry.take_array("sources")?.into_iter().enumerate() {
            let source_ctx = format!("{section_ctx}.sources[{i}]");
            let text = expect_string(value, &source_ctx)?;
            ProjectPath::parse(&text).map_err(|e| schema(format!("{source_ctx}: {e}")))?;
            sources.push(text);
        }
        entry.finish()?;
        sections.push(SectionSuggestion {
            id,
            heading,
            first_line: bounds[0],
            last_line: bounds[1],
            sources,
        });
    }
    if !review.take_bool("whole_readme_pass")? {
        return Err(schema(format!(
            "{ctx}.review.whole_readme_pass must be true; the whole-README pass is always required"
        )));
    }
    let mut fallback_reasons = Vec::new();
    for (index, item) in review
        .take_array("fallback_reasons")?
        .into_iter()
        .enumerate()
    {
        let reason_ctx = format!("{ctx}.review.fallback_reasons[{index}]");
        let mut entry = ObjectReader::new(item, &reason_ctx)?;
        let code = entry.take_string("code")?;
        if !memoria_application::review::FALLBACK_CODES.contains(&code.as_str()) {
            return Err(schema(format!(
                "{reason_ctx}.code {code:?} is not a known fallback code"
            )));
        }
        let identity = entry.take_optional_string("identity")?;
        let message = entry.take_string("message")?;
        entry.finish()?;
        fallback_reasons.push(FallbackReason {
            code,
            identity,
            message,
        });
    }
    review.finish()?;
    if mode == ReviewMode::FullBaseline && !sections.is_empty() {
        return Err(schema(format!(
            "{ctx}.review.sections must be empty in full_baseline mode"
        )));
    }
    if mode == ReviewMode::FocusedCandidate && !fallback_reasons.is_empty() {
        return Err(schema(format!(
            "{ctx}.review.fallback_reasons must be empty in focused_candidate mode"
        )));
    }

    let mut inputs = Vec::new();
    for (index, item) in data.take_array("inputs")?.into_iter().enumerate() {
        let input_ctx = format!("{ctx}.inputs[{index}]");
        let mut entry = ObjectReader::new(item, &input_ctx)?;
        let kind = entry.take_string("kind")?;
        if !["document", "file", "import"].contains(&kind.as_str()) {
            return Err(schema(format!(
                "{input_ctx}.kind {kind:?} must be document, file, or import"
            )));
        }
        let path = entry.take_string("path")?;
        ProjectPath::parse(&path).map_err(|e| schema(format!("{input_ctx}.path: {e}")))?;
        let export_id = entry.take_optional_string("export_id")?;
        if (kind == "import") != export_id.is_some() {
            return Err(schema(format!(
                "{input_ctx}.export_id is non-null exactly for imports"
            )));
        }
        let bytes = entry.take_u64("bytes")?;
        let hash = hash_of(&mut entry, "hash", &input_ctx)?;
        let role = match entry.take_string("role")?.as_str() {
            "whole_readme" => InputRole::WholeReadme,
            "changed_source" => InputRole::ChangedSource,
            "section_context" => InputRole::SectionContext,
            "current_import" => InputRole::CurrentImport,
            other => {
                return Err(schema(format!(
                    "{input_ctx}.role {other:?} is not a known role"
                )));
            }
        };
        entry.finish()?;
        inputs.push(InputEntry {
            kind,
            path,
            export_id,
            bytes,
            hash,
            role,
        });
    }

    let mut guidance = data.take_object("guidance")?;
    let guidance_ctx = format!("{ctx}.guidance");
    let guidance_digest = hash_of(&mut guidance, "digest", &guidance_ctx)?;
    let guidance_changed_since_review = match guidance.take("changed_since_review")? {
        Json::Null => None,
        Json::Bool(value) => Some(value),
        _ => {
            return Err(schema(format!(
                "{guidance_ctx}.changed_since_review must be a boolean or null"
            )));
        }
    };
    let mut guidance_references = Vec::new();
    for (index, item) in guidance.take_array("references")?.into_iter().enumerate() {
        let reference_ctx = format!("{guidance_ctx}.references[{index}]");
        let mut entry = ObjectReader::new(item, &reference_ctx)?;
        let scope = entry.take_string("scope")?;
        DirPath::parse(&scope).map_err(|e| schema(format!("{reference_ctx}.scope: {e}")))?;
        let source = entry.take_string("source")?;
        let kind = entry.take_string("kind")?;
        if GuidanceKind::parse(&kind).is_none() {
            return Err(schema(format!(
                "{reference_ctx}.kind must be `inline` or `file`"
            )));
        }
        let entry_index = entry.take_u64("entry_index")?;
        entry.finish()?;
        guidance_references.push(GuidanceReference {
            scope,
            source,
            kind,
            entry_index,
        });
    }
    for (index, value) in guidance.take_array("command")?.into_iter().enumerate() {
        expect_string(value, &format!("{guidance_ctx}.command[{index}]"))?;
    }
    guidance.finish()?;

    let mut covered_invalidations: Vec<(u64, String)> = Vec::new();
    for (index, item) in data
        .take_array("covered_invalidations")?
        .into_iter()
        .enumerate()
    {
        let entry_ctx = format!("{ctx}.covered_invalidations[{index}]");
        let mut entry = ObjectReader::new(item, &entry_ctx)?;
        let id = entry.take_u64("id")?;
        let reason = entry.take_string("reason")?;
        entry.finish()?;
        if let Some((previous, _)) = covered_invalidations.last()
            && *previous >= id
        {
            return Err(schema(format!(
                "{ctx}.covered_invalidations ids must strictly increase"
            )));
        }
        covered_invalidations.push((id, reason));
    }

    let mut counts = data.take_object("counts")?;
    let selected_files = counts.take_u64("selected_files")?;
    let imports = counts.take_u64("imports")?;
    let raw_input_bytes = counts.take_u64("raw_input_bytes")?;
    let suggested_sources = counts.take_u64("suggested_sources")?;
    counts.finish()?;

    let mut workflow = data.take_object("workflow")?;
    let policy = workflow.take_string("policy")?;
    if policy != memoria_domain::section::SELECTION_POLICY {
        return Err(schema(format!(
            "{ctx}.workflow.policy must be {:?}",
            memoria_domain::section::SELECTION_POLICY
        )));
    }
    let steps = workflow.take_array("steps")?;
    if steps.len() != WORKFLOW_STEPS.len() {
        return Err(schema(format!(
            "{ctx}.workflow.steps must have exactly {} entries",
            WORKFLOW_STEPS.len()
        )));
    }
    for (index, (value, expected)) in steps.into_iter().zip(WORKFLOW_STEPS).enumerate() {
        let text = expect_string(value, &format!("{ctx}.workflow.steps[{index}]"))?;
        if text != expected {
            return Err(schema(format!(
                "{ctx}.workflow.steps[{index}] must be {expected:?} for selection version 1"
            )));
        }
    }
    workflow.finish()?;
    data.finish()?;

    let manifest = ReviewManifest {
        document,
        review_revision,
        token,
        snapshot,
        baseline,
        changes,
        mode,
        sections,
        fallback_reasons,
        inputs,
        guidance_digest,
        guidance_changed_since_review,
        guidance_references,
        covered_invalidations,
        selected_files,
        imports,
        raw_input_bytes,
        artifact_digest: String::new(),
    };
    if manifest.suggested_sources() != suggested_sources {
        return Err(schema(format!(
            "{ctx}.counts.suggested_sources is {suggested_sources} but the sections name {} unique sources",
            manifest.suggested_sources()
        )));
    }
    // The whole-README pass is always required, so the README is always a
    // suggested read. Exactly one entry carries that identity and that role.
    let readme: Vec<&InputEntry> = manifest
        .inputs
        .iter()
        .filter(|entry| entry.kind == "document")
        .collect();
    match readme.as_slice() {
        [entry] => {
            if entry.path != manifest.document.as_str() {
                return Err(schema(format!(
                    "{ctx}.inputs names document {:?} but the artifact describes {}",
                    entry.path, manifest.document
                )));
            }
            if entry.role != InputRole::WholeReadme {
                return Err(schema(format!(
                    "{ctx}.inputs entry for {} must have role whole_readme",
                    manifest.document
                )));
            }
        }
        [] => {
            return Err(schema(format!(
                "{ctx}.inputs must contain {} with role whole_readme; the whole-README pass is always required",
                manifest.document
            )));
        }
        _ => {
            return Err(schema(format!(
                "{ctx}.inputs contains {} document entries; exactly one is permitted",
                readme.len()
            )));
        }
    }
    if manifest
        .inputs
        .iter()
        .any(|entry| entry.kind != "document" && entry.role == InputRole::WholeReadme)
    {
        return Err(schema(format!(
            "{ctx}.inputs gives role whole_readme to an entry that is not the document"
        )));
    }
    // Reading identities are unique: a duplicate would misstate the work.
    let mut identities: Vec<(&str, Option<&str>)> = manifest
        .inputs
        .iter()
        .map(|entry| (entry.path.as_str(), entry.export_id.as_deref()))
        .collect();
    identities.sort_unstable();
    let total = identities.len();
    identities.dedup();
    if identities.len() != total {
        return Err(schema(format!("{ctx}.inputs repeats a reading identity")));
    }
    // A boundary with a selected file cannot report an empty one, and the
    // suggested sources are a subset of the selected files.
    if manifest.suggested_sources() > manifest.selected_files {
        return Err(schema(format!(
            "{ctx}.counts.suggested_sources is {} but the boundary reports {} selected files",
            manifest.suggested_sources(),
            manifest.selected_files
        )));
    }
    Ok(manifest)
}

fn decode_manifest_envelope(envelope: Json, digest: String) -> Result<ReviewManifest, JsonError> {
    let data = envelope_frame(envelope)?;
    let mut manifest = decode_requirements(data, "data")?;
    manifest.artifact_digest = digest;
    Ok(manifest)
}

fn decode_envelope(envelope: Json, digest: String) -> Result<FocusedReviewPacket, JsonError> {
    let schema = |m: String| JsonError {
        code: "json_schema",
        message: m,
    };
    let mut data = envelope_frame(envelope)?;
    if data.take_string("kind")? != "focused_review" {
        return Err(schema(
            "data.kind must be \"focused_review\"; a review plan is not an acknowledgement artifact"
                .into(),
        ));
    }
    let packet_version = data.take_u64("packet_version")?;
    if packet_version != PACKET_VERSION {
        // Packets are ephemeral. An older one is regenerated, never
        // converted, migrated, or reinterpreted as a new manifest.
        return Err(schema(format!(
            "data.packet_version must be {PACKET_VERSION}; this release does not accept version {packet_version} packets. Run `memoria review PATH --full --format json` again to produce a current packet; old packets are not converted"
        )));
    }
    let document = DocumentId::parse(&data.take_string("document")?)
        .map_err(|e| schema(format!("data.document: {e}")))?;
    let review_revision = data.take_u64("review_revision")?;
    let manifest = manifest_from_json(data.take("manifest")?, "data.manifest")?;
    let mut covered = Vec::new();
    for (index, item) in data
        .take_array("covered_invalidations")?
        .into_iter()
        .enumerate()
    {
        let ctx = format!("data.covered_invalidations[{index}]");
        let mut inv = ObjectReader::new(item, &ctx)?;
        let id = inv.take_u64("id")?;
        let reason = inv.take_string("reason")?;
        inv.finish()?;
        covered.push((id, reason));
    }
    let token = data.take_string("token")?;
    let mut budget: u64 = 0;
    let mut content = data.take_object("content")?;
    let readme = file_content(content.take("readme")?, "data.content.readme", &mut budget)?;
    let mut files = Vec::new();
    for (index, item) in content.take_array("files")?.into_iter().enumerate() {
        files.push(file_content(
            item,
            &format!("data.content.files[{index}]"),
            &mut budget,
        )?);
    }
    let mut imports = Vec::new();
    for (index, item) in content.take_array("imports")?.into_iter().enumerate() {
        let ctx = format!("data.content.imports[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let document = entry.take_string("document")?;
        let export_id = entry.take_string("export_id")?;
        let bytes = entry.take_u64("bytes")?;
        let hash = Hash64::parse(&entry.take_string("hash")?)
            .map_err(|e| schema(format!("{ctx}.hash: {e}")))?;
        let encoding = encoding_of(&entry.take_string("encoding")?, &ctx)?;
        let body = decode_body(entry.take_string("body")?, encoding, &ctx, &mut budget)?;
        entry.finish()?;
        imports.push(ImportContent {
            document,
            export_id,
            bytes,
            hash,
            encoding,
            body,
        });
    }
    content.finish()?;

    let mut context = data.take_object("context")?;
    let previous_review = match context.take("previous_review")? {
        Json::Null => None,
        value => Some(record_from_json(value, "data.context.previous_review")?),
    };
    let mut git = context.take_object("git")?;
    let git_context = GitContext {
        base_commit: git.take_optional_string("base_commit")?,
        worktree_dirty: git.take_bool("worktree_dirty")?,
    };
    git.finish()?;
    // Effective guidance travels with the packet so the token can bind the
    // exact context the reviewer saw. Its complete text counts against the
    // decoded budget; it is never truncated.
    let mut guidance_reader = context.take_object("guidance")?;
    let guidance_digest = GuidanceDigest(
        Hash64::parse(&guidance_reader.take_string("digest")?)
            .map_err(|e| schema(format!("data.context.guidance.digest: {e}")))?,
    );
    let mut guidance_entries = Vec::new();
    for (index, item) in guidance_reader
        .take_array("entries")?
        .into_iter()
        .enumerate()
    {
        let ctx = format!("data.context.guidance.entries[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let scope = DirPath::parse(&entry.take_string("scope")?)
            .map_err(|e| schema(format!("{ctx}.scope: {e}")))?;
        let source = entry.take_string("source")?;
        let kind_text = entry.take_string("kind")?;
        let kind = GuidanceKind::parse(&kind_text)
            .ok_or_else(|| schema(format!("{ctx}.kind must be `inline` or `file`")))?;
        let text = entry.take_string("text")?;
        entry.finish()?;
        add_budget(&mut budget, text.len() as u64)?;
        guidance_entries.push(GuidanceEntry {
            scope,
            source,
            kind,
            text,
        });
    }
    guidance_reader.finish()?;
    let guidance = EffectiveGuidance {
        document: document.clone(),
        entries: guidance_entries,
        digest: guidance_digest,
    };
    let mut exports = Vec::new();
    for (index, item) in context.take_array("exports")?.into_iter().enumerate() {
        let ctx = format!("data.context.exports[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let id = entry.take_string("id")?;
        let bytes = entry.take_u64("bytes")?;
        let hash = Hash64::parse(&entry.take_string("hash")?)
            .map_err(|e| schema(format!("{ctx}.hash: {e}")))?;
        let consumers = entry
            .take_array("consumers")?
            .into_iter()
            .enumerate()
            .map(|(i, c)| expect_string(c, &format!("{ctx}.consumers[{i}]")))
            .collect::<Result<Vec<_>, _>>()?;
        entry.finish()?;
        exports.push(ExportEntry {
            id,
            bytes,
            hash,
            consumers,
        });
    }
    let consumers = context
        .take_array("consumers")?
        .into_iter()
        .enumerate()
        .map(|(i, c)| expect_string(c, &format!("data.context.consumers[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut changes = Vec::new();
    for (index, item) in context.take_array("changes")?.into_iter().enumerate() {
        let ctx = format!("data.context.changes[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let kind = entry.take_string("kind")?;
        let change = entry.take_string("change")?;
        let identity = entry.take_string("identity")?;
        let before_bytes = optional_u64(entry.take("before_bytes")?, &ctx)?;
        let before_hash = optional_hash(entry.take("before_hash")?, &ctx)?;
        let after_bytes = optional_u64(entry.take("after_bytes")?, &ctx)?;
        let after_hash = optional_hash(entry.take("after_hash")?, &ctx)?;
        entry.finish()?;
        changes.push(ChangeEntry {
            kind,
            change,
            identity,
            before_bytes,
            before_hash,
            after_bytes,
            after_hash,
        });
    }
    let mut diffs = Vec::new();
    for (index, item) in context.take_array("diffs")?.into_iter().enumerate() {
        let ctx = format!("data.context.diffs[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let identity = entry.take_string("identity")?;
        let status = entry.take_string("status")?;
        let reason = entry.take_optional_string("reason")?;
        let old_encoding = match entry.take_optional_string("old_encoding")? {
            None => None,
            Some(text) => Some(encoding_of(&text, &ctx)?),
        };
        let old_body = match (entry.take_optional_string("old_body")?, old_encoding) {
            (Some(body), Some(encoding)) => Some(decode_body(body, encoding, &ctx, &mut budget)?),
            (None, _) => None,
            (Some(_), None) => return Err(schema(format!("{ctx}.old_body requires old_encoding"))),
        };
        let text = entry.take_optional_string("text")?;
        if let Some(text) = &text {
            add_budget(&mut budget, text.len() as u64)?;
        }
        entry.finish()?;
        diffs.push(DiffEntry {
            identity,
            status,
            reason,
            old_encoding,
            old_body,
            text,
        });
    }
    context.finish()?;
    // The embedded requirements are the complete manifest-v1 data object
    // without its own digest: the packet digest already covers them.
    let requirements = decode_requirements(
        ObjectReader::new(data.take("requirements")?, "data.requirements")?,
        "data.requirements",
    )?;
    let mut binding_reader = data.take_object("binding")?;
    let context = decode_context(binding_reader.take("context")?, "data.binding.context")?;
    let baseline = match binding_reader.take("baseline")? {
        Json::Null => None,
        value => Some(record_from_json(value, "data.binding.baseline")?),
    };
    binding_reader.finish()?;
    let mut size = data.take_object("size")?;
    let raw_input_bytes = size.take_u64("raw_input_bytes")?;
    let record_count = size.take_u64("record_count")?;
    size.finish()?;
    data.finish()?;
    Ok(FocusedReviewPacket {
        document,
        review_revision,
        manifest,
        covered_invalidations: covered,
        token,
        packet_digest: digest,
        content: PacketContent {
            readme,
            files,
            imports,
        },
        context: PacketContext {
            previous_review,
            git: git_context,
            guidance,
            exports,
            consumers,
            changes,
            diffs,
        },
        raw_input_bytes,
        record_count,
        requirements,
        binding: PacketBinding { context, baseline },
    })
}

/// Decode the canonical context descriptors that reproduce `C`.
fn decode_context(value: Json, ctx: &str) -> Result<ReviewContext, JsonError> {
    let schema = |m: String| JsonError {
        code: "json_schema",
        message: m,
    };
    let mut reader = ObjectReader::new(value, ctx)?;
    let strings = |reader: &mut ObjectReader, key: &str| -> Result<Vec<String>, JsonError> {
        reader
            .take_array(key)?
            .into_iter()
            .enumerate()
            .map(|(i, v)| expect_string(v, &format!("{ctx}.{key}[{i}]")))
            .collect()
    };
    let edges = |reader: &mut ObjectReader, key: &str| -> Result<Vec<ImportEdge>, JsonError> {
        let mut out = Vec::new();
        for (index, item) in reader.take_array(key)?.into_iter().enumerate() {
            let edge_ctx = format!("{ctx}.{key}[{index}]");
            let mut entry = ObjectReader::new(item, &edge_ctx)?;
            let provider = entry.take_string("provider")?;
            let export_id = entry.take_string("export_id")?;
            let hash = hash_of(&mut entry, "hash", &edge_ctx)?;
            entry.finish()?;
            out.push(ImportEdge {
                provider,
                export_id,
                hash,
            });
        }
        Ok(out)
    };
    let selection_version = reader.take_u64("selection_version")?;
    let owner = reader.take_string("owner")?;
    let ancestor_boundaries = strings(&mut reader, "ancestor_boundaries")?;
    let descendant_boundaries = strings(&mut reader, "descendant_boundaries")?;
    let nested_repositories = strings(&mut reader, "nested_repositories")?;
    let policy_hash = hash_of(&mut reader, "policy_hash", ctx)?;
    let owned_paths = strings(&mut reader, "owned_paths")?;
    let mapping_state = reader.take_string("mapping_state")?;
    let mapping_value = reader.take("mapping")?;
    let mapping = match (mapping_state.as_str(), mapping_value) {
        ("absent", Json::String(text)) if text == "absent" => SectionMapIdentity::Absent,
        ("invalid", Json::String(text)) if text == "invalid" => SectionMapIdentity::Invalid,
        ("valid", Json::Array(items)) => {
            let mut pairs = Vec::new();
            for (index, item) in items.into_iter().enumerate() {
                let pair_ctx = format!("{ctx}.mapping[{index}]");
                let mut entry = ObjectReader::new(item, &pair_ctx)?;
                let id = entry.take_string("id")?;
                let mut sources = Vec::new();
                for (i, value) in entry.take_array("sources")?.into_iter().enumerate() {
                    sources.push(expect_string(value, &format!("{pair_ctx}.sources[{i}]"))?);
                }
                entry.finish()?;
                pairs.push((id, sources));
            }
            SectionMapIdentity::Valid(pairs)
        }
        (state, _) => {
            return Err(schema(format!(
                "{ctx}.mapping does not match mapping_state {state:?}"
            )));
        }
    };
    let guidance = GuidanceDigest(hash_of(&mut reader, "guidance_digest", ctx)?);
    let imports = edges(&mut reader, "imports")?;
    let mut consumer_edges = Vec::new();
    for (index, item) in reader.take_array("consumer_edges")?.into_iter().enumerate() {
        let edge_ctx = format!("{ctx}.consumer_edges[{index}]");
        let mut entry = ObjectReader::new(item, &edge_ctx)?;
        let export_id = entry.take_string("export_id")?;
        let consumer = entry.take_string("consumer")?;
        entry.finish()?;
        consumer_edges.push(ConsumerEdge {
            export_id,
            consumer,
        });
    }
    let mut providers = Vec::new();
    for (index, item) in reader.take_array("providers")?.into_iter().enumerate() {
        let provider_ctx = format!("{ctx}.providers[{index}]");
        let mut entry = ObjectReader::new(item, &provider_ctx)?;
        let document = entry.take_string("document")?;
        let inputs_digest = hash_of(&mut entry, "inputs_digest", &provider_ctx)?;
        let guidance_digest = hash_of(&mut entry, "guidance_digest", &provider_ctx)?;
        let review_revision = entry.take_u64("review_revision")?;
        let mut active_invalidations = Vec::new();
        for (i, value) in entry
            .take_array("active_invalidations")?
            .into_iter()
            .enumerate()
        {
            let inv_ctx = format!("{provider_ctx}.active_invalidations[{i}]");
            let mut inv = ObjectReader::new(value, &inv_ctx)?;
            let id = inv.take_u64("id")?;
            let reason = inv.take_string("reason")?;
            inv.finish()?;
            active_invalidations.push((id, reason));
        }
        let provider_imports = edges(&mut entry, "imports")?;
        entry.finish()?;
        providers.push(ProviderDescriptor {
            document,
            inputs_digest,
            guidance_digest,
            review_revision,
            active_invalidations,
            imports: provider_imports,
        });
    }
    reader.finish()?;
    Ok(ReviewContext {
        selection_version,
        owner,
        ancestor_boundaries,
        descendant_boundaries,
        nested_repositories,
        policy_hash,
        owned_paths,
        mapping,
        guidance,
        imports,
        consumer_edges,
        providers,
    })
}

fn optional_u64(value: Json, ctx: &str) -> Result<Option<u64>, JsonError> {
    match value {
        Json::Null => Ok(None),
        Json::Number(n) => Ok(Some(n)),
        _ => Err(JsonError {
            code: "json_schema",
            message: format!("{ctx}: expected an unsigned integer or null"),
        }),
    }
}

fn optional_hash(value: Json, ctx: &str) -> Result<Option<Hash64>, JsonError> {
    match value {
        Json::Null => Ok(None),
        Json::String(s) => Hash64::parse(&s).map(Some).map_err(|e| JsonError {
            code: "json_schema",
            message: format!("{ctx}: {e}"),
        }),
        _ => Err(JsonError {
            code: "json_schema",
            message: format!("{ctx}: expected a hash string or null"),
        }),
    }
}

fn encoding_of(text: &str, ctx: &str) -> Result<ContentEncoding, JsonError> {
    match text {
        "utf8" => Ok(ContentEncoding::Utf8),
        "base64" => Ok(ContentEncoding::Base64),
        other => Err(JsonError {
            code: "json_schema",
            message: format!("{ctx}.encoding {other:?} must be utf8 or base64"),
        }),
    }
}

fn add_budget(budget: &mut u64, bytes: u64) -> Result<(), JsonError> {
    *budget += bytes;
    if *budget > MAX_DECODED_BYTES {
        return Err(JsonError {
            code: "json_records",
            message: format!("decoded packet content exceeds the cap of {MAX_DECODED_BYTES} bytes"),
        });
    }
    Ok(())
}

fn decode_body(
    body: String,
    encoding: ContentEncoding,
    ctx: &str,
    budget: &mut u64,
) -> Result<Vec<u8>, JsonError> {
    match encoding {
        ContentEncoding::Utf8 => {
            add_budget(budget, body.len() as u64)?;
            Ok(body.into_bytes())
        }
        ContentEncoding::Base64 => {
            let padding = body.bytes().rev().take_while(|b| *b == b'=').count() as u64;
            let estimated = (body.len() as u64 / 4) * 3;
            add_budget(budget, estimated.saturating_sub(padding))?;
            base64::engine::general_purpose::STANDARD
                .decode(body.as_bytes())
                .map_err(|e| JsonError {
                    code: "json_schema",
                    message: format!("{ctx}.body is not valid base64: {e}"),
                })
        }
    }
}

fn file_content(value: Json, ctx: &str, budget: &mut u64) -> Result<FileContent, JsonError> {
    let mut entry = ObjectReader::new(value, ctx)?;
    let path = entry.take_string("path")?;
    let bytes = entry.take_u64("bytes")?;
    let hash = Hash64::parse(&entry.take_string("hash")?).map_err(|e| JsonError {
        code: "json_schema",
        message: format!("{ctx}.hash: {e}"),
    })?;
    let encoding = encoding_of(&entry.take_string("encoding")?, ctx)?;
    let body = decode_body(entry.take_string("body")?, encoding, ctx, budget)?;
    entry.finish()?;
    Ok(FileContent {
        path,
        bytes,
        hash,
        encoding,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Xxh3Hasher;
    use memoria_application::error::{Detail, DetailMap};
    use memoria_domain::{DocumentId, GitContext, InputManifest};

    // MEM-046: refusal envelopes are measured exactly as they are emitted.
    #[test]
    fn envelope_measurement_matches_presentation_and_hard_limits() {
        use memoria_application::packet::{MAX_DEPTH, MAX_RECORDS, MAX_SERIALIZED_BYTES};
        let diagnostic = Diagnostic::error("packet_too_large", "too big").at_path("README.md");
        let data = DetailMap::default()
            .text("kind", "packet_refused")
            .with("items", Detail::list((0..3).map(Detail::Number)))
            .build();
        let envelope = generic_envelope("review", false, &data, std::slice::from_ref(&diagnostic));
        let size = measure_envelope(&envelope);
        assert_eq!(size.records, 4, "three list elements plus one diagnostic");
        assert_eq!(
            size.depth, 4,
            "envelope, diagnostics array, diagnostic object, details map"
        );
        assert_eq!(
            size.serialized_bytes,
            json::to_pretty(&envelope).len() as u64,
            "the byte length is the emitted length"
        );
        assert!(size.within_hard_limits());
        // Records: exactly the cap fits, one more does not (the diagnostic counts).
        let at_cap = |extra: u64| {
            let data = DetailMap::default()
                .with(
                    "items",
                    Detail::list((0..MAX_RECORDS - 1 + extra).map(|_| Detail::Null)),
                )
                .build();
            measure_envelope(&generic_envelope(
                "review",
                false,
                &data,
                std::slice::from_ref(&diagnostic),
            ))
        };
        assert_eq!(at_cap(0).records, MAX_RECORDS);
        assert!(at_cap(0).within_hard_limits());
        assert_eq!(at_cap(1).records, MAX_RECORDS + 1);
        assert!(!at_cap(1).within_hard_limits());
        // Depth: the envelope is depth 1, data depth 2; nested lists add one each.
        let nested = |levels: u64| {
            let mut value = Detail::Null;
            for _ in 0..levels {
                value = Detail::List(vec![value]);
            }
            let data = DetailMap::default().with("nested", value).build();
            measure_envelope(&generic_envelope("review", false, &data, &[]))
        };
        assert_eq!(nested(MAX_DEPTH - 2).depth, MAX_DEPTH);
        assert!(nested(MAX_DEPTH - 2).within_hard_limits());
        assert_eq!(nested(MAX_DEPTH - 1).depth, MAX_DEPTH + 1);
        assert!(!nested(MAX_DEPTH - 1).within_hard_limits());
        // Bytes: a payload at the serialized cap is refused by length alone.
        let big = DetailMap::default()
            .text("blob", "x".repeat(MAX_SERIALIZED_BYTES as usize))
            .build();
        let size = measure_envelope(&generic_envelope("review", false, &big, &[]));
        assert_eq!(size.records, 0);
        assert!(size.serialized_bytes > MAX_SERIALIZED_BYTES);
        assert!(!size.within_hard_limits());
        // The port method reports the same measurement.
        let hasher = Xxh3Hasher;
        let codec = JsonPacketCodec::new(&hasher);
        assert_eq!(
            codec.envelope_size("review", false, &data, std::slice::from_ref(&diagnostic)),
            measure_envelope(&generic_envelope(
                "review",
                false,
                &data,
                std::slice::from_ref(&diagnostic)
            ))
        );
    }

    fn minimal_requirements() -> ReviewManifest {
        ReviewManifest {
            document: DocumentId::parse("README.md").unwrap(),
            review_revision: 0,
            token: format!("mrv3.{}", "0".repeat(16)),
            snapshot: SnapshotDigests {
                inputs_digest: Hash64(0),
                context_digest: Hash64(0),
                guidance_digest: Hash64(0),
                baseline_digest: Hash64(0),
                selection_version: 1,
            },
            baseline: None,
            changes: vec![],
            mode: ReviewMode::FullBaseline,
            sections: vec![],
            fallback_reasons: vec![FallbackReason {
                code: "baseline_missing".into(),
                identity: None,
                message: "no previous review".into(),
            }],
            // The whole-README pass is always required, so even a minimal
            // artifact names the document as a suggested read.
            inputs: vec![InputEntry {
                kind: "document".into(),
                path: "README.md".into(),
                export_id: None,
                bytes: 0,
                hash: Hash64(2),
                role: InputRole::WholeReadme,
            }],
            guidance_digest: Hash64(0),
            guidance_changed_since_review: None,
            guidance_references: vec![],
            covered_invalidations: vec![],
            selected_files: 0,
            imports: 0,
            raw_input_bytes: 0,
            artifact_digest: String::new(),
        }
    }

    fn minimal_context() -> ReviewContext {
        ReviewContext {
            selection_version: 1,
            owner: "README.md".into(),
            ancestor_boundaries: vec![],
            descendant_boundaries: vec![],
            nested_repositories: vec![],
            policy_hash: Hash64(0),
            owned_paths: vec![],
            mapping: SectionMapIdentity::Absent,
            guidance: GuidanceDigest::default(),
            imports: vec![],
            consumer_edges: vec![],
            providers: vec![],
        }
    }

    fn minimal_packet() -> FocusedReviewPacket {
        let document = DocumentId::parse("README.md").unwrap();
        let manifest =
            InputManifest::new(document.clone(), Hash64(1), 0, Hash64(2), vec![], vec![]).unwrap();
        FocusedReviewPacket {
            document,
            review_revision: 0,
            manifest,
            covered_invalidations: vec![],
            token: format!("mrv3.{}", "0".repeat(16)),
            packet_digest: String::new(),
            content: PacketContent {
                readme: FileContent {
                    path: "README.md".into(),
                    bytes: 0,
                    hash: Hash64(2),
                    encoding: ContentEncoding::Utf8,
                    body: vec![],
                },
                files: vec![],
                imports: vec![],
            },
            context: PacketContext {
                previous_review: None,
                git: GitContext::default(),
                guidance: EffectiveGuidance {
                    document: DirPath::root().readme(),
                    entries: vec![],
                    digest: GuidanceDigest::default(),
                },
                exports: vec![],
                consumers: vec![],
                changes: vec![],
                diffs: vec![],
            },
            raw_input_bytes: 0,
            record_count: 0,
            requirements: minimal_requirements(),
            binding: PacketBinding {
                context: minimal_context(),
                baseline: None,
            },
        }
    }

    /// The decoded full export, or a panic naming the artifact kind.
    fn decode_full(codec: &JsonPacketCodec<'_>, bytes: &[u8]) -> FocusedReviewPacket {
        match codec.decode(bytes).unwrap() {
            ReviewArtifact::Full(packet) => *packet,
            ReviewArtifact::Manifest(_) => panic!("expected a full export"),
        }
    }

    fn diagnostics_with_elements(total: usize) -> Vec<Diagnostic> {
        // One diagnostic whose details hold a list of `total - 1` elements: the
        // diagnostic itself is the remaining element of the envelope array.
        let list = Detail::List(vec![Detail::Number(1); total - 1]);
        vec![
            Diagnostic::warning("probe", "nested")
                .with_details(DetailMap::default().with("items", list).build()),
        ]
    }

    #[test]
    fn record_count_is_the_complete_envelope_array_count() {
        let hasher = Xxh3Hasher;
        let codec = JsonPacketCodec::new(&hasher);
        let bytes = codec.encode(&minimal_packet(), &[]).unwrap();
        let envelope = json::parse(&bytes, Limits::PACKET).unwrap();
        let decoded = decode_full(&codec, &bytes);
        assert_eq!(decoded.record_count, json::count_records(&envelope));
        // A v3 export always states its own review requirements, so its
        // fixed workflow and guidance arrays are part of every packet.
        let base = decoded.record_count;
        assert!(base > 0, "a v3 packet carries its requirements");
        // Diagnostics and their nested arrays count too.
        let bytes = codec
            .encode(&minimal_packet(), &diagnostics_with_elements(5))
            .unwrap();
        let envelope = json::parse(&bytes, Limits::PACKET).unwrap();
        assert_eq!(json::count_records(&envelope), base + 5);
        assert_eq!(decode_full(&codec, &bytes).record_count, base + 5);
        // Exact boundary: 100,000 accepted, 100,001 refused by the producer.
        let bytes = codec
            .encode(
                &minimal_packet(),
                &diagnostics_with_elements(100_000 - base as usize),
            )
            .unwrap();
        assert_eq!(decode_full(&codec, &bytes).record_count, 100_000);
        let err = codec
            .encode(
                &minimal_packet(),
                &diagnostics_with_elements(100_001 - base as usize),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            PacketFailure::Invalid {
                code: "packet_too_large",
                ..
            }
        ));
        // A supplied count that disagrees with the envelope is rejected even with a recomputed digest.
        let mut tampered = json::parse(
            &codec
                .encode(&minimal_packet(), &diagnostics_with_elements(5))
                .unwrap(),
            Limits::PACKET,
        )
        .unwrap();
        set_record_count(&mut tampered, 4);
        remove_digest(&mut tampered);
        let digest = packet_digest(&hasher, &tampered);
        insert_digest(&mut tampered, "packet_digest", digest);
        let err = codec
            .decode(json::to_pretty(&tampered).as_bytes())
            .unwrap_err();
        assert!(
            matches!(
                err,
                PacketFailure::Invalid {
                    code: "packet_schema_invalid",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn j_encoding_is_order_independent_and_tagged() {
        let hasher = Xxh3Hasher;
        let a = json::parse(b"{\"b\":1,\"a\":[true,null,\"x\"]}", Limits::PACKET).unwrap();
        let b = json::parse(
            b"{ \"a\" : [ true , null , \"x\" ] , \"b\" : 1 }",
            Limits::PACKET,
        )
        .unwrap();
        assert_eq!(packet_digest(&hasher, &a), packet_digest(&hasher, &b));
        let c = json::parse(b"{\"b\":2,\"a\":[true,null,\"x\"]}", Limits::PACKET).unwrap();
        assert_ne!(packet_digest(&hasher, &a), packet_digest(&hasher, &c));
        // Manual layout check for a small value.
        struct Capture(Vec<u8>);
        impl HashStream for Capture {
            fn update(&mut self, bytes: &[u8]) {
                self.0.extend_from_slice(bytes);
            }
            fn finish(self: Box<Self>) -> Hash64 {
                Hash64(0)
            }
        }
        let mut capture = Capture(Vec::new());
        j_encode(
            &json::parse(b"{\"k\":[7]}", Limits::PACKET).unwrap(),
            &mut capture,
        );
        let expected: Vec<u8> = [
            vec![6],
            1u64.to_be_bytes().to_vec(),
            1u64.to_be_bytes().to_vec(),
            b"k".to_vec(),
            vec![5],
            1u64.to_be_bytes().to_vec(),
            vec![3],
            7u64.to_be_bytes().to_vec(),
        ]
        .concat();
        assert_eq!(capture.0, expected);
    }
}
