//! Shared verified Git baselines and packet-compatible hunks.
use crate::diff::{DiffText, diff_bytes};
use crate::packet::{ContentEncoding, DiffEntry};
use crate::ports::Services;
use memoria_domain::ReviewRecord;

pub(crate) struct Baseline {
    pub bytes: Option<Vec<u8>>,
    pub observed: Option<(u64, memoria_domain::Hash64)>,
    pub reason_code: Option<&'static str>,
    pub reason: Option<String>,
}

pub(crate) fn baseline(
    services: &Services<'_>,
    record: &ReviewRecord,
    path: &str,
    expected: (u64, memoria_domain::Hash64),
) -> Baseline {
    let unavailable = |code, reason| Baseline {
        bytes: None,
        observed: None,
        reason_code: Some(code),
        reason: Some(reason),
    };
    let Some(commit) = record.git.base_commit.as_deref() else {
        return unavailable(
            "no_base_commit",
            "the previous review recorded no base commit".into(),
        );
    };
    let old = match services.git.read_blob(commit, path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return unavailable(
                "blob_unavailable",
                format!("{path} is not available at commit {commit}"),
            );
        }
        Err(err) => return unavailable("git_read_failed", err.to_string()),
    };
    let observed = (old.len() as u64, services.hasher.hash(&old));
    if observed != expected {
        return Baseline {
            bytes: None,
            observed: Some(observed),
            reason_code: Some("reviewed_bytes_mismatch"),
            reason: Some(format!(
                "{path} at commit {commit} does not match the reviewed hash; the prior snapshot was not committed"
            )),
        };
    }
    Baseline {
        bytes: Some(old),
        observed: Some(observed),
        reason_code: None,
        reason: None,
    }
}

pub(crate) fn diff_entry(
    services: &Services<'_>,
    record: &ReviewRecord,
    identity: &str,
    path: &str,
    previous: Option<(u64, memoria_domain::Hash64)>,
    current: Option<&[u8]>,
    budget: &mut u64,
) -> DiffEntry {
    let status_removed = current.is_none();
    let Some((prev_len, prev_hash)) = previous else {
        return DiffEntry {
            identity: identity.into(),
            status: "added".into(),
            reason: None,
            old_encoding: None,
            old_body: None,
            text: None,
        };
    };
    let baseline = baseline(services, record, path, (prev_len, prev_hash));
    let Some(old) = baseline.bytes else {
        return unavailable(
            identity,
            baseline.reason.as_deref().unwrap_or("baseline unavailable"),
        );
    };
    // Available content is never relabeled to fit a budget: the producer
    // refuses the whole packet when the decoded total exceeds the cap.
    *budget += old.len() as u64;
    let encoding = ContentEncoding::for_bytes(&old);
    if status_removed {
        return DiffEntry {
            identity: identity.into(),
            status: "removed".into(),
            reason: None,
            old_encoding: Some(encoding),
            old_body: Some(old),
            text: None,
        };
    }
    let current = current.unwrap_or(&[]);
    let (status, text) = match diff_bytes(&old, current) {
        DiffText::Unified(text) => ("available", Some(text)),
        DiffText::Identical => ("available", Some(String::new())),
        DiffText::Binary => ("binary", None),
        DiffText::TooLarge => ("too_large", None),
    };
    DiffEntry {
        identity: identity.into(),
        status: status.into(),
        reason: None,
        old_encoding: Some(encoding),
        old_body: Some(old),
        text,
    }
}

fn unavailable(identity: &str, reason: &str) -> DiffEntry {
    DiffEntry {
        identity: identity.into(),
        status: "unavailable".into(),
        reason: Some(reason.to_string()),
        old_encoding: None,
        old_body: None,
        text: None,
    }
}
