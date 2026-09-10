//! Change-first reading views; canonical JSON and detailed views stay separate.
use memoria_application::{
    error::Detail, packet::FocusedReviewPacket, usecases::explain::ExplainReport,
};
use std::fmt::Write as _;

pub fn packet(p: &FocusedReviewPacket) -> String {
    let mut out = format!(
        "Review required: {}\nToken           {} (reading view, not an acknowledgement packet)\n",
        p.document, p.token
    );
    if p.context.previous_review.is_none() {
        out.push_str("Reason: first review; examine the full boundary.\n");
    }
    for (_, reason) in &p.covered_invalidations {
        let _ = writeln!(out, "Semantic review: {reason}");
    }
    for c in &p.context.changes {
        let _ = writeln!(out, "{} {} {}", c.change, c.kind, c.identity);
    }
    for d in &p.context.diffs {
        let _ = writeln!(
            out,
            "Evidence {}: {}{}",
            d.identity,
            d.status,
            d.reason
                .as_deref()
                .or(match d.status.as_str() {
                    "binary" => Some("The content is binary; inspect the saved bodies."),
                    "too_large" =>
                        Some("The diff exceeds the line limit; inspect the saved bodies."),
                    "removed" => Some("The old body is saved; explain supplies deletion hunks."),
                    "added" => Some("No previous input exists; inspect the new saved body."),
                    "unavailable" => Some(
                        "No usable text hunk is available; inspect the saved history and content."
                    ),
                    _ => None,
                })
                .map(|r| format!(" — {r}"))
                .unwrap_or_default()
        );
        if let Some(text) = &d.text {
            out.push_str(text);
        }
    }
    let unchanged = p
        .content
        .files
        .iter()
        .filter(|f| {
            !p.context
                .changes
                .iter()
                .any(|c| c.kind == "file" && c.identity == f.path)
        })
        .count();
    let _ = writeln!(
        out,
        "Scope: {} files ({} unchanged), {} imports, {} raw bytes",
        p.content.files.len(),
        unchanged,
        p.content.imports.len(),
        p.raw_input_bytes
    );
    let _ = writeln!(
        out,
        "Input size      {} raw bytes in {} record(s)",
        p.raw_input_bytes, p.record_count
    );
    let _ = writeln!(
        out,
        "Guidance: {} entries, digest {} — memoria guidance {}",
        p.context.guidance.entries.len(),
        p.context.guidance.digest.to_hex(),
        p.document
    );
    let _ = writeln!(
        out,
        "Full review remains required unless the owner opts into experimental P1.\nSave canonical packet outside the project: memoria review {} --format json > /tmp/review.json\nRead saved inputs: memoria packet view /tmp/review.json --section content\nDetailed human output: memoria review {} --full",
        p.document, p.document
    );
    out
}

pub fn explain(p: &ExplainReport) -> String {
    let mut out = format!("Freshness: {}\n", p.document);
    for key in ["status", "ready", "waiting_on", "active_invalidations"] {
        if let Some(value) = p.state.get(key) {
            super::human::detail(&mut out, &Detail::map().with(key, value.clone()).build(), 0);
        }
    }
    let count = |key| match p.current_manifest.get(key) {
        Some(Detail::List(items)) => items.len(),
        _ => 0,
    };
    let _ = writeln!(
        out,
        "Scope: {} owned files, {} imports",
        count("files"),
        count("imports")
    );
    out.push_str("Changes:\n");
    super::human::detail(&mut out, &Detail::List(p.changes.clone()), 1);
    out.push_str("Verified historical evidence:\n");
    for evidence in &p.evidence {
        let mut item = Detail::map();
        for key in [
            "identity",
            "status",
            "reason_code",
            "reason",
            "baseline_verified",
            "base_commit",
            "text",
        ] {
            if let Some(value) = evidence.get(key) {
                item = item.with(key, value.clone());
            }
        }
        super::human::detail(&mut out, &item.build(), 1);
    }
    let _ = writeln!(
        out,
        "Guidance: memoria guidance {}\nReview when ready: memoria review {}\nFull explanation: memoria explain {} --full",
        p.document, p.document, p.document
    );
    out
}
