//! `memoria ack`: record a review result against its exact snapshot.

use memoria_domain::{AckError, AckRequest, InputChange, ReviewNote, ReviewResult, ReviewerName};

use crate::diff::{DiffText, diff_bytes};
use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::packet::{ContentEncoding, FocusedReviewPacket, compute_token, validate_token_text};
use crate::ports::{PacketFailure, PacketSource, Services};
use crate::snapshot::{self, Snapshot};

use super::invalidate::state_error;
use super::{acquire_lock, diff_changes, parse_document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckArgs {
    pub document: String,
    pub packet: PacketSource,
    pub token: String,
    pub reviewer: String,
    pub result: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckReport {
    pub document: String,
    pub revision: u64,
    pub result: String,
    pub reviewer: String,
    pub cleared: Vec<u64>,
    pub still_pending: Vec<(u64, String)>,
}

impl AckReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("document", self.document.clone())
            .number("revision", self.revision)
            .text("result", self.result.clone())
            .text("reviewer", self.reviewer.clone())
            .with(
                "cleared_invalidations",
                Detail::list(self.cleared.iter().map(|id| Detail::Number(*id))),
            )
            .with(
                "still_pending_invalidations",
                Detail::list(self.still_pending.iter().map(|(id, reason)| {
                    DetailMap::default()
                        .number("id", *id)
                        .text("reason", reason.clone())
                        .build()
                })),
            )
            .bool("still_pending", !self.still_pending.is_empty())
            .build()
    }
}

fn packet_error(failure: PacketFailure) -> AppError {
    match failure {
        PacketFailure::Io(err) => AppError::io("packet_unreadable", err.to_string()),
        PacketFailure::Invalid { code, message } => AppError::usage(code, message),
    }
}

/// Verify content bodies against the manifest and the token against the
/// packet's snapshot fields. Pure validation before any lock.
pub fn verify_packet(
    services: &Services<'_>,
    packet: &FocusedReviewPacket,
) -> Result<(), AppError> {
    verify_packet_with_hasher(services.hasher, packet)
}

/// Shared integrity gate for acknowledgement and offline snapshot views.
pub fn verify_packet_with_hasher(
    hasher: &dyn crate::ports::FingerprintHasher,
    packet: &FocusedReviewPacket,
) -> Result<(), AppError> {
    let manifest = &packet.manifest;
    let content = &packet.content;
    let mismatch = |what: String| AppError::usage("packet_content_mismatch", what);
    if content.readme.path != manifest.document.as_str() {
        return Err(mismatch(format!(
            "readme content path {} does not match the manifest document",
            content.readme.path
        )));
    }
    let check_body = |name: &str,
                      bytes: u64,
                      hash: memoria_domain::Hash64,
                      encoding: ContentEncoding,
                      body: &[u8]|
     -> Result<(), AppError> {
        if body.len() as u64 != bytes {
            return Err(mismatch(format!(
                "{name}: body has {} bytes but the manifest records {bytes}",
                body.len()
            )));
        }
        if hasher.hash(body) != hash {
            return Err(mismatch(format!(
                "{name}: body hash does not match the manifest"
            )));
        }
        if encoding == ContentEncoding::Utf8 && std::str::from_utf8(body).is_err() {
            return Err(mismatch(format!("{name}: utf8 body is not valid UTF-8")));
        }
        Ok(())
    };
    check_body(
        "readme",
        manifest.document_bytes,
        manifest.document_hash,
        content.readme.encoding,
        &content.readme.body,
    )?;
    if std::str::from_utf8(&content.readme.body).is_err() {
        return Err(mismatch("readme content must be valid UTF-8".into()));
    }
    if content.files.len() != manifest.files().len() {
        return Err(mismatch(format!(
            "packet has {} file bodies but the manifest lists {}",
            content.files.len(),
            manifest.files().len()
        )));
    }
    for (entry, file) in content.files.iter().zip(manifest.files()) {
        if entry.path != file.path.as_str() {
            return Err(mismatch(format!(
                "file content {} does not match manifest entry {}",
                entry.path, file.path
            )));
        }
        if entry.bytes != file.bytes || entry.hash != file.hash {
            return Err(mismatch(format!(
                "{}: content identity differs from the manifest",
                entry.path
            )));
        }
        check_body(
            &entry.path,
            file.bytes,
            file.hash,
            entry.encoding,
            &entry.body,
        )?;
    }
    if content.imports.len() != manifest.imports().len() {
        return Err(mismatch(format!(
            "packet has {} import bodies but the manifest lists {}",
            content.imports.len(),
            manifest.imports().len()
        )));
    }
    for (entry, import) in content.imports.iter().zip(manifest.imports()) {
        let name = format!("{}#{}", import.document, import.export_id);
        if entry.document != import.document.as_str()
            || entry.export_id != import.export_id.as_str()
        {
            return Err(mismatch(format!(
                "import content {}#{} does not match manifest entry {name}",
                entry.document, entry.export_id
            )));
        }
        if entry.bytes != import.bytes || entry.hash != import.hash {
            return Err(mismatch(format!(
                "{name}: content identity differs from the manifest"
            )));
        }
        check_body(
            &name,
            import.bytes,
            import.hash,
            entry.encoding,
            &entry.body,
        )?;
        if std::str::from_utf8(&entry.body).is_err() {
            return Err(mismatch(format!(
                "{name}: import content must be valid UTF-8"
            )));
        }
    }
    let expected = compute_token(
        hasher,
        &packet.document,
        packet.review_revision,
        manifest,
        packet.context.guidance.digest,
        &packet.covered_invalidations,
    );
    if expected != packet.token {
        return Err(AppError::usage(
            "packet_token_mismatch",
            "the packet token does not match its own snapshot fields",
        ));
    }
    Ok(())
}

pub fn run(services: &Services<'_>, args: &AckArgs) -> Result<Outcome<AckReport>, AppError> {
    // 1. Argument validation without I/O.
    let document = parse_document(&args.document)?;
    validate_token_text(&args.token)
        .map_err(|message| AppError::usage("token_invalid", message))?;
    let reviewer = ReviewerName::parse(&args.reviewer)
        .map_err(|err| AppError::usage("reviewer_invalid", err.to_string()))?;
    let result = ReviewResult::parse(&args.result).ok_or_else(|| {
        AppError::usage(
            "result_invalid",
            "--result must be `updated` or `no-update`",
        )
    })?;
    let note = ReviewNote::parse(&args.note)
        .map_err(|err| AppError::usage("note_invalid", err.to_string()))?;

    // 2. Packet transport, decoding, and integrity before any lock.
    let bytes = services
        .packet_input
        .read(&args.packet)
        .map_err(packet_error)?;
    let packet = services.packets.decode(&bytes).map_err(packet_error)?;
    drop(bytes);
    verify_packet(services, &packet)?;
    if packet.token != args.token {
        return Err(AppError::usage(
            "token_mismatch",
            "--token does not equal the packet token",
        ));
    }
    if packet.document != document {
        return Err(AppError::usage(
            "packet_document_mismatch",
            format!(
                "the packet describes {} but the command names {document}",
                packet.document
            ),
        ));
    }

    // 3. Lock, rebuild the snapshot, and compare.
    let _guard = acquire_lock(services)?;
    let snapshot = snapshot::build(services)?;
    snapshot.require_valid()?;
    // The packet proved the document existed at capture; in an otherwise
    // valid project its disappearance is a snapshot conflict, reported with
    // the packet baseline, never a request or project validation error.
    let Some(status) = snapshot.status_of(&document) else {
        return Err(snapshot_changed(
            &snapshot,
            &packet,
            &memoria_domain::ManifestDiff::removed(&packet.manifest),
        ));
    };
    // Changed inputs are reported with their exact differences before any
    // readiness diagnostic, so a changed import is never hidden behind
    // `dependencies_pending`.
    let current_manifest = snapshot.manifests.get(&document).cloned().ok_or_else(|| {
        AppError::validation("document_not_found", format!("{document} has no manifest"))
    })?;
    let diff = packet.manifest.diff(&current_manifest);
    if !diff.is_empty() {
        return Err(snapshot_changed(&snapshot, &packet, &diff));
    }
    // Guidance is review context, not freshness. A change after packet
    // creation invalidates the reviewed context without marking a current
    // document stale, so it is a conflict and never a state write.
    guidance_conflict(&snapshot, &packet)?;
    if status.waiting() {
        let waiting: Vec<String> = status
            .waiting_on
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        return Err(AppError::new(
            ExitClass::Conflict,
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

    let coverage = super::history::History::new(services)
        .coverage(&current_manifest, snapshot.git.base_commit.as_deref());
    let mut reviewed_git = snapshot.git.clone();
    reviewed_git.base_commit = coverage.commit.clone();

    // 4. State transition under the lock with a fresh reload.
    let loaded = services.state.load().map_err(state_error)?;
    let (mut state, expected) = match loaded {
        Some(loaded) => (loaded.state, Some(loaded.bytes)),
        None => (memoria_domain::ReviewState::empty(), None),
    };
    state.validate().map_err(|err| {
        AppError::new(
            ExitClass::Io,
            Diagnostic::error("state_corrupt", err.to_string()).at_path(snapshot::STATE_PATH),
        )
    })?;
    let input_fingerprint = services
        .hasher
        .hash(&memoria_domain::canonical::encode_inputs(&current_manifest));
    let token_digest =
        validate_token_text(&packet.token).map_err(|m| AppError::usage("token_invalid", m))?;
    services.progress.note(&format!(
        "ack: {document} revision {} -> {} ({})",
        packet.review_revision,
        packet.review_revision + 1,
        result.as_str()
    ));
    let outcome = state
        .acknowledge(AckRequest {
            document: document.clone(),
            packet_revision: packet.review_revision,
            packet_manifest: packet.manifest.clone(),
            current_manifest,
            covered: packet.covered_invalidations.clone(),
            input_fingerprint,
            token_digest,
            guidance: packet.context.guidance.digest,
            reviewed_at: services.clock.now(),
            reviewer: reviewer.clone(),
            result,
            note,
            git: reviewed_git,
        })
        .map_err(|err| match err {
            AckError::SnapshotChanged(diff) => snapshot_changed(&snapshot, &packet, &diff),
            AckError::RevisionConflict { packet: p, current } => AppError::new(
                ExitClass::Conflict,
                Diagnostic::error("revision_conflict", format!("the packet was created for document revision {p}, but the current revision is {current}; obtain a fresh packet"))
                    .at_path(document.as_str())
                    .with_details(DetailMap::default().number("packet_revision", p).number("current_revision", current).build()),
            ),
            AckError::InvalidationNotActive { id } => AppError::conflict("invalidation_not_active", format!("invalidation {id} covered by the packet is no longer active for {document}")),
            AckError::InvalidationReasonMismatch { id, .. } => AppError::conflict("invalidation_reason_mismatch", format!("invalidation {id} has a different stored reason than the packet")),
            AckError::Counter(err) => AppError::io("state_corrupt", err.to_string()),
        })?;

    // 5. Final revalidation immediately before the durable write.
    let recheck = snapshot::build(services)?;
    recheck.require_valid()?;
    // Exact differences come first, exactly as in the initial phase: a
    // changed export is reported with its identity, sizes, hashes, and
    // packet-baseline text even when it also made a provider pending.
    match recheck.manifests.get(&document) {
        Some(manifest) if manifest == &packet.manifest => {}
        Some(manifest) => {
            return Err(snapshot_changed(
                &recheck,
                &packet,
                &packet.manifest.diff(manifest),
            ));
        }
        None => {
            return Err(snapshot_changed(
                &recheck,
                &packet,
                &memoria_domain::ManifestDiff::removed(&packet.manifest),
            ));
        }
    }
    // The same guidance check runs again under the write lock.
    guidance_conflict(&recheck, &packet)?;
    if let Some(final_status) = recheck.status_of(&document)
        && final_status.waiting()
    {
        let waiting: Vec<String> = final_status
            .waiting_on
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();
        return Err(AppError::new(
            ExitClass::Conflict,
            Diagnostic::error(
                "dependencies_pending",
                format!(
                    "{document} waits for: {} (a dependency changed during acknowledgement)",
                    waiting.join(", ")
                ),
            )
            .at_path(document.as_str())
            .with_details(
                DetailMap::default()
                    .with("waiting_on", Detail::texts(waiting))
                    .build(),
            ),
        ));
    }
    services
        .state
        .save(&state, expected.as_deref())
        .map_err(state_error)?;
    let mut diagnostics = snapshot.non_error_diagnostics();
    diagnostics.push(coverage.diagnostic());
    Ok(Outcome::new(
        AckReport {
            document: document.as_str().to_string(),
            revision: outcome.revision,
            result: result.as_str().to_string(),
            reviewer: reviewer.as_str().to_string(),
            cleared: outcome.cleared,
            still_pending: outcome.still_pending,
        },
        diagnostics,
    ))
}

/// Refuse acknowledgement when the effective guidance changed after the
/// packet was created. The reviewer regenerates the packet; no review state
/// changes and the document does not become stale.
fn guidance_conflict(snapshot: &Snapshot, packet: &FocusedReviewPacket) -> Result<(), AppError> {
    let current = snapshot.guidance_of(&packet.document);
    if current.digest == packet.context.guidance.digest {
        return Ok(());
    }
    Err(AppError::new(
        ExitClass::Conflict,
        Diagnostic::error(
            "guidance_changed",
            format!(
                "project documentation guidance changed after the packet was created; run `memoria review {}` again for a fresh packet",
                packet.document
            ),
        )
        .at_path(packet.document.as_str())
        .with_details(
            DetailMap::default()
                .text("packet_digest", packet.context.guidance.digest.to_hex())
                .text("current_digest", current.digest.to_hex())
                .build(),
        ),
    ))
}

/// Explain every difference between the packet snapshot and the project.
fn snapshot_changed(
    snapshot: &Snapshot,
    packet: &FocusedReviewPacket,
    diff: &memoria_domain::ManifestDiff,
) -> AppError {
    let mut changes: Vec<Detail> = Vec::new();
    for change in diff_changes(diff) {
        let mut detail = change.to_detail();
        let text_diff = match change.kind {
            "document" => Some(diff_bytes(
                &packet.content.readme.body,
                snapshot.document_bytes(&packet.document),
            )),
            "file" => match diff
                .files
                .iter()
                .find(|c| identity_of(c) == change.identity)
            {
                Some(InputChange::Changed { after, .. }) => packet
                    .content
                    .files
                    .iter()
                    .find(|f| f.path == change.identity)
                    .map(|f| diff_bytes(&f.body, snapshot.file_bytes(&after.path))),
                Some(InputChange::Removed(_)) => packet
                    .content
                    .files
                    .iter()
                    .find(|f| f.path == change.identity)
                    .map(|f| diff_bytes(&f.body, b"")),
                _ => None,
            },
            "import" => {
                // The packet carries the exact export bytes it was built from.
                let old = packet
                    .content
                    .imports
                    .iter()
                    .find(|i| format!("{}#{}", i.document, i.export_id) == change.identity)
                    .map(|i| i.body.as_slice());
                let current = diff
                    .imports
                    .iter()
                    .find_map(|c| match c {
                        InputChange::Changed { after, .. }
                            if format!("{}#{}", after.document, after.export_id)
                                == change.identity =>
                        {
                            snapshot.export_body(&after.document, &after.export_id)
                        }
                        _ => None,
                    })
                    .unwrap_or(&[]);
                old.map(|old| diff_bytes(old, current))
            }
            _ => None,
        };
        if let (Some(text), Detail::Map(map)) = (text_diff, &mut detail) {
            let value = match text {
                DiffText::Unified(text) => Detail::Text(text),
                DiffText::Identical => Detail::Text(String::new()),
                DiffText::Binary => Detail::Text("(binary content differs)".into()),
                DiffText::TooLarge => Detail::Text("(diff omitted: too many lines)".into()),
            };
            map.insert("diff".into(), value);
        }
        changes.push(detail);
    }
    let summary: Vec<String> = diff_changes(diff)
        .iter()
        .map(|c| format!("{} {} {}", c.change, c.kind, c.identity))
        .collect();
    AppError::new(
        ExitClass::Conflict,
        Diagnostic::error(
            "snapshot_changed",
            format!(
                "the review inputs changed after the packet was created: {}; obtain a fresh packet",
                summary.join("; ")
            ),
        )
        .at_path(packet.document.as_str())
        .with_details(
            DetailMap::default()
                .with("changes", Detail::List(changes))
                .build(),
        ),
    )
}

fn identity_of(change: &InputChange<memoria_domain::FileInput>) -> String {
    match change {
        InputChange::Added(f) | InputChange::Removed(f) => f.path.as_str().to_string(),
        InputChange::Changed { after, .. } => after.path.as_str().to_string(),
    }
}
