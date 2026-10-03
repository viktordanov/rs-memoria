//! `memoria review` without a document: the ordered review plan.

use crate::error::{AppError, Detail, DetailMap, Outcome};
use crate::ports::Services;
use crate::snapshot::{self, Snapshot};

use super::document_status_detail;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPlan {
    pub tasks: Vec<Detail>,
    pub waiting_current: Vec<Detail>,
    /// Read the effective guidance of a document before you review it.
    pub guidance_first: String,
    pub next_ready: Option<String>,
    /// The next executable step: `("render", document)` when the ready
    /// document still has outdated imports, otherwise `("review", document)`.
    pub next_action: Option<(String, String)>,
    /// Reviewed documents whose effective guidance changed since their
    /// review. A separate assessment item, never a pending review.
    pub guidance_changed_documents: u64,
}

/// The command that assesses a guidance change.
pub const GUIDANCE_ASSESSMENT_COMMAND: &str = "memoria guidance --changed";

impl ReviewPlan {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", "review_plan")
            .with("tasks", Detail::List(self.tasks.clone()))
            .with(
                "waiting_current",
                Detail::List(self.waiting_current.clone()),
            )
            .text("guidance_first", self.guidance_first.clone())
            .with("next_ready", Detail::option_text(self.next_ready.clone()))
            .with(
                "next_action",
                match &self.next_action {
                    None => Detail::Null,
                    Some((kind, document)) => DetailMap::default()
                        .text("kind", kind.clone())
                        .text("document", document.clone())
                        .build(),
                },
            )
            .with(
                "guidance_assessment",
                if self.guidance_changed_documents == 0 {
                    Detail::Null
                } else {
                    DetailMap::default()
                        .number("changed_documents", self.guidance_changed_documents)
                        .text("command", GUIDANCE_ASSESSMENT_COMMAND)
                        .build()
                },
            )
            .build()
    }
}

pub fn plan_from(snapshot: &Snapshot) -> ReviewPlan {
    let mut tasks = Vec::new();
    let mut waiting_current = Vec::new();
    let mut next_ready = None;
    let mut next_action = None;
    for (index, status) in snapshot.statuses.iter().enumerate() {
        let manifest = snapshot.manifests.get(&status.document);
        let outdated = snapshot.outdated_imports_of(&status.document);
        if status.pending() {
            let mut detail = document_status_detail(status);
            if let Detail::Map(map) = &mut detail {
                map.insert("order".into(), Detail::Number(index as u64 + 1));
                map.insert(
                    "raw_input_bytes".into(),
                    Detail::Number(manifest.map(|m| m.raw_input_bytes()).unwrap_or(0)),
                );
                map.insert(
                    "record_count".into(),
                    Detail::Number(manifest.map(|m| m.record_count()).unwrap_or(0)),
                );
                map.insert(
                    "document_kind".into(),
                    Detail::Text(status.document.kind().as_str().to_string()),
                );
                map.insert(
                    "scope_files".into(),
                    Detail::Number(snapshot.scopes.scope_of(&status.document).len() as u64),
                );
                map.insert(
                    "co_covering".into(),
                    Detail::texts(
                        snapshot
                            .co_covering(&status.document)
                            .iter()
                            .map(|d| d.as_str().to_string()),
                    ),
                );
                map.insert("render_required".into(), Detail::Bool(!outdated.is_empty()));
                let effective = snapshot.guidance_of(&status.document);
                map.insert(
                    "guidance_present".into(),
                    Detail::Bool(!effective.is_empty()),
                );
                map.insert(
                    "guidance_changed".into(),
                    snapshot
                        .guidance_changed(&status.document)
                        .map(Detail::Bool)
                        .unwrap_or(Detail::Null),
                );
                map.insert(
                    "guidance_command".into(),
                    Detail::Text(format!("memoria guidance {}", status.document)),
                );
                map.insert(
                    "outdated_imports".into(),
                    Detail::texts(
                        outdated
                            .iter()
                            .map(|o| format!("{}#{}", o.provider, o.export_id)),
                    ),
                );
            }
            if next_ready.is_none() && status.ready() {
                next_ready = Some(status.document.as_str().to_string());
                next_action = Some((
                    if outdated.is_empty() {
                        "review"
                    } else {
                        "render"
                    }
                    .to_string(),
                    status.document.as_str().to_string(),
                ));
            }
            tasks.push(detail);
        } else if status.waiting() {
            waiting_current.push(document_status_detail(status));
        }
    }
    ReviewPlan {
        tasks,
        waiting_current,
        guidance_first:
            "Read the effective guidance of a document before you review it: `memoria guidance <DOCUMENT>`."
                .to_string(),
        next_ready,
        next_action,
        guidance_changed_documents: super::status::guidance_counts(snapshot).changed_documents,
    }
}

pub fn run(services: &Services<'_>) -> Result<Outcome<ReviewPlan>, AppError> {
    let snapshot = snapshot::build(services)?;
    snapshot.require_valid()?;
    Ok(Outcome::new(
        plan_from(&snapshot),
        snapshot.non_error_diagnostics(),
    ))
}
