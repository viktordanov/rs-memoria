//! `memoria invalidate`: request semantic review with a reason.

use memoria_domain::{DirPath, DocumentId, InvalidationScope, Reason};

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::{Services, StateFailure};
use crate::snapshot::{self, STATE_PATH};

use super::acquire_lock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidateReport {
    pub id: u64,
    pub scope: String,
    pub reason: String,
    pub targets: Vec<String>,
}

impl InvalidateReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .number("id", self.id)
            .text("scope", self.scope.clone())
            .text("reason", self.reason.clone())
            .with("targets", Detail::texts(self.targets.clone()))
            .build()
    }
}

pub fn parse_scope(raw: &str) -> Result<InvalidationScope, AppError> {
    if raw == "all" {
        return Ok(InvalidationScope::All);
    }
    if let Some(path) = raw.strip_prefix("doc:") {
        return DocumentId::parse(path)
            .map(InvalidationScope::Document)
            .map_err(|err| AppError::usage("scope_invalid", err.to_string()));
    }
    if let Some(dir) = raw.strip_prefix("subtree:") {
        return DirPath::parse(dir)
            .map(InvalidationScope::Subtree)
            .map_err(|err| AppError::usage("scope_invalid", err.to_string()));
    }
    Err(AppError::usage(
        "scope_invalid",
        format!("scope {raw:?} must be `all`, `doc:<README.md>`, or `subtree:<directory>`"),
    ))
}

pub fn run(
    services: &Services<'_>,
    raw_scope: &str,
    raw_reason: &str,
) -> Result<Outcome<InvalidateReport>, AppError> {
    let scope = parse_scope(raw_scope)?;
    let reason = Reason::parse(raw_reason)
        .map_err(|err| AppError::usage("reason_invalid", err.to_string()))?;
    let snapshot = snapshot::build(services)?;
    let targets: Vec<DocumentId> = snapshot
        .collected
        .documents
        .iter()
        .filter(|document| scope.covers(document))
        .cloned()
        .collect();
    if targets.is_empty() {
        return Err(AppError::usage(
            "scope_empty",
            format!("scope {raw_scope} matches no discovered README"),
        ));
    }
    let _guard = acquire_lock(services)?;
    let loaded = services.state.load().map_err(state_error)?;
    let (mut state, expected) = match loaded {
        Some(loaded) => (loaded.state, Some(loaded.bytes)),
        None => (memoria_domain::ReviewState::empty(), None),
    };
    state.validate().map_err(|err| {
        AppError::new(
            ExitClass::Io,
            Diagnostic::error("state_corrupt", err.to_string()).at_path(STATE_PATH),
        )
    })?;
    services.progress.note(&format!(
        "invalidate: {} README(s) with reason {:?}",
        targets.len(),
        reason.as_str()
    ));
    let id = state
        .add_invalidation(
            scope.clone(),
            reason.clone(),
            targets.clone(),
            services.clock.now(),
        )
        .map_err(|err| AppError::io("state_corrupt", err.to_string()))?;
    services
        .state
        .save(&state, expected.as_deref())
        .map_err(state_error)?;
    Ok(Outcome::new(
        InvalidateReport {
            id,
            scope: scope.to_string(),
            reason: reason.as_str().to_string(),
            targets: targets.iter().map(|d| d.as_str().to_string()).collect(),
        },
        snapshot.non_error_diagnostics(),
    ))
}

pub(crate) fn state_error(failure: StateFailure) -> AppError {
    match failure {
        StateFailure::Io(err) => AppError::io("io_error", err.to_string()),
        StateFailure::Corrupt(message) => AppError::new(
            ExitClass::Io,
            Diagnostic::error("state_corrupt", message).at_path(STATE_PATH),
        ),
        StateFailure::Conflict => AppError::conflict(
            "state_conflict",
            "the state file changed under the write lock; retry",
        ),
    }
}
