//! Bounded packet transport and strict envelope codec with integrity digest.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use base64::Engine as _;
use memoria_application::error::{Detail, Diagnostic};
use memoria_application::packet::{
    ChangeEntry, ContentEncoding, DiffEntry, EnvelopeSize, ExportEntry, FileContent,
    FocusedReviewPacket, ImportContent, InstructionEntry, MAX_DECODED_BYTES, MAX_DEPTH,
    MAX_RECORDS, MAX_SERIALIZED_BYTES, PACKET_VERSION, PacketContent, PacketContext,
};
use memoria_application::ports::{
    AdapterError, FileKind, FingerprintHasher, HashStream, PacketFailure, PacketInput,
    PacketSource, ReviewPacketCodec,
};
use memoria_domain::canonical::PACKET_DOMAIN;
use memoria_domain::{DocumentId, GitContext, Hash64};

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

/// `hex(H(C("memoria.packet.v1") || J(envelope)))`.
pub fn packet_digest(hasher: &dyn FingerprintHasher, envelope_without_digest: &Json) -> String {
    let mut sink = hasher.stream();
    sink.update(&(PACKET_DOMAIN.len() as u64).to_be_bytes());
    sink.update(PACKET_DOMAIN.as_bytes());
    j_encode(envelope_without_digest, &mut *sink);
    sink.finish().to_hex()
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
        return data.remove("packet_digest");
    }
    None
}

fn set_record_count(envelope: &mut Json, records: u64) {
    if let Json::Object(map) = envelope
        && let Some(Json::Object(data)) = map.get_mut("data")
        && let Some(Json::Object(size)) = data.get_mut("size")
    {
        size.insert("record_count".into(), Json::Number(records));
    }
}

fn insert_digest(envelope: &mut Json, digest: String) {
    if let Json::Object(map) = envelope
        && let Some(Json::Object(data)) = map.get_mut("data")
    {
        data.insert("packet_digest".into(), Json::String(digest));
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
    map.insert("schema_version".into(), Json::Number(1));
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
        insert_digest(&mut envelope, digest);
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

    fn decode(&self, bytes: &[u8]) -> Result<FocusedReviewPacket, PacketFailure> {
        if bytes.len() as u64 > MAX_SERIALIZED_BYTES {
            return Err(limit_error());
        }
        let mut envelope = json::parse(bytes, Limits::PACKET).map_err(from_json_error)?;
        let Some(Json::String(claimed)) = remove_digest(&mut envelope) else {
            return Err(invalid(
                "packet_schema_invalid",
                "data.packet_digest must be a string",
            ));
        };
        Hash64::parse(&claimed)
            .map_err(|e| invalid("packet_schema_invalid", format!("data.packet_digest: {e}")))?;
        let actual = packet_digest(self.hasher, &envelope);
        if actual != claimed {
            return Err(invalid(
                "packet_integrity_failed",
                "packet_digest does not match the packet content; the packet was modified after `memoria review` produced it",
            ));
        }
        let records = json::count_records(&envelope);
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
        Ok(packet)
    }
}

fn decode_envelope(envelope: Json, digest: String) -> Result<FocusedReviewPacket, JsonError> {
    let schema = |m: String| JsonError {
        code: "json_schema",
        message: m,
    };
    let mut reader = ObjectReader::new(envelope, "envelope")?;
    if reader.take_u64("schema_version")? != 1 {
        return Err(schema("envelope.schema_version must be 1".into()));
    }
    if reader.take_string("command")? != "review" {
        return Err(schema("envelope.command must be \"review\"".into()));
    }
    if !reader.take_bool("ok")? {
        return Err(schema(
            "envelope.ok must be true; failed or refused output is not a packet".into(),
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
    let mut data = reader.take_object("data")?;
    reader.finish()?;
    if data.take_string("kind")? != "focused_review" {
        return Err(schema(
            "data.kind must be \"focused_review\"; a review plan is not a packet".into(),
        ));
    }
    if data.take_u64("packet_version")? != PACKET_VERSION {
        return Err(schema(format!(
            "data.packet_version must be {PACKET_VERSION}"
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
    let mut instructions = Vec::new();
    for (index, item) in context.take_array("instructions")?.into_iter().enumerate() {
        let ctx = format!("data.context.instructions[{index}]");
        let mut entry = ObjectReader::new(item, &ctx)?;
        let source = entry.take_string("source")?;
        let kind = entry.take_string("kind")?;
        let text = entry.take_string("text")?;
        entry.finish()?;
        add_budget(&mut budget, text.len() as u64)?;
        instructions.push(InstructionEntry { source, kind, text });
    }
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
            instructions,
            exports,
            consumers,
            changes,
            diffs,
        },
        raw_input_bytes,
        record_count,
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
    use crate::hash::Xxh64Hasher;
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
        let hasher = Xxh64Hasher;
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

    fn minimal_packet() -> FocusedReviewPacket {
        let document = DocumentId::parse("README.md").unwrap();
        let manifest =
            InputManifest::new(document.clone(), Hash64(1), 0, Hash64(2), vec![], vec![]).unwrap();
        FocusedReviewPacket {
            document,
            review_revision: 0,
            manifest,
            covered_invalidations: vec![],
            token: format!("mrv1.{}", "0".repeat(16)),
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
                instructions: vec![],
                exports: vec![],
                consumers: vec![],
                changes: vec![],
                diffs: vec![],
            },
            raw_input_bytes: 0,
            record_count: 0,
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
        let hasher = Xxh64Hasher;
        let codec = JsonPacketCodec::new(&hasher);
        let bytes = codec.encode(&minimal_packet(), &[]).unwrap();
        let envelope = json::parse(&bytes, Limits::PACKET).unwrap();
        let decoded = codec.decode(&bytes).unwrap();
        assert_eq!(decoded.record_count, json::count_records(&envelope));
        assert_eq!(
            decoded.record_count, 0,
            "a minimal packet has no array elements"
        );
        // Diagnostics and their nested arrays count too.
        let bytes = codec
            .encode(&minimal_packet(), &diagnostics_with_elements(5))
            .unwrap();
        let envelope = json::parse(&bytes, Limits::PACKET).unwrap();
        assert_eq!(json::count_records(&envelope), 5);
        assert_eq!(codec.decode(&bytes).unwrap().record_count, 5);
        // Exact boundary: 100,000 accepted, 100,001 refused by the producer.
        let bytes = codec
            .encode(&minimal_packet(), &diagnostics_with_elements(100_000))
            .unwrap();
        assert_eq!(codec.decode(&bytes).unwrap().record_count, 100_000);
        let err = codec
            .encode(&minimal_packet(), &diagnostics_with_elements(100_001))
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
        insert_digest(&mut tampered, digest);
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
        let hasher = Xxh64Hasher;
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
