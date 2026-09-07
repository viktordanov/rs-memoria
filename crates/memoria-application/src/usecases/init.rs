//! `memoria init`: create missing configuration, root README, and state.

use memoria_domain::ReviewState;

use crate::config::{ROOT_CONFIG_TEMPLATE, ROOT_README_TEMPLATE};
use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::{FileKind, Services, StateFailure};
use crate::snapshot::{ROOT_CONFIG_PATH, STATE_PATH};

use super::acquire_lock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub created: Vec<String>,
    pub existing: Vec<String>,
    pub checklist: Vec<String>,
}

impl InitReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .with("created", Detail::texts(self.created.clone()))
            .with("existing", Detail::texts(self.existing.clone()))
            .with("checklist", Detail::texts(self.checklist.clone()))
            .build()
    }
}

pub const CHECKLIST: &[&str] = &[
    "Review committed generated output and snapshots; exclude them in memoria.toml `ignore` when they should not drive reviews.",
    "Decide whether tests and fixtures are documentation inputs; they stay selected until a project rule excludes them.",
    "Add a README.md to every directory that deserves its own explanation; the nearest README owns each file.",
    "Declare imports with <!-- memoria:import src=\"child/README.md#summary\" --> and run `memoria render`.",
    "Run `memoria review`, acknowledge each packet, then finish with `memoria check`.",
];

fn io(err: crate::ports::AdapterError) -> AppError {
    AppError::io("io_error", err.to_string())
}

pub fn run(services: &Services<'_>) -> Result<Outcome<InitReport>, AppError> {
    let mut report = InitReport {
        created: vec![],
        existing: vec![],
        checklist: CHECKLIST.iter().map(|s| s.to_string()).collect(),
    };

    // Validate existing files before any write.
    let config_kind = services.files.kind(ROOT_CONFIG_PATH).map_err(io)?;
    match config_kind {
        FileKind::Missing => {}
        FileKind::Regular => {
            let bytes = services.files.read(ROOT_CONFIG_PATH).map_err(io)?;
            let config = services.config.parse_root(&bytes).map_err(|message| {
                AppError::new(
                    ExitClass::Validation,
                    Diagnostic::error("configuration_invalid", message).at_path(ROOT_CONFIG_PATH),
                )
            })?;
            // The configuration must also mean something valid: patterns
            // compile and instruction references resolve, exactly as
            // inspection requires, before any directory, lock, or file exists.
            let problems = crate::snapshot::validate_root_config(services, &config)?;
            if !problems.is_empty() {
                return Err(AppError::many(ExitClass::Validation, problems));
            }
            report.existing.push(ROOT_CONFIG_PATH.to_string());
        }
        other => {
            return Err(AppError::validation(
                "configuration_invalid",
                format!("{ROOT_CONFIG_PATH} is an unsupported {other:?}"),
            ));
        }
    }
    let readme_kind = services.files.kind("README.md").map_err(io)?;
    match readme_kind {
        FileKind::Missing => {}
        FileKind::Regular => {
            // An existing root README must be valid before anything is written.
            let bytes = services.files.read("README.md").map_err(io)?;
            let root = memoria_domain::DirPath::root().readme();
            let parsed = services.markdown.parse(&root, &bytes);
            let mut problems: Vec<Diagnostic> = parsed
                .issues
                .iter()
                .map(|issue| {
                    let mut diagnostic =
                        Diagnostic::error(issue.code, issue.message.clone()).at_path("README.md");
                    if let Some(location) = issue.location {
                        diagnostic = diagnostic.at(location.line, location.column);
                    }
                    diagnostic
                })
                .collect();
            for import in &parsed.imports {
                if let Err(message) = crate::snapshot::resolve_import(&root, &import.source_text) {
                    problems.push(
                        Diagnostic::error("import_invalid", message)
                            .at_path("README.md")
                            .at(import.location.line, import.location.column),
                    );
                }
            }
            if !problems.is_empty() {
                return Err(AppError::many(ExitClass::Validation, problems));
            }
            report.existing.push("README.md".to_string());
        }
        other => {
            return Err(AppError::validation(
                "path_unsupported",
                format!("README.md is an unsupported {other:?}"),
            ));
        }
    }
    let state_exists = match services.state.load() {
        Ok(Some(loaded)) => {
            crate::snapshot::validate_state(services, &loaded.state)?;
            report.existing.push(STATE_PATH.to_string());
            true
        }
        Ok(None) => false,
        Err(StateFailure::Corrupt(message)) => {
            return Err(AppError::new(
                ExitClass::Io,
                Diagnostic::error("state_corrupt", message).at_path(STATE_PATH),
            ));
        }
        Err(StateFailure::Io(err)) => return Err(io(err)),
        Err(StateFailure::Conflict) => {
            return Err(AppError::conflict(
                "state_conflict",
                "state changed while loading",
            ));
        }
    };

    let _guard = acquire_lock(services)?;
    services.progress.note(&format!(
        "init: creating missing files under {}",
        services.files.root_display()
    ));
    let mut failures = Vec::new();
    if config_kind == FileKind::Missing {
        match services
            .writer
            .create_new(ROOT_CONFIG_PATH, ROOT_CONFIG_TEMPLATE.as_bytes())
        {
            Ok(()) => report.created.push(ROOT_CONFIG_PATH.to_string()),
            Err(err) => failures
                .push(Diagnostic::error("io_error", err.to_string()).at_path(ROOT_CONFIG_PATH)),
        }
    }
    if readme_kind == FileKind::Missing {
        match services
            .writer
            .create_new("README.md", ROOT_README_TEMPLATE.as_bytes())
        {
            Ok(()) => report.created.push("README.md".to_string()),
            Err(err) => {
                failures.push(Diagnostic::error("io_error", err.to_string()).at_path("README.md"))
            }
        }
    }
    if !state_exists {
        match services.state.save(&ReviewState::empty(), None) {
            Ok(_) => report.created.push(STATE_PATH.to_string()),
            Err(StateFailure::Conflict) => failures.push(
                Diagnostic::error("state_conflict", "state file appeared during init")
                    .at_path(STATE_PATH),
            ),
            Err(StateFailure::Io(err)) => {
                failures.push(Diagnostic::error("io_error", err.to_string()).at_path(STATE_PATH))
            }
            Err(StateFailure::Corrupt(message)) => {
                failures.push(Diagnostic::error("state_corrupt", message).at_path(STATE_PATH))
            }
        }
    }
    if !failures.is_empty() {
        return Err(AppError::many(ExitClass::Io, failures).with_data(report.to_detail()));
    }
    Ok(Outcome::new(report, vec![]))
}
