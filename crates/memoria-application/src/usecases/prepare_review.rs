//! `memoria review <README.md>`: one complete focused packet with its token.

use memoria_domain::{DocumentId, InputChange};

use super::evidence::diff_entry;
use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::packet::{
    ChangeEntry, ContentEncoding, DEFAULT_RAW_INPUT_LIMIT, DiffEntry, ExportEntry, FileContent,
    FocusedReviewPacket, ImportContent, MAX_DECODED_BYTES, MAX_RAW_INPUT_LIMIT, MAX_RECORDS,
    PacketContent, PacketContext, compute_token, manifest_detail,
};
use crate::ports::Services;
use crate::snapshot::{self, Snapshot};

use super::{diff_changes, parse_document};

/// Validate `--max-bytes`.
pub fn resolve_limit(max_bytes: Option<u64>) -> Result<u64, AppError> {
    match max_bytes {
        None => Ok(DEFAULT_RAW_INPUT_LIMIT),
        Some(0) => Err(AppError::usage(
            "max_bytes_invalid",
            "--max-bytes must be greater than zero",
        )),
        Some(value) if value > MAX_RAW_INPUT_LIMIT => Err(AppError::usage(
            "max_bytes_invalid",
            format!("--max-bytes cannot exceed the hard cap of {MAX_RAW_INPUT_LIMIT} bytes"),
        )),
        Some(value) => Ok(value),
    }
}

/// Reasons a document cannot receive a packet right now.
pub fn readiness_error(snapshot: &Snapshot, document: &DocumentId) -> Result<(), AppError> {
    let Some(status) = snapshot.status_of(document) else {
        return Err(AppError::validation(
            "document_not_found",
            format!("{document} is not a discovered README"),
        ));
    };
    if status.waiting() {
        let waiting: Vec<String> = status
            .waiting_on
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        return Err(AppError::new(
            ExitClass::Validation,
            Diagnostic::error(
                "dependencies_pending",
                format!("{document} waits for: {}", waiting.join(", ")),
            )
            .at_path(document.as_str())
            .with_details(
                DetailMap::default()
                    .with("waiting_on", Detail::texts(waiting))
                    .build(),
            ),
        ));
    }
    if !status.pending() {
        return Err(AppError::validation(
            "review_not_pending",
            format!("{document} is current and needs no review"),
        ));
    }
    let outdated = snapshot.outdated_imports_of(document);
    if !outdated.is_empty() {
        let list: Vec<String> = outdated
            .iter()
            .map(|o| format!("{}#{}", o.provider, o.export_id))
            .collect();
        return Err(AppError::new(
            ExitClass::Validation,
            Diagnostic::error(
                "imports_outdated",
                format!("{document} has outdated imports; run `memoria render {document}` first"),
            )
            .at_path(document.as_str())
            .with_details(
                DetailMap::default()
                    .with("imports", Detail::texts(list))
                    .build(),
            ),
        ));
    }
    Ok(())
}

pub fn run(
    services: &Services<'_>,
    raw_document: &str,
    max_bytes: Option<u64>,
) -> Result<Outcome<FocusedReviewPacket>, AppError> {
    let document = parse_document(raw_document)?;
    let limit = resolve_limit(max_bytes)?;
    let snapshot = snapshot::build(services)?;
    snapshot.require_valid()?;
    readiness_error(&snapshot, &document)?;
    let packet = build_packet(services, &snapshot, &document, limit)?;
    Ok(Outcome::new(packet, snapshot.non_error_diagnostics()))
}

/// Assemble the packet for a ready document.
pub fn build_packet(
    services: &Services<'_>,
    snapshot: &Snapshot,
    document: &DocumentId,
    limit: u64,
) -> Result<FocusedReviewPacket, AppError> {
    let hasher = services.hasher;
    let manifest = snapshot.manifests.get(document).cloned().ok_or_else(|| {
        AppError::validation("document_not_found", format!("{document} has no manifest"))
    })?;
    let graph = snapshot.graph.as_ref().expect("valid snapshot has a graph");
    let raw_input_bytes = manifest.raw_input_bytes();
    // Every refusal is bounded before it exists: the full manifest is
    // attached only when the complete refusal envelope (data plus the
    // diagnostic) fits every hard limit; otherwise the refusal carries
    // bounded counts and the diagnostic alone.
    let refuse = |message: String| -> AppError {
        let counts = DetailMap::default()
            .number("raw_input_bytes", raw_input_bytes)
            .number("limit", limit)
            .number("files", manifest.files().len() as u64)
            .number("imports", manifest.imports().len() as u64)
            .number("record_count", manifest.record_count());
        let diagnostic = Diagnostic::error("packet_too_large", message)
            .at_path(document.as_str())
            .with_details(counts.clone().build());
        let size = DetailMap::default()
            .number("raw_input_bytes", raw_input_bytes)
            .number("record_count", manifest.record_count())
            .number("limit", limit)
            .build();
        let full = DetailMap::default()
            .text("kind", "packet_refused")
            .text("document", document.as_str())
            .with("manifest", manifest_detail(&manifest))
            .with("size", size.clone())
            .build();
        let envelope = services.packets.envelope_size(
            "review",
            false,
            &full,
            std::slice::from_ref(&diagnostic),
        );
        let data = if envelope.within_hard_limits() {
            full
        } else {
            DetailMap::default()
                .text("kind", "packet_refused")
                .text("document", document.as_str())
                .bool("manifest_omitted", true)
                .with(
                    "manifest_summary",
                    counts
                        .text("policy_hash", manifest.policy_hash.to_hex())
                        .number("document_bytes", manifest.document_bytes)
                        .text("document_hash", manifest.document_hash.to_hex())
                        .number("envelope_records", envelope.records)
                        .number("envelope_bytes", envelope.serialized_bytes)
                        .build(),
                )
                .with("size", size)
                .build()
        };
        AppError::new(ExitClass::Validation, diagnostic).with_data(data)
    };
    if raw_input_bytes > limit {
        return Err(refuse(format!(
            "raw review inputs are {raw_input_bytes} bytes, above the limit of {limit} bytes; pass --max-bytes up to {MAX_RAW_INPUT_LIMIT} or reduce the owner's inputs"
        )));
    }

    let readme_bytes = snapshot.document_bytes(document);
    let file_content = |path: &str, bytes: &[u8]| FileContent {
        path: path.to_string(),
        bytes: bytes.len() as u64,
        hash: hasher.hash(bytes),
        encoding: ContentEncoding::for_bytes(bytes),
        body: bytes.to_vec(),
    };
    let readme = file_content(document.as_str(), readme_bytes);
    let files: Vec<FileContent> = manifest
        .files()
        .iter()
        .map(|f| file_content(f.path.as_str(), snapshot.file_bytes(&f.path)))
        .collect();
    let imports: Vec<ImportContent> = manifest
        .imports()
        .iter()
        .map(|i| {
            let body = snapshot
                .export_body(&i.document, &i.export_id)
                .unwrap_or(&[]);
            ImportContent {
                document: i.document.as_str().to_string(),
                export_id: i.export_id.as_str().to_string(),
                bytes: body.len() as u64,
                hash: hasher.hash(body),
                encoding: ContentEncoding::for_bytes(body),
                body: body.to_vec(),
            }
        })
        .collect();

    let previous = snapshot.state.reviews.get(document).cloned();
    // Effective guidance comes before the owned evidence. Its complete text
    // belongs to the packet byte budget: an oversized entry produces an
    // explicit limit error and never disappears through silent truncation.
    let guidance = snapshot.guidance_of(document);
    let exports: Vec<ExportEntry> = snapshot.documents[document]
        .exports
        .iter()
        .map(|export| {
            let body = snapshot.export_body(document, &export.id).unwrap_or(&[]);
            ExportEntry {
                id: export.id.as_str().to_string(),
                bytes: body.len() as u64,
                hash: hasher.hash(body),
                consumers: graph
                    .consumers_of_export(document, &export.id)
                    .iter()
                    .map(|d| d.as_str().to_string())
                    .collect(),
            }
        })
        .collect();
    let consumers: Vec<String> = graph
        .consumers_of(document)
        .iter()
        .map(|d| d.as_str().to_string())
        .collect();

    let mut history = super::history::History::new(services);
    let mut changes = Vec::new();
    let mut diffs = Vec::new();
    let mut decoded_budget: u64 = readme.bytes
        + files.iter().map(|f| f.bytes).sum::<u64>()
        + imports.iter().map(|i| i.bytes).sum::<u64>()
        + crate::guidance::text_bytes(&guidance.entries);
    if let Some(record) = &previous {
        let diff = record.manifest.diff(&manifest);
        for change in diff_changes(&diff) {
            changes.push(ChangeEntry {
                kind: change.kind.to_string(),
                change: change.change.to_string(),
                identity: change.identity.clone(),
                before_bytes: change.before.map(|b| b.0),
                before_hash: change.before.map(|b| b.1),
                after_bytes: change.after.map(|a| a.0),
                after_hash: change.after.map(|a| a.1),
            });
        }
        if let Some(((prev_len, prev_hash), _)) = diff.document {
            diffs.push(diff_entry(
                record,
                "README",
                document.as_str(),
                Some((prev_len, prev_hash)),
                Some(readme_bytes),
                &mut decoded_budget,
                &mut history,
            ));
        }
        for change in &diff.files {
            match change {
                InputChange::Added(f) => diffs.push(DiffEntry {
                    identity: f.path.as_str().into(),
                    status: "added".into(),
                    reason: None,
                    old_encoding: None,
                    old_body: None,
                    text: None,
                }),
                InputChange::Removed(f) => diffs.push(diff_entry(
                    record,
                    f.path.as_str(),
                    f.path.as_str(),
                    Some((f.bytes, f.hash)),
                    None,
                    &mut decoded_budget,
                    &mut history,
                )),
                InputChange::Changed { before, after } => diffs.push(diff_entry(
                    record,
                    after.path.as_str(),
                    after.path.as_str(),
                    Some((before.bytes, before.hash)),
                    Some(snapshot.file_bytes(&after.path)),
                    &mut decoded_budget,
                    &mut history,
                )),
            }
        }
        for change in &diff.imports {
            if let InputChange::Changed { after, .. } | InputChange::Removed(after) = change {
                let base = history.lookup(
                    record.git.base_commit.as_deref(),
                    after.document.as_str(),
                    Some(after.export_id.as_str()),
                    match change {
                        InputChange::Changed { before, .. } => (before.bytes, before.hash),
                        InputChange::Removed(before) => (before.bytes, before.hash),
                        InputChange::Added(_) => unreachable!(),
                    },
                );
                let text = base.bytes.as_ref().and_then(|old| {
                    match crate::diff::diff_bytes(
                        old,
                        snapshot
                            .export_body(&after.document, &after.export_id)
                            .unwrap_or(&[]),
                    ) {
                        crate::diff::DiffText::Unified(text) => Some(text),
                        crate::diff::DiffText::Identical => Some(String::new()),
                        _ => None,
                    }
                });
                decoded_budget += base.bytes.as_ref().map_or(0, |b| b.len() as u64);
                diffs.push(DiffEntry {
                    identity: format!("{}#{}", after.document, after.export_id),
                    status: if text.is_some() {
                        "available"
                    } else {
                        "unavailable"
                    }
                    .into(),
                    reason: base.reason,
                    old_encoding: base.bytes.as_ref().map(|_| ContentEncoding::Utf8),
                    old_body: base.bytes,
                    text,
                });
            }
        }
    }

    let covered: Vec<(u64, String)> = snapshot
        .state
        .active_invalidations_for(document)
        .iter()
        .map(|inv| (inv.id, inv.reason.as_str().to_string()))
        .collect();
    let review_revision = snapshot.state.document_revision(document);
    let token = compute_token(
        hasher,
        document,
        review_revision,
        &manifest,
        guidance.digest,
        &covered,
    );

    // Records are array elements across the whole envelope. The data subtree
    // is counted here; the codec adds the diagnostics array and publishes the
    // complete count in `size.record_count`.
    let provisional = FocusedReviewPacket {
        document: document.clone(),
        review_revision,
        manifest: manifest.clone(),
        covered_invalidations: covered.clone(),
        token: token.clone(),
        packet_digest: String::new(),
        content: PacketContent {
            readme: readme.clone(),
            files: files.clone(),
            imports: imports.clone(),
        },
        context: PacketContext {
            previous_review: previous.clone(),
            git: snapshot.git.clone(),
            guidance: guidance.clone(),
            exports: exports.clone(),
            consumers: consumers.clone(),
            changes: changes.clone(),
            diffs: diffs.clone(),
        },
        raw_input_bytes,
        record_count: 0,
    };
    let record_count = provisional.to_detail().list_elements();
    if record_count > MAX_RECORDS {
        return Err(refuse(format!(
            "the packet would contain {record_count} records, above the hard cap of {MAX_RECORDS}"
        )));
    }
    // Generated diff text is decoded content too; count it exactly as the
    // decoder does before emitting a token.
    decoded_budget += diffs
        .iter()
        .map(|d| d.text.as_ref().map(|t| t.len() as u64).unwrap_or(0))
        .sum::<u64>();
    if decoded_budget > MAX_DECODED_BYTES {
        return Err(refuse(format!(
            "decoded packet content would be {decoded_budget} bytes, above the hard cap of {MAX_DECODED_BYTES}"
        )));
    }

    Ok(FocusedReviewPacket {
        document: document.clone(),
        review_revision,
        manifest,
        covered_invalidations: covered,
        token,
        packet_digest: String::new(),
        content: PacketContent {
            readme,
            files,
            imports,
        },
        context: PacketContext {
            previous_review: previous,
            git: snapshot.git.clone(),
            guidance,
            exports,
            consumers,
            changes,
            diffs,
        },
        raw_input_bytes,
        record_count,
    })
}
