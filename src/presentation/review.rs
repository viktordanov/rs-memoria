//! The default manifest view and the freshness explanation.
//!
//! Canonical JSON and the detailed full export stay in their own modules.
use memoria_application::review::{
    EvidenceStatus, Relationship, RelationshipKind, ReviewManifest, ReviewMode,
};
use memoria_application::{error::Detail, usecases::explain::ExplainReport};
use std::fmt::Write as _;

/// Most lines a bounded human list shows before it points at `memoria graph`.
const MAX_HUMAN_LINES: usize = 10;

fn folder(subtree: &str) -> String {
    if subtree.is_empty() {
        "./".to_string()
    } else {
        format!("{subtree}/")
    }
}

/// "a, b and c" for a short list of phrases.
fn join_phrases(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn bounded_tail(out: &mut String, shown: usize, total: u64) {
    if total > shown as u64 {
        let _ = writeln!(
            out,
            "  and {} more; `memoria graph` lists all",
            total - shown as u64
        );
    }
}

/// The phrase that says how one change relates to the reviewed document.
fn relationship_phrase(change: &str, relationship: &Relationship, handoffs: &[String]) -> String {
    let mut phrase = match relationship.kind {
        RelationshipKind::OwnText => "this document's own text".to_string(),
        RelationshipKind::SelectionPolicy => "selection policy".to_string(),
        RelationshipKind::Import => format!(
            "import of {}#{}",
            relationship.provider.as_deref().unwrap_or("?"),
            relationship.export_id.as_deref().unwrap_or("?")
        ),
        RelationshipKind::Handoff if change == "removed" => match handoffs {
            [] => "left the scope: handed off".to_string(),
            targets => format!("left the scope: handed off to {}", targets.join(" or ")),
        },
        RelationshipKind::Handoff => "entered the scope: a handoff no longer applies".to_string(),
        RelationshipKind::CoverageUnrecorded => format!(
            "entered the scope; former coverage not recorded ({})",
            relationship.unrecorded_reason.as_deref().unwrap_or("?")
        ),
        RelationshipKind::ScopeSource => match relationship.sections.as_slice() {
            [] => "scope source · no section of this document describes it".to_string(),
            [one] => format!("scope source · section \"{one}\" describes it"),
            many => format!(
                "scope source · sections {} describe it",
                many.iter()
                    .map(|id| format!("\"{id}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    };
    if relationship.also_covered_by_total > 0 {
        let _ = write!(
            phrase,
            " · {} covered by {} other document{}",
            if change == "removed" { "now" } else { "also" },
            relationship.also_covered_by_total,
            if relationship.also_covered_by_total == 1 {
                ""
            } else {
                "s"
            }
        );
    }
    phrase
}

/// The default human view: what changed, how it relates to the document,
/// what to read, and the next three steps. It carries no file content.
///
/// `details` adds tokens, digests, per-input sizes and hashes, and counts.
pub fn manifest(m: &ReviewManifest, details: bool) -> String {
    let mut out = String::new();
    // 1. Header.
    let state = match &m.baseline {
        None => "never reviewed".to_string(),
        Some(_) if m.changes.is_empty() && m.covered_invalidations.is_empty() => {
            format!("current at revision {}", m.review_revision)
        }
        Some(baseline) => format!("pending since revision {}", baseline.revision),
    };
    let _ = writeln!(
        out,
        "Review {} — {}, {}",
        m.document,
        m.document_kind().label(),
        state
    );
    // 2. Scope.
    let dir = m.document.directory();
    let mut scope = format!(
        "Scope: {} source{} in {} and below",
        m.scope.files,
        if m.scope.files == 1 { "" } else { "s" },
        folder(dir.as_str())
    );
    if !m.scope.handoffs.is_empty() {
        let mut by_subtree: Vec<(String, Vec<String>)> = Vec::new();
        for handoff in &m.scope.handoffs {
            let phrase = format!("{}, line {}", handoff.via, handoff.line);
            match by_subtree.iter_mut().find(|(s, _)| s == &handoff.subtree) {
                Some((_, list)) => list.push(phrase),
                None => by_subtree.push((handoff.subtree.clone(), vec![phrase])),
            }
        }
        let phrases: Vec<String> = by_subtree
            .iter()
            .map(|(subtree, list)| format!("{} ({})", folder(subtree), list.join("; ")))
            .collect();
        let _ = write!(scope, "; hands off {}", join_phrases(&phrases));
        if m.scope.handoffs_total > m.scope.handoffs.len() as u64 {
            let _ = write!(
                scope,
                " and {} more",
                m.scope.handoffs_total - m.scope.handoffs.len() as u64
            );
        }
    }
    out.push_str(&scope);
    out.push('\n');
    if !m.scope.handed_off_by.is_empty() {
        let parents: Vec<String> = m
            .scope
            .handed_off_by
            .iter()
            .map(|h| format!("{} ({}, line {})", h.parent, h.via, h.line))
            .collect();
        let _ = writeln!(
            out,
            "Handed {} by {}",
            folder(dir.as_str()),
            join_phrases(&parents)
        );
    }
    // 3. Baseline.
    match &m.baseline {
        None => out.push_str("Baseline: none; this is a first review of the complete scope.\n"),
        Some(baseline) => {
            let _ = write!(
                out,
                "Baseline: revision {} by {} ({})",
                baseline.revision, baseline.reviewer, baseline.result
            );
            if details || baseline.evidence_status != EvidenceStatus::Verified {
                let _ = write!(out, ", evidence {}", baseline.evidence_status.as_str());
            }
            out.push('\n');
        }
    }
    // 4. What changed.
    if !m.changes.is_empty() {
        out.push_str("What changed since that review:\n");
        for (change, relationship) in m.changes.iter().zip(&m.relationships) {
            let targets: Vec<String> = m
                .scope
                .handoffs
                .iter()
                .filter(|h| {
                    change.identity.starts_with(&format!("{}/", h.subtree)) || h.subtree.is_empty()
                })
                .map(|h| h.target.clone())
                .collect();
            let subject = match relationship.kind {
                RelationshipKind::OwnText | RelationshipKind::SelectionPolicy => String::new(),
                RelationshipKind::Import => String::new(),
                _ => format!("{} · ", change.identity),
            };
            let _ = writeln!(
                out,
                "  {:<8} {subject}{}",
                change.change,
                relationship_phrase(&change.change, relationship, &targets)
            );
        }
    }
    // 5. Semantic review requests.
    if !m.covered_invalidations.is_empty() {
        out.push_str("Semantic review requests:\n");
        for (id, reason) in &m.covered_invalidations {
            let _ = writeln!(out, "  [{id}] {reason}");
        }
    }
    // 6. Co-covering documents.
    if !m.downstream.co_covering.is_empty() {
        out.push_str("Also pending for the same changes:\n");
        for other in m.downstream.co_covering.iter().take(MAX_HUMAN_LINES) {
            let relation = if other.document.rsplit_once('/').map(|(d, _)| d)
                == m.document.as_str().rsplit_once('/').map(|(d, _)| d)
            {
                "covers the same folder"
            } else {
                "also covers a changed source"
            };
            let _ = writeln!(out, "  {} ({relation}; {})", other.document, other.status);
        }
        bounded_tail(
            &mut out,
            m.downstream.co_covering.len().min(MAX_HUMAN_LINES),
            m.downstream.co_covering_total,
        );
    }
    // 7. Downstream export consumers.
    if !m.downstream.consumers.is_empty() {
        out.push_str("Downstream:\n");
        for consumer in m.downstream.consumers.iter().take(MAX_HUMAN_LINES) {
            let _ = writeln!(
                out,
                "  export {} → {}{}",
                consumer.export_id,
                consumer.consumer,
                if consumer.waits_for_this_document {
                    " (waits for this review)"
                } else {
                    ""
                }
            );
        }
        bounded_tail(
            &mut out,
            m.downstream.consumers.len().min(MAX_HUMAN_LINES),
            m.downstream.consumers_total,
        );
    }
    // 8. How to read.
    out.push_str("How to read:\n");
    match m.mode {
        ReviewMode::FocusedCandidate => {
            out.push_str(
                "  Mode: focused candidate. Eligibility only; it does not certify the prior review.\n",
            );
            for section in &m.sections {
                let _ = writeln!(
                    out,
                    "  Suggested section {} \"{}\" lines {}-{}: {}",
                    section.id,
                    section.heading,
                    section.first_line,
                    section.last_line,
                    section.sources.join(", ")
                );
            }
        }
        ReviewMode::FullBaseline => {
            out.push_str("  Mode: full baseline. Read the complete current scope.\n");
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
    out.push_str("  Read:\n");
    for input in &m.inputs {
        let identity = match &input.export_id {
            Some(export) => format!("{}#{}", input.path, export),
            None => input.path.clone(),
        };
        if details {
            let _ = writeln!(
                out,
                "    {} ({}, {} bytes, {})",
                identity,
                input.role.as_str(),
                input.bytes,
                input.hash.to_hex()
            );
        } else {
            let _ = writeln!(out, "    {} ({})", identity, input.role.as_str());
        }
    }
    out.push_str("  Whole document pass: required\n");
    let _ = writeln!(
        out,
        "  Guidance: memoria guidance {}{}",
        m.document,
        match m.guidance_changed_since_review {
            Some(true) => " (changed since the last review)",
            _ => "",
        }
    );
    if details {
        out.push_str("Details:\n");
        let _ = writeln!(out, "  Token             {}", m.token);
        let _ = writeln!(out, "  Review revision   {}", m.review_revision);
        let _ = writeln!(
            out,
            "  Inputs digest     {}",
            m.snapshot.inputs_digest.to_hex()
        );
        let _ = writeln!(
            out,
            "  Context digest    {}",
            m.snapshot.context_digest.to_hex()
        );
        let _ = writeln!(
            out,
            "  Baseline digest   {}",
            m.snapshot.baseline_digest.to_hex()
        );
        let _ = writeln!(out, "  Guidance digest   {}", m.guidance_digest.to_hex());
        let _ = writeln!(out, "  Selection version {}", m.snapshot.selection_version);
        let _ = writeln!(
            out,
            "  Counts            {} scope files, {} imports, {} raw input bytes, {} suggested sources, {} handoffs",
            m.scope_files,
            m.imports,
            m.raw_input_bytes,
            m.suggested_sources(),
            m.scope.handoffs_total
        );
        if let Some(baseline) = &m.baseline {
            let _ = writeln!(
                out,
                "  Baseline token    {}",
                baseline.token_digest.to_hex()
            );
        }
        for step in memoria_application::review::WORKFLOW_STEPS {
            let _ = writeln!(out, "  Workflow: {step}");
        }
    }
    // 9. Next.
    let _ = writeln!(
        out,
        "Next:\n  1. Read the whole document and the listed inputs; edit {doc} if it is wrong.\n  2. After any edit, save a fresh artifact: dir=$(mktemp -d); memoria review {doc} --save \"$dir\"\n  3. Record the result: memoria ack {doc} --packet <saved file> --reviewer <you> --result <updated|no-update> --note \"<why>\"",
        doc = m.document
    );
    // 10. Details pointer.
    if !details {
        let _ = writeln!(out, "Details: memoria review {} --details", m.document);
    }
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
    let handoffs: Vec<String> = match p.scope.get("handoffs") {
        Some(Detail::List(items)) => items
            .iter()
            .map(|h| {
                let text = |key: &str| match h.get(key) {
                    Some(Detail::Text(t)) => t.clone(),
                    Some(Detail::Number(n)) => n.to_string(),
                    _ => String::new(),
                };
                format!(
                    "{}/ ({}, line {})",
                    text("subtree"),
                    text("via"),
                    text("line")
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let dir = match p.document.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/"),
        None => "./".to_string(),
    };
    let mut scope = format!(
        "Scope: {} sources in {dir} and below, {} imports",
        count("files"),
        count("imports")
    );
    if !handoffs.is_empty() {
        let _ = write!(scope, "; hands off {}", handoffs.join(", "));
    }
    let _ = writeln!(out, "{scope}");
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
