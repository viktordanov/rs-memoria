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
/// Most hunk lines the default view shows for one change.
const MAX_HUNK_LINES: usize = 40;
/// Most hunk lines the default view shows in total.
const MAX_TOTAL_HUNK_LINES: usize = 160;

/// Remaining hunk lines in one bounded view.
pub struct HunkBudget {
    remaining: usize,
    unbounded: bool,
    /// Whether any hunk was cut.
    pub cut: bool,
}

impl HunkBudget {
    pub fn new(unbounded: bool) -> HunkBudget {
        HunkBudget {
            remaining: MAX_TOTAL_HUNK_LINES,
            unbounded,
            cut: false,
        }
    }
}

fn text_field<'a>(item: &'a Detail, key: &str) -> Option<&'a str> {
    match item.get(key) {
        Some(Detail::Text(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// Write the verified hunk for one changed input under its change line.
///
/// The hunk is the exact text between the reviewed bytes and the current
/// bytes. When Git no longer holds the reviewed bytes, one line gives the
/// reason code and the reason, so the reader knows to read the whole input.
/// The review view skips `added_file`, because its change line already says
/// that the input is new; `explain` names it.
fn hunk(
    out: &mut String,
    identity: &str,
    evidence: &[Detail],
    budget: &mut HunkBudget,
    name_added: bool,
) {
    let Some(item) = evidence
        .iter()
        .find(|item| text_field(item, "identity") == Some(identity))
    else {
        return;
    };
    let status = text_field(item, "status");
    if let (Some("available"), Some(text)) = (status, text_field(item, "text"))
        && !text.is_empty()
    {
        let lines: Vec<&str> = text.lines().collect();
        let limit = if budget.unbounded {
            lines.len()
        } else {
            lines.len().min(MAX_HUNK_LINES).min(budget.remaining)
        };
        for line in &lines[..limit] {
            let _ = writeln!(out, "      {line}");
        }
        if !budget.unbounded {
            budget.remaining -= limit;
        }
        if limit < lines.len() {
            budget.cut = true;
            let _ = writeln!(out, "      … {} more hunk lines", lines.len() - limit);
        }
    }
    let Some(code) = text_field(item, "reason_code") else {
        return;
    };
    if code == "added_file" && !name_added {
        return;
    }
    let label = if status == Some("available") {
        "Note"
    } else {
        "No hunk"
    };
    let _ = writeln!(
        out,
        "      {label} ({code}): {}",
        text_field(item, "reason").unwrap_or("")
    );
}

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
/// `evidence` holds the verified hunks from the snapshot that produced the
/// artifact. The default view bounds them; `details` shows them complete.
pub fn manifest(m: &ReviewManifest, details: bool, evidence: &[Detail]) -> String {
    let mut budget = HunkBudget::new(details);
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
            hunk(&mut out, &change.identity, evidence, &mut budget, false);
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
                    "  Suggested section {} \"{}\" lines {}-{}: {}{}",
                    section.id,
                    section.heading,
                    section.first_line,
                    section.last_line,
                    section.sources.join(", "),
                    match &section.guidance {
                        Some(guide) => format!(" · guide: {guide}"),
                        None => String::new(),
                    }
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
    let section_guides: Vec<&str> = m
        .guidance_references
        .iter()
        .filter(|r| r.kind == "section")
        .map(|r| r.source.as_str())
        .collect();
    let _ = writeln!(
        out,
        "  Guidance: memoria guidance {}{}{}",
        m.document,
        guidance_layers(m.guidance_references.len(), section_guides.len()),
        match m.guidance_changed_since_review {
            Some(true) => " (changed since the last review; apply the current text)",
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
        if !section_guides.is_empty() {
            let _ = writeln!(
                out,
                "  Section guides    {}",
                capped_list(&section_guides, MAX_GUIDE_NAMES)
            );
        }
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
    // 10. Details pointer, only when this view left something out.
    if budget.cut {
        let _ = writeln!(
            out,
            "Complete hunks: memoria review {} --details",
            m.document
        );
    }
    out
}

/// The default freshness explanation: state, scope, each change with its
/// verified hunk, and the one next step that fits the state.
pub fn explain(p: &ExplainReport) -> String {
    let status = text_field(&p.state, "status").unwrap_or("unknown");
    let waiting: Vec<String> = match p.state.get("waiting_on") {
        Some(Detail::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Detail::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut out = format!("Freshness: {} — {status}", p.document);
    if !waiting.is_empty() {
        let _ = write!(out, ", waits for {}", waiting.join(", "));
    }
    out.push('\n');
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
    if let Some(Detail::List(requests)) = p.state.get("active_invalidations")
        && !requests.is_empty()
    {
        out.push_str("Semantic review requests:\n");
        for request in requests {
            let id = match request.get("id") {
                Some(Detail::Number(id)) => id.to_string(),
                _ => "?".to_string(),
            };
            let _ = writeln!(
                out,
                "  [{id}] {}",
                text_field(request, "reason").unwrap_or("")
            );
        }
    }
    if p.changes.is_empty() {
        if status == "never_reviewed" {
            out.push_str("Changes: no earlier review to compare with (no_previous_review)\n");
        } else {
            out.push_str("Changes: none since the last review\n");
        }
    } else {
        out.push_str("Changes since the last review:\n");
        let mut budget = HunkBudget::new(false);
        for change in &p.changes {
            let identity = text_field(change, "identity").unwrap_or("");
            let _ = writeln!(
                out,
                "  {:<8} {identity}",
                text_field(change, "change").unwrap_or("")
            );
            hunk(&mut out, identity, &p.evidence, &mut budget, true);
        }
    }
    let next = if !waiting.is_empty() && status == "current" {
        format!(
            "Next: no review needed now. It waits for {}: review that first, then explain this document again.",
            waiting.join(", ")
        )
    } else if !waiting.is_empty() {
        format!(
            "Next: review {} first; this document waits for it.",
            waiting.join(", ")
        )
    } else if p.render_required {
        // A render can restore bytes that were already reviewed, so the
        // state after it is unknown here: name only the render.
        format!(
            "Next: memoria render {doc}. Then run memoria explain {doc} again, because the render can leave it current.",
            doc = p.document
        )
    } else if status == "current" {
        "Next: no review needed; the document matches its last review.".to_string()
    } else {
        format!("Next: memoria review {}", p.document)
    };
    let _ = writeln!(out, "{next}");
    let _ = writeln!(out, "Exact evidence: memoria explain {} --full", p.document);
    out
}

/// Most section guide paths one human line names.
const MAX_GUIDE_NAMES: usize = 10;

/// " (project guidance and 2 section guides)" when a section guide applies,
/// and nothing otherwise, so a document without guides keeps its 0.8 line.
fn guidance_layers(entries: usize, section_guides: usize) -> String {
    if section_guides == 0 {
        return String::new();
    }
    let guides = format!(
        "{section_guides} section guide{}",
        if section_guides == 1 { "" } else { "s" }
    );
    if entries > section_guides {
        format!(" (project guidance and {guides})")
    } else {
        format!(" ({guides})")
    }
}

/// "a, b, c" with at most `cap` names, then "+N more".
fn capped_list(names: &[&str], cap: usize) -> String {
    let mut text = names
        .iter()
        .take(cap)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > cap {
        text.push_str(&format!(", +{} more", names.len() - cap));
    }
    text
}
