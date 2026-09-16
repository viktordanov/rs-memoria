//! The default manifest view and the freshness explanation.
//!
//! Canonical JSON and the detailed full export stay in their own modules.
use memoria_application::review::{ReviewManifest, ReviewMode};
use memoria_application::{error::Detail, usecases::explain::ExplainReport};
use std::fmt::Write as _;

/// The default human view: what to read, and why it cannot be less.
///
/// It carries no file content. Ordinary file tools supply the reading.
pub fn manifest(m: &ReviewManifest) -> String {
    let mut out = format!(
        "Review required: {}\nToken           {}\n",
        m.document, m.token
    );
    match &m.baseline {
        None => out.push_str("Baseline: none; this is a first review of the complete boundary.\n"),
        Some(baseline) => {
            let _ = writeln!(
                out,
                "Baseline: revision {} by {} ({}), evidence {}",
                baseline.revision,
                baseline.reviewer,
                baseline.result,
                baseline.evidence_status.as_str()
            );
        }
    }
    for (id, reason) in &m.covered_invalidations {
        let _ = writeln!(out, "Semantic review [{id}]: {reason}");
    }
    for c in &m.changes {
        let _ = writeln!(out, "{} {} {}", c.change, c.kind, c.identity);
    }
    match m.mode {
        ReviewMode::FocusedCandidate => {
            out.push_str(
                "Scope: focused candidate. Eligibility only; it does not certify the prior review.\n",
            );
            for section in &m.sections {
                let _ = writeln!(
                    out,
                    "  Section {} \"{}\" lines {}-{}: {}",
                    section.id,
                    section.heading,
                    section.first_line,
                    section.last_line,
                    section.sources.join(", ")
                );
            }
        }
        ReviewMode::FullBaseline => {
            out.push_str("Scope: full baseline. Read the complete current boundary.\n");
            for reason in &m.fallback_reasons {
                let _ = writeln!(
                    out,
                    "  {} [{}]: {}",
                    reason.code,
                    reason.identity.as_deref().unwrap_or("-"),
                    reason.message
                );
            }
        }
    }
    out.push_str("Read:\n");
    for input in &m.inputs {
        let identity = match &input.export_id {
            Some(export) => format!("{}#{}", input.path, export),
            None => input.path.clone(),
        };
        let _ = writeln!(
            out,
            "  {} ({}, {} bytes)",
            identity,
            input.role.as_str(),
            input.bytes
        );
    }
    let _ = writeln!(
        out,
        "Whole README pass: required at hash {}",
        m.inputs
            .first()
            .map(|i| i.hash.to_hex())
            .unwrap_or_default()
    );
    let _ = writeln!(
        out,
        "Boundary: {} selected files, {} imports, {} raw bytes",
        m.selected_files, m.imports, m.raw_input_bytes
    );
    let _ = writeln!(
        out,
        "Guidance: {} references, digest {}{} — memoria guidance {}",
        m.guidance_references.len(),
        m.guidance_digest.to_hex(),
        match m.guidance_changed_since_review {
            Some(true) => ", changed since the last review",
            _ => "",
        },
        m.document
    );
    for step in memoria_application::review::WORKFLOW_STEPS {
        let _ = writeln!(out, "  - {step}");
    }
    let _ = writeln!(
        out,
        "Save this manifest outside the project: memoria review {} --format json > /tmp/review.json\nRead the listed paths with ordinary file tools; ranges above are hints, not the reviewed state.\nAfter any edit, obtain a fresh manifest and reconcile before `memoria ack`.\nFull offline export: memoria review {} --full --format json",
        m.document, m.document
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
