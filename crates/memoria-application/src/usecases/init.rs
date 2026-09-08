//! `memoria init`: preview the setup, and `--apply` to create the two
//! committed files.
//!
//! The preview is read-only. It explains that the author chooses the
//! documentation strategy before Memoria writes anything. Apply is the
//! explicit acknowledgement of the setup action. It is not a statement that
//! Memoria understands the documentation strategy.

use memoria_domain::ReviewState;

use crate::config::ROOT_CONFIG_TEMPLATE;
use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::{FileKind, Services};
use crate::snapshot::{ROOT_CONFIG_PATH, STATE_PATH, state_failure};

use super::acquire_lock;

/// One proposed or completed file operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAction {
    pub path: String,
    /// `create` or `keep`.
    pub action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    /// Whether this run wrote files.
    pub applied: bool,
    pub root_readme_present: bool,
    /// The two committed files, with the operation each one needs.
    pub files: Vec<FileAction>,
    pub created: Vec<String>,
    pub existing: Vec<String>,
    pub strategy: Vec<String>,
    pub examples: Vec<StrategyExample>,
    pub next_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyExample {
    pub name: &'static str,
    pub summary: &'static str,
}

impl InitReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .bool("applied", self.applied)
            .bool("root_readme_present", self.root_readme_present)
            .with(
                "files",
                Detail::list(self.files.iter().map(|f| {
                    DetailMap::default()
                        .text("path", f.path.clone())
                        .text("action", f.action)
                        .build()
                })),
            )
            .with("created", Detail::texts(self.created.clone()))
            .with("existing", Detail::texts(self.existing.clone()))
            .with("strategy", Detail::texts(self.strategy.clone()))
            .with(
                "examples",
                Detail::list(self.examples.iter().map(|e| {
                    DetailMap::default()
                        .text("name", e.name)
                        .text("summary", e.summary)
                        .build()
                })),
            )
            .text("next_command", self.next_command.clone())
            .build()
    }
}

/// What Memoria does and does not decide. Shown by both the preview and the
/// apply response.
pub const STRATEGY: &[&str] = &[
    "Memoria tracks documentation freshness. You choose what your README hierarchy represents.",
    "The nearest README owns each selected file. Another README starts a new boundary.",
    "Memoria records the inputs a reviewer examined and shows which documents need another review.",
    "Memoria does not write documentation and does not decide whether its explanation is correct.",
    "Project documentation guidance lives in memoria.toml under [documentation]. It is advisory review context, never a selection rule.",
];

/// Three equally valid documentation strategies. `init` never chooses one.
pub const EXAMPLES: &[StrategyExample] = &[
    StrategyExample {
        name: "architecture modules",
        summary: "One README for each crate, package, or layer boundary.",
    },
    StrategyExample {
        name: "business concepts",
        summary: "One README for each domain concept, with its rules and its code.",
    },
    StrategyExample {
        name: "operational workflows",
        summary: "One README for each workflow, from its entry point to its outputs.",
    },
];

fn io(err: crate::ports::AdapterError) -> AppError {
    AppError::io("io_error", err.to_string())
}

pub fn run(services: &Services<'_>, apply: bool) -> Result<Outcome<InitReport>, AppError> {
    let mut report = InitReport {
        applied: apply,
        root_readme_present: false,
        files: vec![],
        created: vec![],
        existing: vec![],
        strategy: STRATEGY.iter().map(|s| s.to_string()).collect(),
        examples: EXAMPLES.to_vec(),
        next_command: "memoria review".to_string(),
    };

    // Inspect the root README first. The preview reports an absent README
    // without failing; apply refuses before any write.
    let readme_kind = services.files.kind("README.md").map_err(io)?;
    match readme_kind {
        FileKind::Missing => {}
        FileKind::Regular => {
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
            report.root_readme_present = true;
        }
        other => {
            return Err(AppError::validation(
                "path_unsupported",
                format!("README.md is an unsupported {other:?}"),
            ));
        }
    }

    // Validate existing configuration before any write.
    let config_kind = services.files.kind(ROOT_CONFIG_PATH).map_err(io)?;
    match config_kind {
        FileKind::Missing => report.files.push(FileAction {
            path: ROOT_CONFIG_PATH.to_string(),
            action: "create",
        }),
        FileKind::Regular => {
            let bytes = services.files.read(ROOT_CONFIG_PATH).map_err(io)?;
            let config = services.config.parse_root(&bytes).map_err(|message| {
                AppError::new(
                    ExitClass::Validation,
                    Diagnostic::error("configuration_invalid", message).at_path(ROOT_CONFIG_PATH),
                )
            })?;
            let problems = crate::snapshot::validate_root_config(services, &config)?;
            if !problems.is_empty() {
                return Err(AppError::many(ExitClass::Validation, problems));
            }
            report.existing.push(ROOT_CONFIG_PATH.to_string());
            report.files.push(FileAction {
                path: ROOT_CONFIG_PATH.to_string(),
                action: "keep",
            });
        }
        other => {
            return Err(AppError::validation(
                "configuration_invalid",
                format!("{ROOT_CONFIG_PATH} is an unsupported {other:?}"),
            ));
        }
    }

    let state_exists = match services.state.load() {
        Ok(Some(loaded)) => {
            crate::snapshot::validate_state(services, &loaded.state)?;
            report.existing.push(STATE_PATH.to_string());
            report.files.push(FileAction {
                path: STATE_PATH.to_string(),
                action: "keep",
            });
            true
        }
        Ok(None) => {
            report.files.push(FileAction {
                path: STATE_PATH.to_string(),
                action: "create",
            });
            false
        }
        Err(failure) => return Err(state_failure(failure)),
    };

    if !apply {
        // The preview writes nothing, not even when the project is empty.
        return Ok(Outcome::new(report, vec![]));
    }

    if !report.root_readme_present {
        return Err(AppError::new(
            ExitClass::Validation,
            Diagnostic::error(
                "root_readme_missing",
                "the project root has no README.md; write the root README first, then run `memoria init --apply`",
            )
            .at_path("README.md"),
        )
        .with_data(report.to_detail()));
    }

    let _guard = acquire_lock(services)?;
    services.progress.note(&format!(
        "init --apply: creating missing files under {}",
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
    if !state_exists {
        match services.state.save(&ReviewState::empty(), None) {
            Ok(_) => report.created.push(STATE_PATH.to_string()),
            Err(crate::ports::StateFailure::Conflict) => failures.push(
                Diagnostic::error(
                    "state_conflict",
                    "an unrelated memoria.lock appeared during init; resolve it explicitly",
                )
                .at_path(STATE_PATH),
            ),
            Err(failure) => failures
                .push(Diagnostic::error(failure.code(), failure.message()).at_path(STATE_PATH)),
        }
    }
    if !failures.is_empty() {
        // Partial I/O failure still reports exactly which files reached disk.
        return Err(AppError::many(ExitClass::Io, failures).with_data(report.to_detail()));
    }
    Ok(Outcome::new(report, vec![]))
}
