//! Shared verified Git baselines and packet-compatible hunks.
use crate::diff::{DiffText, diff_bytes};
use crate::packet::{ContentEncoding, DiffEntry};
use memoria_domain::ReviewRecord;

pub(crate) struct Baseline {
    pub bytes: Option<Vec<u8>>,
    pub commit: Option<String>,
    pub observed: Option<(u64, memoria_domain::Hash64)>,
    pub reason_code: Option<&'static str>,
    pub reason: Option<String>,
}

pub(crate) fn diff_entry(
    record: &ReviewRecord,
    identity: &str,
    path: &str,
    previous: Option<(u64, memoria_domain::Hash64)>,
    current: Option<&[u8]>,
    budget: &mut u64,
    history: &mut super::history::History<'_, '_>,
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
    let baseline = history.lookup(
        record.git.base_commit.as_deref(),
        path,
        None,
        (prev_len, prev_hash),
    );
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
