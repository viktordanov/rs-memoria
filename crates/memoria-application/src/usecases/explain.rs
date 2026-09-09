//! Read-only whole-file freshness, independent of review readiness.
use super::{diff_changes, document_status_detail, parse_document};
use crate::error::{AppError, Detail, DetailMap, Outcome};
use crate::packet::{MAX_DECODED_BYTES, MAX_RECORDS, manifest_detail};
use crate::ports::Services;
use crate::snapshot;

pub const MAX_RENDERED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainReport {
    pub document: String,
    pub state: Detail,
    pub changes: Vec<Detail>,
    pub policy: Detail,
    pub guidance: Detail,
    pub evidence: Vec<Detail>,
    pub before_manifest: Detail,
    pub current_manifest: Detail,
}

impl ExplainReport {
    pub fn to_detail(&self) -> Detail {
        let Detail::Map(mut map) = self.state.clone() else {
            unreachable!()
        };
        map.insert("kind".into(), Detail::text("freshness_explanation"));
        map.insert("changes".into(), Detail::List(self.changes.clone()));
        map.insert("policy".into(), self.policy.clone());
        map.insert("guidance".into(), self.guidance.clone());
        map.insert("evidence".into(), Detail::List(self.evidence.clone()));
        map.insert("before_manifest".into(), self.before_manifest.clone());
        map.insert("current_manifest".into(), self.current_manifest.clone());
        Detail::Map(map)
    }
}

fn limit(decoded: u64, records: u64, rendered: u64) -> Result<(), AppError> {
    if decoded > MAX_DECODED_BYTES || records > MAX_RECORDS || rendered > MAX_RENDERED_BYTES {
        return Err(AppError::validation(
            "explain_limit_exceeded",
            "The explanation exceeds its evidence or output limit.",
        )
        .with_data(
            DetailMap::default()
                .number("decoded_bytes", decoded)
                .number("records", records)
                .number("rendered_bytes", rendered)
                .build(),
        ));
    }
    Ok(())
}

pub fn run(
    services: &Services<'_>,
    raw_document: &str,
) -> Result<Outcome<ExplainReport>, AppError> {
    let document = parse_document(raw_document)?;
    let snapshot = snapshot::build(services)?;
    snapshot.require_valid()?;
    let status = snapshot
        .statuses
        .iter()
        .find(|s| s.document == document)
        .ok_or_else(|| {
            AppError::validation(
                "document_not_found",
                format!("{document} is not a discovered README"),
            )
        })?;
    let manifest = &snapshot.manifests[&document];
    let previous = snapshot.state.reviews.get(&document);
    let mut state = document_status_detail(status);
    if let Detail::Map(map) = &mut state {
        map.insert(
            "previous_review_revision".into(),
            previous
                .map(|r| Detail::Number(r.revision))
                .unwrap_or(Detail::Null),
        );
        map.insert(
            "before_fingerprint".into(),
            Detail::option_text(previous.map(|r| r.input_fingerprint.to_hex())),
        );
        map.insert(
            "current_fingerprint".into(),
            Detail::text(
                services
                    .hasher
                    .hash(&memoria_domain::canonical::encode_inputs(manifest))
                    .to_hex(),
            ),
        );
        map.insert(
            "active_invalidations".into(),
            Detail::list(
                snapshot
                    .state
                    .active_invalidations_for(&document)
                    .iter()
                    .map(|inv| {
                        DetailMap::default()
                            .number("id", inv.id)
                            .text("scope", inv.scope.to_string())
                            .text("reason", inv.reason.as_str())
                            .text("created_at", inv.created_at.0.clone())
                            .build()
                    }),
            ),
        );
    }
    let changes = previous
        .map(|record| diff_changes(&record.manifest.diff(manifest)))
        .unwrap_or_default();
    let mut evidence = Vec::new();
    if previous.is_none() {
        evidence.push(
            DetailMap::default()
                .text("identity", document.as_str())
                .text("status", "unavailable")
                .text("reason_code", "no_previous_review")
                .text(
                    "reason",
                    "There is no previous review or verified prior Git baseline.",
                )
                .bool("baseline_verified", false)
                .with("base_commit", Detail::Null)
                .with("expected_bytes", Detail::Null)
                .with("expected_hash", Detail::Null)
                .with("observed_bytes", Detail::Null)
                .with("observed_hash", Detail::Null)
                .with("text", Detail::Null)
                .build(),
        );
    }
    let mut decoded = 0;
    for change in &changes {
        if change.kind == "policy" {
            continue;
        }
        let mut item = DetailMap::default()
            .text("identity", &change.identity)
            .with(
                "base_commit",
                Detail::option_text(previous.and_then(|r| r.git.base_commit.clone())),
            )
            .with(
                "expected_bytes",
                change
                    .before
                    .map(|v| Detail::Number(v.0))
                    .unwrap_or(Detail::Null),
            )
            .with(
                "expected_hash",
                Detail::option_text(change.before.map(|v| v.1.to_hex())),
            );
        let mut observed = None;
        let mut verified = false;
        let mut text = None;
        let (status, mut code, mut reason) = if change.kind == "import" {
            (
                "unavailable",
                Some("import_content_not_stored"),
                Some("Previous export content is not stored.".to_string()),
            )
        } else if let (Some(record), Some(expected)) = (previous, change.before) {
            let path = if change.kind == "document" {
                document.as_str()
            } else {
                &change.identity
            };
            let base = super::evidence::baseline(services, record, path, expected);
            observed = base.observed;
            decoded += observed.map(|v| v.0).unwrap_or(0);
            limit(decoded, evidence.len() as u64, 0)?;
            if let Some(old) = base.bytes {
                verified = true;
                let current = if change.after.is_none() {
                    &[]
                } else if change.kind == "document" {
                    snapshot.document_bytes(&document)
                } else {
                    snapshot
                        .collected
                        .file_bytes
                        .iter()
                        .find(|(p, _)| p.as_str() == path)
                        .map(|(_, b)| b.as_slice())
                        .unwrap_or(&[])
                };
                decoded += current.len() as u64;
                limit(decoded, evidence.len() as u64, 0)?;
                match crate::diff::diff_bytes(&old, current) {
                    crate::diff::DiffText::Unified(hunk) => {
                        decoded += hunk.len() as u64;
                        text = Some(hunk);
                        ("available", None, None)
                    }
                    crate::diff::DiffText::Identical => {
                        text = Some(String::new());
                        ("available", None, None)
                    }
                    crate::diff::DiffText::Binary => (
                        "unavailable",
                        Some("binary_content"),
                        Some("The previous or current bytes are not UTF-8 text.".into()),
                    ),
                    crate::diff::DiffText::TooLarge => (
                        "unavailable",
                        Some("diff_line_limit"),
                        Some("A side exceeds the existing Git hunk line limit.".into()),
                    ),
                }
            } else {
                let reason = if base.reason_code == Some("git_read_failed") {
                    Some("Local Git evidence could not be read.".into())
                } else {
                    base.reason
                };
                ("unavailable", base.reason_code, reason)
            }
        } else {
            (
                "unavailable",
                Some("added_file"),
                Some("The prior manifest has no bytes for this file.".into()),
            )
        };
        if verified && change.kind == "file" && change.after.is_none() {
            code = Some("removed_file");
            let mut context = "This input was removed from the requested boundary. The existing review packet emits no deletion hunk; this explanation retains verified deletion evidence.".to_string();
            if let Some(unavailable) = reason {
                context.push(' ');
                context.push_str(&unavailable);
            }
            reason = Some(context);
        }
        item = item
            .text("status", status)
            .with("reason_code", Detail::option_text(code))
            .with("reason", Detail::option_text(reason))
            .bool("baseline_verified", verified)
            .with(
                "observed_bytes",
                observed
                    .map(|v| Detail::Number(v.0))
                    .unwrap_or(Detail::Null),
            )
            .with(
                "observed_hash",
                Detail::option_text(observed.map(|v| v.1.to_hex())),
            )
            .with("text", Detail::option_text(text));
        evidence.push(item.build());
        limit(decoded, evidence.len() as u64, 0)?;
    }
    let policy = &snapshot.policies[&document];
    let rule = |bytes: &[u8]| {
        let encoding = crate::packet::ContentEncoding::for_bytes(bytes);
        DetailMap::default()
            .text("encoding", encoding.as_str())
            .with("body", crate::packet::body_detail(encoding, bytes))
            .build()
    };
    let policy = DetailMap::default()
        .with(
            "before_hash",
            Detail::option_text(previous.map(|r| r.manifest.policy_hash.to_hex())),
        )
        .text("current_hash", manifest.policy_hash.to_hex())
        .with(
            "changed",
            previous
                .map(|r| Detail::Bool(r.manifest.policy_hash != manifest.policy_hash))
                .unwrap_or(Detail::Null),
        )
        .text("prior_scopes_unavailable_reason", "not_stored")
        .with(
            "git_scopes",
            Detail::list(policy.git_scopes.iter().map(|scope| {
                DetailMap::default()
                    .text("identity", &scope.identity)
                    .with(
                        "patterns",
                        Detail::list(scope.patterns.iter().map(|b| rule(b))),
                    )
                    .build()
            })),
        )
        .with(
            "memoria_scopes",
            Detail::list(policy.memoria_scopes.iter().map(|scope| {
                DetailMap::default()
                    .text("scope", scope.scope.as_str())
                    .with("ignore", Detail::texts(scope.ignore.clone()))
                    .with("include", Detail::texts(scope.include.clone()))
                    .build()
            })),
        )
        .build();
    let guidance = super::guidance::report_for(&snapshot, &document);
    let guidance = DetailMap::default()
        .with(
            "previous_digest",
            Detail::option_text(guidance.reviewed_digest),
        )
        .text("current_digest", guidance.digest)
        .bool("present", !guidance.entries.is_empty())
        .with(
            "changed",
            guidance
                .changed_since_review
                .map(Detail::Bool)
                .unwrap_or(Detail::Null),
        )
        .with("sources", Detail::texts(guidance.sources))
        .bool("advisory", true)
        .text("previous_prose_unavailable_reason", "not_stored")
        .text("command", format!("memoria guidance {document}"))
        .build();
    let report = ExplainReport {
        document: document.to_string(),
        state,
        changes: changes.iter().map(|c| c.to_detail()).collect(),
        policy,
        guidance,
        evidence,
        before_manifest: previous
            .map(|r| manifest_detail(&r.manifest))
            .unwrap_or(Detail::Null),
        current_manifest: manifest_detail(manifest),
    };
    let detail = report.to_detail();
    let diagnostics = snapshot.non_error_diagnostics();
    let envelope = services
        .packets
        .envelope_size("explain", true, &detail, &diagnostics);
    limit(decoded, envelope.records, envelope.serialized_bytes)?;
    Ok(Outcome::new(report, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_evidence_limits_are_inclusive() {
        assert!(limit(MAX_DECODED_BYTES, MAX_RECORDS, MAX_RENDERED_BYTES).is_ok());
        for (bytes, records, rendered) in [
            (MAX_DECODED_BYTES + 1, 0, 0),
            (0, MAX_RECORDS + 1, 0),
            (0, 0, MAX_RENDERED_BYTES + 1),
        ] {
            let error = limit(bytes, records, rendered).unwrap_err();
            assert_eq!(error.class.code(), 1);
            assert_eq!(error.diagnostics[0].code, "explain_limit_exceeded");
            assert!(error.data.get("decoded_bytes").is_some());
        }
    }
}
