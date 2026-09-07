//! Human-readable rendering of command results.

use std::fmt::Write as _;

use memoria_application::error::{Detail, Diagnostic};
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

pub fn diagnostic_line(diagnostic: &Diagnostic) -> String {
    let mut out = String::new();
    let _ = write!(out, "{}: {}", diagnostic.severity.as_str(), diagnostic.code);
    if let Some(path) = &diagnostic.path {
        let _ = write!(out, " {path}");
        if let Some(line) = diagnostic.line {
            let _ = write!(out, ":{line}");
            if let Some(column) = diagnostic.column {
                let _ = write!(out, ":{column}");
            }
        }
    }
    let _ = write!(out, ": {}", diagnostic.message);
    render_details(&mut out, &diagnostic.details, 1);
    out
}

/// Render structured details under a diagnostic so human output carries the
/// same facts as JSON: cycle edges with marker lines, snapshot differences
/// with lengths and hashes, and textual diffs.
fn render_details(out: &mut String, details: &Detail, depth: usize) {
    let indent = "  ".repeat(depth);
    match details {
        Detail::Map(map) if !map.is_empty() => {
            for (key, value) in map {
                match value {
                    Detail::List(items) if !items.is_empty() => {
                        let _ = write!(out, "\n{indent}{key}:");
                        for item in items {
                            render_list_item(out, item, depth + 1);
                        }
                    }
                    Detail::List(_) => {}
                    Detail::Map(_) => {
                        let _ = write!(out, "\n{indent}{key}:");
                        render_details(out, value, depth + 1);
                    }
                    scalar => {
                        let _ = write!(out, "\n{indent}{key}: ");
                        render_scalar(out, scalar, depth + 1);
                    }
                }
            }
        }
        _ => {}
    }
}

fn render_list_item(out: &mut String, item: &Detail, depth: usize) {
    let indent = "  ".repeat(depth);
    match item {
        Detail::Map(map) => {
            let mut fields = Vec::new();
            let mut blocks = Vec::new();
            for (key, value) in map {
                match value {
                    Detail::Text(text) if text.contains('\n') => blocks.push((key, text)),
                    Detail::List(items) if !items.is_empty() => {
                        let rendered: Vec<String> = items
                            .iter()
                            .map(|i| match i {
                                Detail::Text(t) => t.clone(),
                                Detail::Number(n) => n.to_string(),
                                other => format!("{other:?}"),
                            })
                            .collect();
                        fields.push(format!("{key}=[{}]", rendered.join(", ")));
                    }
                    Detail::List(_) => {}
                    Detail::Null => {}
                    Detail::Text(text) => fields.push(format!("{key}={text}")),
                    Detail::Number(n) => fields.push(format!("{key}={n}")),
                    Detail::Bool(b) => fields.push(format!("{key}={b}")),
                    Detail::Map(_) => fields.push(format!("{key}={{...}}")),
                }
            }
            let _ = write!(out, "\n{indent}- {}", fields.join(" "));
            for (key, text) in blocks {
                let _ = write!(out, "\n{indent}  {key}:");
                for line in text.lines() {
                    let _ = write!(out, "\n{indent}    {line}");
                }
            }
        }
        scalar => {
            let _ = write!(out, "\n{indent}- ");
            render_scalar(out, scalar, depth + 1);
        }
    }
}

fn render_scalar(out: &mut String, value: &Detail, depth: usize) {
    match value {
        Detail::Text(text) if text.contains('\n') => {
            let indent = "  ".repeat(depth);
            for line in text.lines() {
                let _ = write!(out, "\n{indent}{line}");
            }
        }
        Detail::Text(text) => out.push_str(text),
        Detail::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Detail::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        Detail::Null => out.push_str("null"),
        other => {
            let _ = write!(out, "{other:?}");
        }
    }
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
    for path in &report.created {
        let _ = writeln!(out, "created   {path}");
    }
    for path in &report.existing {
        let _ = writeln!(out, "existing  {path}");
    }
    let _ = writeln!(out, "\nNext steps:");
    for (index, item) in report.checklist.iter().enumerate() {
        let _ = writeln!(out, "{}. {item}", index + 1);
    }
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
    if !packet.context.instructions.is_empty() {
        let _ = writeln!(out, "\nWriting instructions:");
        for instruction in &packet.context.instructions {
            let _ = writeln!(
                out,
                "  [{} {}]\n{}",
                instruction.kind,
                instruction.source,
                instruction.text.trim_end()
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
    let _ = writeln!(out, "  existing   {}", plan.existing);
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
    match plan.operation {
        memoria_application::ports::SkillOperation::Install => "install",
        memoria_application::ports::SkillOperation::Uninstall => "uninstall",
    }
}
