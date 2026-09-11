//! Human-readable rendering of command results.

use std::fmt::Write as _;

use memoria_application::error::Detail;
use memoria_application::packet::{ContentEncoding, FocusedReviewPacket};
use memoria_application::usecases::ack::AckReport;
use memoria_application::usecases::agent::AgentReport;
use memoria_application::usecases::check::CheckReport;
use memoria_application::usecases::graph::GraphReport;
use memoria_application::usecases::init::InitReport;
use memoria_application::usecases::invalidate::InvalidateReport;
use memoria_application::usecases::lint::LintReport;
use memoria_application::usecases::plan::ReviewPlan;
use memoria_application::usecases::render::RenderReport;
use memoria_application::usecases::status::StatusReport;

pub fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.1} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub fn state_diff(report: &memoria_application::usecases::state_diff::StateDiffReport) -> String {
    let mut out = format!(
        "State comparison: {} -> {}\n",
        text_of(&report.before, "path"),
        text_of(&report.after, "path")
    );
    out.push_str(if !report.changes.is_empty() {
        "Logical state changed\n"
    } else if report.byte_equal {
        "Logical state unchanged; encoded bytes identical.\n"
    } else {
        "Logical state unchanged; encoded bytes differ.\n"
    });
    for change in &report.changes {
        let _ = writeln!(out, "\n{}", texts_of(change, "path").join(" -> "));
        for side in ["before", "after"] {
            let _ = writeln!(
                out,
                "  {side} (present: {}):",
                text_of(change, &format!("{side}_present"))
            );
            if let Some(value) = change.get(side) {
                super::human::detail(&mut out, value, 2);
            }
        }
    }
    out.push_str(
        "\nThis compares saved review records. It does not establish current freshness.\n",
    );
    out
}

pub fn explain(report: &memoria_application::usecases::explain::ExplainReport) -> String {
    let mut out = format!(
        "Freshness: {} ({})\n",
        report.document,
        text_of(&report.state, "status")
    );
    super::human::detail(&mut out, &report.state, 0);
    out.push_str("\nChanges:\n");
    super::human::detail(&mut out, &Detail::List(report.changes.clone()), 1);
    out.push_str("\nPolicy (prior rules are not stored):\n");
    super::human::detail(&mut out, &report.policy, 1);
    out.push_str("\nGuidance (advisory context):\n");
    super::human::detail(&mut out, &report.guidance, 1);
    out.push_str("\nGit evidence:\n");
    super::human::detail(&mut out, &Detail::List(report.evidence.clone()), 1);
    let _ = writeln!(out, "\nRead guidance: memoria guidance {}", report.document);
    let _ = writeln!(
        out,
        "Review packet (when ready): memoria review {} --format json",
        report.document
    );
    out
}

fn text_of(detail: &Detail, key: &str) -> String {
    match detail.get(key) {
        Some(Detail::Text(s)) => s.clone(),
        Some(Detail::Number(n)) => n.to_string(),
        Some(Detail::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn texts_of(detail: &Detail, key: &str) -> Vec<String> {
    match detail.get(key) {
        Some(Detail::List(items)) => items
            .iter()
            .filter_map(|i| {
                if let Detail::Text(s) = i {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn causes_of(detail: &Detail) -> String {
    match detail.get("causes") {
        Some(Detail::List(items)) => items
            .iter()
            .map(|cause| {
                let code = text_of(cause, "code");
                if code == "explicit_invalidation" {
                    format!(
                        "{code}#{} ({})",
                        text_of(cause, "id"),
                        text_of(cause, "reason")
                    )
                } else {
                    code
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

pub fn init(report: &InitReport) -> String {
    let mut out = String::new();
    if report.applied {
        let _ = writeln!(out, "memoria init --apply");
    } else {
        let _ = writeln!(out, "memoria init preview. Nothing was written.");
    }
    // The strategy explanation comes before the file operations: the author
    // chooses the documentation model, not the tool.
    let _ = writeln!(out, "\nWhat Memoria does:");
    for line in &report.strategy {
        let _ = writeln!(out, "  - {line}");
    }
    let _ = writeln!(out, "\nDocumentation strategies other projects use:");
    for example in &report.examples {
        let _ = writeln!(out, "  - {}: {}", example.name, example.summary);
    }
    let _ = writeln!(out, "\nCommitted files:");
    for file in &report.files {
        let _ = writeln!(out, "  {:<7} {}", file.action, file.path);
    }
    let _ = writeln!(
        out,
        "\nRoot README.md  {}",
        if report.root_readme_present {
            "present"
        } else {
            "missing; write it before `memoria init --apply`"
        }
    );
    if report.applied {
        for path in &report.created {
            let _ = writeln!(out, "created   {path}");
        }
        for path in &report.existing {
            let _ = writeln!(out, "existing  {path}");
        }
    } else {
        let _ = writeln!(
            out,
            "\nRun `memoria init --apply` to create the missing files."
        );
    }
    let _ = writeln!(out, "Then run `{}`.", report.next_command);
    out
}

pub fn guidance(report: &memoria_application::usecases::guidance::GuidanceReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Guidance for {}", report.document);
    let _ = writeln!(out, "Digest          {}", report.digest);
    match report.changed_since_review {
        None => {
            let _ = writeln!(out, "Reviewed        never");
        }
        Some(changed) => {
            let _ = writeln!(
                out,
                "Reviewed        {} ({})",
                report.reviewed_digest.as_deref().unwrap_or("none"),
                if changed { "changed" } else { "unchanged" }
            );
        }
    }
    if report.entries.is_empty() {
        let _ = writeln!(
            out,
            "\nThis boundary has no guidance. Add entries under [documentation] in memoria.toml."
        );
    } else {
        let _ = writeln!(out, "\nEffective guidance, in applied order:");
        for entry in &report.entries {
            let field = |key: &str| match entry.get(key) {
                Some(memoria_application::error::Detail::Text(text)) => text.clone(),
                _ => String::new(),
            };
            let scope = field("scope");
            let scope = if scope.is_empty() {
                "<root>".to_string()
            } else {
                scope
            };
            let _ = writeln!(
                out,
                "\n  [{} {} scope={scope}]\n{}",
                field("kind"),
                field("source"),
                field("text").trim_end()
            );
        }
    }
    if !report.scopes.is_empty() {
        let _ = writeln!(out, "\nScopes that add guidance:");
        for scope in &report.scopes {
            let label = if scope.scope.is_empty() {
                "<root>"
            } else {
                scope.scope.as_str()
            };
            let _ = writeln!(
                out,
                "  {label} ({} entries, {}): {}",
                scope.entries, scope.source, scope.inspect_command
            );
        }
    }
    let _ = writeln!(
        out,
        "\nGuidance is review context. It never selects files and never decides freshness."
    );
    out
}

pub fn summary(report: &memoria_application::usecases::status::StatusSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "READMEs         {}", report.readmes);
    let _ = writeln!(out, "Selected files  {}", report.selected_files);
    let _ = writeln!(
        out,
        "Input size      {}",
        human_bytes(report.selected_bytes)
    );
    let _ = writeln!(
        out,
        "Reviews         {} current, {} pending, {} never reviewed, {} waiting",
        report.current, report.pending, report.never_reviewed, report.waiting
    );
    let _ = writeln!(
        out,
        "Guidance        {} documents, {} changed since review, {} never reviewed",
        report.guidance.documents_with_guidance,
        report.guidance.changed_documents,
        report.guidance.unreviewed_documents
    );
    let _ = writeln!(
        out,
        "Coverage        {} unowned, {} disconnected",
        report.unowned, report.disconnected
    );
    let _ = writeln!(
        out,
        "Invalidations   {} open, {} missing targets",
        report.open_invalidations, report.missing_invalidation_targets
    );
    let _ = writeln!(
        out,
        "Diagnostics     {} errors, {} warnings",
        report.error_diagnostics, report.warning_diagnostics
    );
    out
}

pub fn state_inspect(
    report: &memoria_application::usecases::state_inspect::InspectReport,
) -> String {
    let i = &report.inspected;
    let mut out = String::new();
    let _ = writeln!(out, "State file      {}", i.path);
    let _ = writeln!(out, "Format          version {}", i.format_version);
    let _ = writeln!(out, "Codec           {}", i.codec);
    let _ = writeln!(out, "File bytes      {}", i.file_bytes);
    let _ = writeln!(out, "Payload bytes   {}", i.payload_bytes);
    let _ = writeln!(out, "Checksum        {}", i.checksum);
    let _ = writeln!(out, "Revision        {}", i.state.revision);
    let _ = writeln!(out, "Next id         {}", i.state.next_invalidation_id);
    let digests: std::collections::BTreeMap<&str, &str> = i
        .guidance
        .iter()
        .map(|(document, digest)| (document.as_str(), digest.as_str()))
        .collect();
    let _ = writeln!(out, "\nReviews ({}):", i.state.reviews.len());
    for (document, record) in &i.state.reviews {
        let _ = writeln!(
            out,
            "  {document}\n    revision {} by {} at {} ({})",
            record.revision,
            record.reviewer.as_str(),
            record.reviewed_at,
            record.result.as_str()
        );
        let _ = writeln!(
            out,
            "    files {}, imports {}, guidance {}",
            record.manifest.files().len(),
            record.manifest.imports().len(),
            digests
                .get(document.as_str())
                .copied()
                .unwrap_or("0000000000000000")
        );
        let _ = writeln!(
            out,
            "    commit {}{}",
            record.git.base_commit.as_deref().unwrap_or("none"),
            if record.git.worktree_dirty {
                " (dirty)"
            } else {
                ""
            }
        );
        let _ = writeln!(out, "    note {}", record.note.as_str());
    }
    let _ = writeln!(
        out,
        "\nActive invalidations ({}):",
        i.state.invalidations.len()
    );
    for invalidation in &i.state.invalidations {
        let _ = writeln!(
            out,
            "  #{} {} at {}: {}",
            invalidation.id,
            invalidation.scope,
            invalidation.created_at,
            invalidation.reason.as_str()
        );
        let _ = writeln!(
            out,
            "    {} target(s), {} still pending",
            invalidation.targets.len(),
            invalidation.pending.len()
        );
    }
    let _ = writeln!(
        out,
        "\nInspection reads stored bytes only. Run `memoria status` for worktree freshness."
    );
    out
}

pub fn status(report: &StatusReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "READMEs         {}", report.readmes);
    let _ = writeln!(out, "Selected files  {}", report.selected_files);
    let _ = writeln!(
        out,
        "Input size      {}",
        human_bytes(report.selected_bytes)
    );
    let _ = writeln!(
        out,
        "Reviews         {} current, {} pending, {} never reviewed, {} waiting",
        report.current, report.pending, report.never_reviewed, report.waiting
    );
    let pending_total: usize = report
        .invalidations
        .iter()
        .map(|i| i.pending_existing.len() + i.pending_missing.len())
        .sum();
    let _ = writeln!(
        out,
        "Invalidations   {} active, {} READMEs pending",
        report.invalidations.len(),
        pending_total
    );
    let _ = writeln!(
        out,
        "Navigation      {} README(s) not reachable from the root",
        report.disconnected.len()
    );
    let _ = writeln!(
        out,
        "Coverage        {} selected file(s) without an owner",
        report.unowned.len()
    );
    if !report.boundaries.is_empty() {
        let _ = writeln!(
            out,
            "Boundaries      {} nested repository path(s) not entered",
            report.boundaries.len()
        );
    }
    if !report.exclusions.is_empty() {
        let parts: Vec<String> = report
            .exclusions
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(out, "Excluded        {}", parts.join(", "));
    }
    for inv in &report.invalidations {
        let _ = writeln!(
            out,
            "\ninvalidation #{} ({}) {:?}",
            inv.id, inv.scope, inv.reason
        );
        for doc in &inv.pending_existing {
            let _ = writeln!(out, "  pending  {doc}");
        }
        for doc in &inv.pending_missing {
            let _ = writeln!(out, "  missing  {doc}");
        }
    }
    if !report.documents.is_empty() {
        let _ = writeln!(out, "\nDocuments:");
        for doc in &report.documents {
            let mut line = format!(
                "  {:<16} {}",
                text_of(doc, "status"),
                text_of(doc, "document")
            );
            let waiting = texts_of(doc, "waiting_on");
            if !waiting.is_empty() {
                let _ = write!(line, "  waiting on {}", waiting.join(", "));
            }
            let causes = causes_of(doc);
            if !causes.is_empty() {
                let _ = write!(line, "  [{causes}]");
            }
            if text_of(doc, "render_required") == "true" {
                line.push_str("  (render required)");
            }
            if text_of(doc, "disconnected") == "true" {
                line.push_str("  (disconnected)");
            }
            let _ = writeln!(out, "{line}");
        }
    }
    for path in &report.unowned {
        let _ = writeln!(out, "unowned  {path}");
    }
    if let Some(explanation) = &report.explanation {
        let _ = writeln!(out, "\nExplain {}", explanation.path);
        let _ = writeln!(out, "  outcome  {}", explanation.outcome);
        let _ = writeln!(out, "  reason   {}", explanation.reason);
        if let Some(owner) = &explanation.owner {
            let _ = writeln!(out, "  owner    {owner}");
        }
        for step in &explanation.steps {
            let _ = writeln!(out, "  rule     {step}");
        }
    }
    out
}

pub fn lint(report: &LintReport) -> String {
    format!(
        "READMEs {}: {} error(s), {} warning(s), {} hint(s)\n",
        report.readmes, report.errors, report.warnings, report.hints
    )
}

pub fn plan(report: &ReviewPlan) -> String {
    let mut out = String::new();
    if report.tasks.is_empty() {
        let _ = writeln!(out, "No README needs review.");
    } else {
        let _ = writeln!(
            out,
            "Review plan ({} task(s), dependency order):",
            report.tasks.len()
        );
        for task in &report.tasks {
            let state = if text_of(task, "ready") == "true" {
                "ready"
            } else {
                "waiting"
            };
            let mut line = format!(
                "{:>3}. {:<8} {}",
                text_of(task, "order"),
                state,
                text_of(task, "document")
            );
            let _ = write!(
                line,
                "  {} in {} record(s)",
                human_bytes(text_of(task, "raw_input_bytes").parse().unwrap_or(0)),
                text_of(task, "record_count")
            );
            let causes = causes_of(task);
            if !causes.is_empty() {
                let _ = write!(line, "  [{causes}]");
            }
            let waiting = texts_of(task, "waiting_on");
            if !waiting.is_empty() {
                let _ = write!(line, "  waiting on {}", waiting.join(", "));
            }
            if text_of(task, "render_required") == "true" {
                line.push_str("  (run `memoria render` first)");
            }
            if text_of(task, "guidance_present") == "true" {
                line.push_str("; guidance present");
            }
            if text_of(task, "guidance_changed") == "true" {
                line.push_str("; guidance changed");
            }
            let _ = writeln!(out, "{line}");
        }
    }
    for waiting in &report.waiting_current {
        let _ = writeln!(
            out,
            "     current  {}  waiting on {}",
            text_of(waiting, "document"),
            texts_of(waiting, "waiting_on").join(", ")
        );
    }
    if !report.tasks.is_empty() {
        let _ = writeln!(out, "{}", report.guidance_first);
        if let Some(next) = &report.next_ready
            && let Some(task) = report
                .tasks
                .iter()
                .find(|task| text_of(task, "document") == *next)
        {
            let _ = writeln!(out, "Guidance: {}", text_of(task, "guidance_command"));
        }
    }
    match &report.next_action {
        Some((kind, next)) => {
            let _ = writeln!(out, "Next: memoria {kind} {next}");
        }
        None if !report.tasks.is_empty() => {
            let _ = writeln!(
                out,
                "Next: review or render the pending dependencies first."
            );
        }
        None => {}
    }
    out
}

pub fn packet(packet: &FocusedReviewPacket) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Focused review: {}", packet.document);
    let _ = writeln!(out, "Token           {}", packet.token);
    let _ = writeln!(out, "Revision        {}", packet.review_revision);
    let _ = writeln!(
        out,
        "Input size      {} in {} record(s)",
        human_bytes(packet.raw_input_bytes),
        packet.record_count
    );
    let _ = writeln!(out, "Owned files     {}", packet.manifest.files().len());
    let _ = writeln!(out, "Imports         {}", packet.manifest.imports().len());
    let _ = writeln!(
        out,
        "Consumers       {}",
        if packet.context.consumers.is_empty() {
            "none".to_string()
        } else {
            packet.context.consumers.join(", ")
        }
    );
    let _ = writeln!(
        out,
        "Git             base {} ({})",
        packet.context.git.base_commit.as_deref().unwrap_or("none"),
        if packet.context.git.worktree_dirty {
            "dirty worktree"
        } else {
            "clean worktree"
        }
    );
    let _ = writeln!(
        out,
        "\nThis human view is for reading. Use `--format json` output with `memoria ack --packet`."
    );
    if !packet.covered_invalidations.is_empty() {
        let _ = writeln!(out, "\nSemantic review reasons:");
        for (id, reason) in &packet.covered_invalidations {
            let _ = writeln!(out, "  #{id}: {reason}");
        }
    }
    // Effective guidance comes before the owned evidence: read the project's
    // documentation goals before you judge this boundary.
    let guidance = &packet.context.guidance;
    let _ = writeln!(
        out,
        "\nProject documentation guidance (digest {}):",
        guidance.digest
    );
    if guidance.entries.is_empty() {
        let _ = writeln!(out, "  none declared for this boundary");
    } else {
        for entry in &guidance.entries {
            let scope = if entry.scope.is_root() {
                "<root>".to_string()
            } else {
                entry.scope.as_str().to_string()
            };
            let _ = writeln!(
                out,
                "  [{} {} scope={scope}]\n{}",
                entry.kind,
                entry.source,
                entry.text.trim_end()
            );
        }
    }
    match &packet.context.previous_review {
        Some(record) => {
            let _ = writeln!(
                out,
                "\nPrevious review: revision {} by {} at {} ({}): {}",
                record.revision,
                record.reviewer.as_str(),
                record.reviewed_at,
                record.result.as_str(),
                record.note.as_str()
            );
        }
        None => {
            let _ = writeln!(out, "\nPrevious review: none");
        }
    }
    if !packet.context.changes.is_empty() {
        let _ = writeln!(out, "\nChanged inputs:");
        for change in &packet.context.changes {
            let _ = writeln!(
                out,
                "  {} {} {}",
                change.change, change.kind, change.identity
            );
        }
    }
    for diff in &packet.context.diffs {
        let _ = writeln!(
            out,
            "\nDiff {} [{}]{}",
            diff.identity,
            diff.status,
            diff.reason
                .as_ref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        );
        if let Some(text) = &diff.text {
            out.push_str(text);
        }
    }
    let _ = writeln!(
        out,
        "\n==== README {} ({} bytes) ====",
        packet.content.readme.path, packet.content.readme.bytes
    );
    out.push_str(&String::from_utf8_lossy(&packet.content.readme.body));
    for file in &packet.content.files {
        let _ = writeln!(
            out,
            "\n==== FILE {} ({} bytes, {}) ====",
            file.path,
            file.bytes,
            file.encoding.as_str()
        );
        match file.encoding {
            ContentEncoding::Utf8 => out.push_str(&String::from_utf8_lossy(&file.body)),
            ContentEncoding::Base64 => {
                let _ = writeln!(out, "(binary content omitted from the human view)");
            }
        }
    }
    for import in &packet.content.imports {
        let _ = writeln!(
            out,
            "\n==== IMPORT {}#{} ({} bytes) ====",
            import.document, import.export_id, import.bytes
        );
        out.push_str(&String::from_utf8_lossy(&import.body));
    }
    out
}

pub fn render(report: &RenderReport) -> String {
    let mut out = String::new();
    let changed: Vec<_> = report.changed().collect();
    if changed.is_empty() {
        let _ = writeln!(
            out,
            "All import blocks are current ({} README(s) checked).",
            report.documents.len()
        );
        return out;
    }
    for document in changed {
        let verb = if report.dry_run {
            "would update"
        } else if document.applied {
            "updated"
        } else {
            "not applied"
        };
        let _ = writeln!(out, "{verb}  {}", document.document);
        for patch in &document.patches {
            let _ = writeln!(
                out,
                "    line {}: {}#{} ({} -> {} bytes)",
                patch.line, patch.provider, patch.export_id, patch.before_bytes, patch.after_bytes
            );
        }
    }
    out
}

pub fn ack(report: &AckReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Recorded {} revision {} ({}) by {}",
        report.document, report.revision, report.result, report.reviewer
    );
    if !report.cleared.is_empty() {
        let ids: Vec<String> = report.cleared.iter().map(|id| format!("#{id}")).collect();
        let _ = writeln!(out, "Cleared invalidations: {}", ids.join(", "));
    }
    for (id, reason) in &report.still_pending {
        let _ = writeln!(out, "Still pending: #{id} {reason:?}");
    }
    out
}

pub fn invalidate(report: &InvalidateReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Invalidation #{} ({}) recorded: {:?}",
        report.id, report.scope, report.reason
    );
    for target in &report.targets {
        let _ = writeln!(out, "  pending  {target}");
    }
    out
}

pub fn check(report: &CheckReport) -> String {
    let mut out = String::new();
    let ok = report.pending.is_empty()
        && report.outdated_imports.is_empty()
        && report.structural_errors == 0;
    if ok {
        let _ = writeln!(
            out,
            "OK: {} README(s) current, imports rendered, no coverage or structure errors.",
            report.readmes
        );
        return out;
    }
    let _ = writeln!(
        out,
        "FAILED: {} pending review(s), {} outdated import(s), {} structural error(s)",
        report.pending.len(),
        report.outdated_imports.len(),
        report.structural_errors
    );
    for pending in &report.pending {
        let _ = writeln!(
            out,
            "  pending  {}  [{}]",
            text_of(pending, "document"),
            causes_of(pending)
        );
    }
    for outdated in &report.outdated_imports {
        let _ = writeln!(out, "  outdated {outdated}");
    }
    out
}

pub fn graph(report: &GraphReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Nodes:");
    for node in &report.nodes {
        let mut line = format!(
            "  {:<16} {} ({} file(s))",
            text_of(node, "status"),
            text_of(node, "document"),
            text_of(node, "owned_files")
        );
        if text_of(node, "waiting") == "true" {
            let _ = write!(
                line,
                "  waiting on {}",
                texts_of(node, "waiting_on").join(", ")
            );
        }
        if text_of(node, "disconnected") == "true" {
            line.push_str("  (disconnected)");
        }
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out, "Edges:");
    for edge in &report.edges {
        let kind = text_of(edge, "kind");
        let extra = if kind == "import" {
            format!("#{}", text_of(edge, "export_id"))
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "  {:<7} {} -> {}{extra}",
            kind,
            text_of(edge, "from"),
            text_of(edge, "to")
        );
    }
    out
}

pub fn agent(report: &AgentReport) -> String {
    let mut out = String::new();
    let plan = &report.plan;
    let verb = match (report.dry_run, plan.no_change, report.applied) {
        (true, _, _) => "dry run",
        (_, true, _) => "no change",
        (_, _, true) => "applied",
        _ => "planned",
    };
    let _ = writeln!(
        out,
        "{verb}: {} target={} destination={}",
        plan_operation(plan),
        plan.target.as_str(),
        plan.destination
    );
    let _ = writeln!(out, "  scope      {}", plan.scope.as_str());
    let _ = writeln!(out, "  state      {}", plan.state);
    let _ = writeln!(
        out,
        "  version    installed {} / embedded {}",
        plan.package_version.as_deref().unwrap_or("none"),
        plan.embedded_version
    );
    let _ = writeln!(out, "  existing   {}", plan.existing);
    for path in &plan.modified_paths {
        let _ = writeln!(out, "  modified   {path}");
    }
    for path in &plan.unknown_paths {
        let _ = writeln!(out, "  unknown    {path}");
    }
    for artifact in &plan.retained_artifacts {
        let _ = writeln!(
            out,
            "  retained   {} ({}, removable_by_uninstall={})",
            artifact.path, artifact.reason, artifact.removable_by_uninstall
        );
    }
    for package in &plan.overlapping {
        let _ = writeln!(
            out,
            "  overlap    {} {} ({}): {}",
            package.scope, package.destination, package.state, package.note
        );
    }
    if plan.recovery_needed {
        let _ = writeln!(
            out,
            "  recovery   an interrupted transaction will be recovered first"
        );
    }
    for path in &plan.writes {
        let _ = writeln!(out, "  write      {path}");
    }
    for path in &plan.replaced {
        let _ = writeln!(out, "  replace    {path}");
    }
    for path in &plan.removals {
        let _ = writeln!(out, "  remove     {path}");
    }
    if let Some(backup) = &plan.backup {
        let _ = writeln!(out, "  backup     {backup}");
    }
    out
}

fn plan_operation(plan: &memoria_application::ports::SkillPlan) -> &'static str {
    plan.operation.as_str()
}

pub fn agent_hook(report: &memoria_application::usecases::agent_hooks::HookReport) -> String {
    let plan = &report.plan;
    let mut out = String::new();
    let verb = match (report.dry_run, plan.no_change, report.applied) {
        (true, _, _) => "dry run",
        (_, true, _) => "no change",
        (_, _, true) => "applied",
        _ => "planned",
    };
    let _ = writeln!(
        out,
        "{verb}: Stop hook target={} state={}",
        plan.target.as_str(),
        plan.state
    );
    let _ = writeln!(out, "  configuration {}", plan.configuration);
    let _ = writeln!(out, "  record        {}", plan.record);
    let _ = writeln!(out, "  activation    {}", plan.activation);
    if plan.recovery_needed {
        let _ = writeln!(
            out,
            "  recovery      an interrupted hook transaction record remains"
        );
    }
    for path in &plan.writes {
        let _ = writeln!(out, "  write         {path}");
    }
    for path in &plan.removals {
        let _ = writeln!(out, "  remove        {path}");
    }
    let _ = writeln!(out, "\nCommand:\n  {}", plan.command);
    let _ = writeln!(
        out,
        "\nThese are local installation changes. The absolute executable path does not belong in a portable shared hook configuration.\nThe client still reviews and activates the hook: open its /hooks interface."
    );
    out
}

/// The managed consumer GitHub workflow plan or result.
pub fn github_workflow(
    report: &memoria_application::usecases::github_workflow::WorkflowReport,
) -> String {
    let mut out = String::new();
    let plan = &report.plan;
    let verb = match (report.applied, report.dry_run, plan.no_change) {
        (true, _, _) => "applied",
        (_, _, true) => "no change",
        (_, true, _) => "dry run",
        _ => "preview",
    };
    let _ = writeln!(
        out,
        "{verb}: integrations github {} {}",
        report.operation.as_str(),
        plan.path
    );
    let _ = writeln!(out, "  state      {}", plan.state);
    let _ = writeln!(out, "  record     {}", plan.record_path);
    let _ = writeln!(
        out,
        "  memoria    installed {} / desired {}",
        plan.installed_version.as_deref().unwrap_or("none"),
        plan.desired_version
    );
    let _ = writeln!(
        out,
        "  action ref installed {} / desired {}",
        plan.installed_action_ref.as_deref().unwrap_or("none"),
        plan.desired_action_ref
    );
    let _ = writeln!(
        out,
        "  runner     installed {} / desired {}",
        plan.installed_runner.as_deref().unwrap_or("none"),
        plan.desired_runner
    );
    for path in &plan.writes {
        let _ = writeln!(out, "  write      {path}");
    }
    for path in &plan.removals {
        let _ = writeln!(out, "  remove     {path}");
    }
    for artifact in &plan.retained_artifacts {
        let _ = writeln!(
            out,
            "  retained   {} ({}, removable_by_uninstall={})",
            artifact.path, artifact.reason, artifact.removable_by_uninstall
        );
    }
    for name in &plan.siblings {
        let _ = writeln!(out, "  sibling    .github/workflows/{name}");
    }
    for note in &plan.notes {
        let _ = writeln!(out, "  note       {note}");
    }
    if plan.recovery_needed {
        let _ = writeln!(
            out,
            "  recovery   an interrupted transaction must be resolved before a change"
        );
    }
    if let Some(rendered) = &plan.rendered
        && !report.applied
    {
        let _ = writeln!(out, "\nproposed {}:\n", plan.path);
        for line in rendered.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }
    if let Some(command) = &report.apply_command {
        let _ = writeln!(out, "\nRun `{command}` to perform this change.");
    }
    out
}
