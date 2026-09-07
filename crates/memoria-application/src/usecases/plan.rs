//! `memoria review` without a document: the ordered review plan.

use crate::error::{AppError, Detail, DetailMap, Outcome};
use crate::ports::Services;
use crate::snapshot::{self, Snapshot};

use super::document_status_detail;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPlan {
    pub tasks: Vec<Detail>,
    pub waiting_current: Vec<Detail>,
    pub next_ready: Option<String>,
    /// The next executable step: `("render", document)` when the ready
    /// document still has outdated imports, otherwise `("review", document)`.
    pub next_action: Option<(String, String)>,
}

impl ReviewPlan {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", "review_plan")
            .with("tasks", Detail::List(self.tasks.clone()))
            .with(
                "waiting_current",
                Detail::List(self.waiting_current.clone()),
            )
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
                    "owned_files".into(),
                    Detail::Number(snapshot.ownership.owned_by(&status.document).len() as u64),
                );
                map.insert("render_required".into(), Detail::Bool(!outdated.is_empty()));
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
        next_ready,
        next_action,
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
