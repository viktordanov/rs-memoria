//! `memoria integrations github install|status|upgrade|uninstall`.
//!
//! The CLI creates and maintains one consumer GitHub Actions workflow. That
//! workflow calls the first-party setup Action, which installs a verified
//! prebuilt executable on the runner. The two parts are complementary: this
//! use case never downloads a binary, and the Action never changes a project.
//!
//! Every mutating operation shows a preview first. `--apply` performs the
//! change. An apply recomputes its plan from the current bytes, so an earlier
//! preview never authorizes an overwrite of a later edit.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::{
    ConfigurationReader, FileKind, ProjectFiles, Services, StateStore, WorkflowFailure,
    WorkflowOperation, WorkflowPlan, WorkflowRequest, WorkflowStore,
};

/// The runner labels the setup Action supports.
pub const RUNNERS: [&str; 3] = ["ubuntu-24.04", "ubuntu-latest", "ubuntu-24.04-arm"];

/// The runner an installation selects when the caller names none.
pub const DEFAULT_RUNNER: &str = "ubuntu-24.04";

/// The first Memoria version the setup Action can install.
pub const MINIMUM_VERSION: (u64, u64, u64) = (0, 5, 0);

/// The public repository that owns the root setup Action.
pub const ACTION_REPOSITORY: &str = "viktordanov/rs-memoria";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowReport {
    pub plan: WorkflowPlan,
    pub operation: WorkflowOperation,
    pub applied: bool,
    pub dry_run: bool,
    /// The command that would perform this preview.
    pub apply_command: Option<String>,
    /// Initialization requirements this project does not meet yet.
    pub missing_prerequisites: Vec<Prerequisite>,
}

impl WorkflowReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("operation", self.operation.as_str())
            .with("plan", plan_detail(&self.plan))
            .bool("applied", self.applied)
            .bool("dry_run", self.dry_run)
            .with(
                "apply_command",
                Detail::option_text(self.apply_command.clone()),
            )
            .with(
                "missing_prerequisites",
                prerequisite_detail(&self.missing_prerequisites),
            )
            .build()
    }
}

pub fn plan_detail(plan: &WorkflowPlan) -> Detail {
    DetailMap::default()
        .text("path", plan.path.clone())
        .text("record_path", plan.record_path.clone())
        .text("state", plan.state.clone())
        .with(
            "installed_version",
            Detail::option_text(plan.installed_version.clone()),
        )
        .with(
            "installed_action_ref",
            Detail::option_text(plan.installed_action_ref.clone()),
        )
        .with(
            "installed_runner",
            Detail::option_text(plan.installed_runner.clone()),
        )
        .with(
            "installed_template",
            match plan.installed_template {
                Some(version) => Detail::Number(version),
                None => Detail::Null,
            },
        )
        .text("desired_version", plan.desired_version.clone())
        .text("desired_action_ref", plan.desired_action_ref.clone())
        .text("desired_runner", plan.desired_runner.clone())
        .number("desired_template", plan.desired_template)
        .with("workflow", Detail::option_text(plan.rendered.clone()))
        .with("writes", Detail::texts(plan.writes.clone()))
        .with("removals", Detail::texts(plan.removals.clone()))
        .bool("no_change", plan.no_change)
        .bool("recovery_needed", plan.recovery_needed)
        .with(
            "retained_artifacts",
            Detail::list(plan.retained_artifacts.iter().map(|artifact| {
                DetailMap::default()
                    .text("path", artifact.path.clone())
                    .text("reason", artifact.reason)
                    .bool("removable_by_uninstall", artifact.removable_by_uninstall)
                    .build()
            })),
        )
        .with("siblings", Detail::texts(plan.siblings.clone()))
        .with("notes", Detail::texts(plan.notes.clone()))
        .build()
}

/// One initialization requirement that install and upgrade need.
///
/// The workflow the CLI writes runs `memoria check` on a runner. A project
/// without an authored README, a valid `memoria.toml`, and a readable
/// `memoria.lock` cannot answer that command, so creating the workflow would
/// hand the consumer a job that can only fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prerequisite {
    /// Stable code: `readme_missing`, `configuration_missing`,
    /// `configuration_invalid`, `state_missing`, or the state failure code.
    pub code: &'static str,
    /// The project-relative path the requirement concerns.
    pub path: &'static str,
    pub problem: String,
}

impl Prerequisite {
    fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("code", self.code)
            .text("path", self.path)
            .text("problem", self.problem.clone())
            .build()
    }
}

/// Inspect the three initialization requirements. This never writes.
///
/// A pending review is permitted: review follows the workflow change. This
/// function therefore reads setup state only; it never runs `check`.
pub fn prerequisites(
    files: &dyn ProjectFiles,
    config: &dyn ConfigurationReader,
    state: &dyn StateStore,
) -> Result<Vec<Prerequisite>, AppError> {
    let mut missing = Vec::new();

    match files.kind("README.md") {
        Ok(FileKind::Regular) => {}
        Ok(other) => missing.push(Prerequisite {
            code: "readme_missing",
            path: "README.md",
            problem: match other {
                FileKind::Missing => "the root README does not exist".to_string(),
                _ => format!("the root README is a {other:?}, not a regular file"),
            },
        }),
        Err(err) => return Err(AppError::io("io_error", err.to_string())),
    }

    match files.kind("memoria.toml") {
        Ok(FileKind::Regular) => match files.read("memoria.toml") {
            Ok(bytes) => {
                if let Err(problem) = config.parse_root(&bytes) {
                    missing.push(Prerequisite {
                        code: "configuration_invalid",
                        path: "memoria.toml",
                        problem,
                    });
                }
            }
            Err(err) => return Err(AppError::io("io_error", err.to_string())),
        },
        Ok(other) => missing.push(Prerequisite {
            code: "configuration_missing",
            path: "memoria.toml",
            problem: match other {
                FileKind::Missing => "the project configuration does not exist".to_string(),
                _ => format!("memoria.toml is a {other:?}, not a regular file"),
            },
        }),
        Err(err) => return Err(AppError::io("io_error", err.to_string())),
    }

    match state.load() {
        Ok(Some(_)) => {}
        Ok(None) => missing.push(Prerequisite {
            code: "state_missing",
            path: "memoria.lock",
            problem: "the committed review state does not exist".to_string(),
        }),
        Err(failure) => missing.push(Prerequisite {
            code: failure.code(),
            path: "memoria.lock",
            problem: failure.message(),
        }),
    }

    Ok(missing)
}

fn prerequisite_detail(missing: &[Prerequisite]) -> Detail {
    Detail::list(missing.iter().map(Prerequisite::to_detail))
}

/// Everything one workflow command needs after argument parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowArgs {
    pub operation: WorkflowOperation,
    pub path: Option<String>,
    pub version: Option<String>,
    pub action_ref: Option<String>,
    pub runner: Option<String>,
    pub apply: bool,
    pub dry_run: bool,
}

fn workflow_error(failure: WorkflowFailure) -> AppError {
    match failure {
        WorkflowFailure::Conflict {
            code,
            message,
            paths,
        } => AppError::new(
            ExitClass::Conflict,
            Diagnostic::error(code, message).with_details(
                DetailMap::default()
                    .with("paths", Detail::texts(paths))
                    .build(),
            ),
        ),
        WorkflowFailure::Usage { code, message } => AppError::usage(code, message),
        WorkflowFailure::NotInstalled(message) => {
            AppError::validation("github_not_installed", message)
        }
        WorkflowFailure::UpgradeRequired(message) => {
            AppError::validation("github_upgrade_required", message)
        }
        WorkflowFailure::Io(err) => AppError::io("io_error", err.to_string()),
    }
}

/// Parse an exact stable version, with an optional leading `v`.
pub fn parse_version(raw: &str) -> Result<String, AppError> {
    let text = raw.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let refuse = || {
        AppError::usage(
            "version_invalid",
            format!(
                "--version {raw} is not an exact stable version; give MAJOR.MINOR.PATCH such as 0.5.0"
            ),
        )
    };
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 3 {
        return Err(refuse());
    }
    let mut numbers = [0u64; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
            return Err(refuse());
        }
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse());
        }
        numbers[index] = part.parse().map_err(|_| refuse())?;
    }
    let value = (numbers[0], numbers[1], numbers[2]);
    if value < MINIMUM_VERSION {
        let (major, minor, patch) = MINIMUM_VERSION;
        return Err(AppError::usage(
            "version_unsupported",
            format!(
                "--version {raw} is below {major}.{minor}.{patch}; the setup Action installs {major}.{minor}.{patch} and later"
            ),
        ));
    }
    Ok(format!("{}.{}.{}", value.0, value.1, value.2))
}

/// Order two exact stable versions.
pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    fn triple(value: &str) -> (u64, u64, u64) {
        let mut parts = value.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }
    triple(left).cmp(&triple(right))
}

/// Whether a reference is a full 40-character lowercase commit SHA.
pub fn is_commit_ref(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Parse an Action code reference: a full commit SHA or an exact `vX.Y.Z` tag.
pub fn parse_action_ref(raw: &str) -> Result<String, AppError> {
    let text = raw.trim();
    if is_commit_ref(text) {
        return Ok(text.to_string());
    }
    if let Some(rest) = text.strip_prefix('v')
        && rest.split('.').count() == 3
        && rest
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && rest
            .split('.')
            .all(|part| part.len() == 1 || !part.starts_with('0'))
    {
        return Ok(text.to_string());
    }
    Err(AppError::usage(
        "action_ref_invalid",
        format!(
            "--action-ref {raw} must be a full 40-character commit SHA or an exact release tag such as v0.5.0"
        ),
    ))
}

/// Validate a runner label against the labels the setup Action supports.
pub fn parse_runner(raw: &str) -> Result<String, AppError> {
    let text = raw.trim();
    if RUNNERS.contains(&text) {
        return Ok(text.to_string());
    }
    Err(AppError::usage(
        "runner_invalid",
        format!(
            "--runner {raw} is not supported; the setup Action supports {}",
            RUNNERS.join(", ")
        ),
    ))
}

/// Validate an explicit `--path`: one direct `.yml` or `.yaml` child of
/// `.github/workflows`, given as a project-relative path.
pub fn parse_path(raw: &str) -> Result<String, AppError> {
    let text = raw.trim();
    let refuse = |reason: &str| {
        AppError::usage(
            "workflow_path_invalid",
            format!("--path {raw} {reason}; give one .yml or .yaml file under .github/workflows"),
        )
    };
    if text.is_empty() {
        return Err(refuse("is empty"));
    }
    if text.starts_with('/') || text.starts_with('\\') || text.contains('\\') {
        return Err(refuse("is not a project-relative path"));
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(refuse("holds a control character"));
    }
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() != 3 || parts[0] != ".github" || parts[1] != "workflows" {
        return Err(refuse("is not a direct child of .github/workflows"));
    }
    let name = parts[2];
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return Err(refuse("does not name a workflow file"));
    }
    if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
        return Err(refuse("does not end with .yml or .yaml"));
    }
    Ok(text.to_string())
}

/// The command that performs the previewed change.
fn apply_command(args: &WorkflowArgs, default_path: &str) -> String {
    let mut command = format!("memoria integrations github {}", args.operation.as_str());
    if let Some(path) = &args.path
        && path != default_path
    {
        command.push_str(&format!(" --path {path}"));
    }
    if let Some(version) = &args.version {
        command.push_str(&format!(" --version {version}"));
    }
    if let Some(reference) = &args.action_ref {
        command.push_str(&format!(" --action-ref {reference}"));
    }
    if let Some(runner) = &args.runner {
        command.push_str(&format!(" --runner {runner}"));
    }
    command.push_str(" --apply");
    command
}

/// Build the adapter request from validated arguments.
pub fn build_request(
    workflows: &dyn WorkflowStore,
    args: &WorkflowArgs,
) -> Result<WorkflowRequest, AppError> {
    if args.apply && args.dry_run {
        return Err(AppError::usage(
            "apply_conflict",
            "--apply performs the change and --dry-run previews it; give one of them",
        ));
    }
    if args.operation == WorkflowOperation::Status && (args.apply || args.dry_run) {
        return Err(AppError::usage(
            "status_read_only",
            "status is always read-only; it accepts neither --apply nor --dry-run",
        ));
    }
    if args.operation == WorkflowOperation::Uninstall
        && (args.version.is_some() || args.action_ref.is_some() || args.runner.is_some())
    {
        return Err(AppError::usage(
            "uninstall_arguments_invalid",
            "uninstall removes the managed workflow; it accepts neither --version, --action-ref nor --runner",
        ));
    }
    if args.operation == WorkflowOperation::Status
        && (args.version.is_some() || args.action_ref.is_some() || args.runner.is_some())
    {
        return Err(AppError::usage(
            "status_arguments_invalid",
            "status reports the installed and desired values; it accepts none of --version, --action-ref or --runner",
        ));
    }
    let path = match &args.path {
        Some(raw) => parse_path(raw)?,
        None => workflows.default_path().to_string(),
    };
    let version = args.version.as_deref().map(parse_version).transpose()?;
    let action_ref = args
        .action_ref
        .as_deref()
        .map(parse_action_ref)
        .transpose()?;
    let runner = args.runner.as_deref().map(parse_runner).transpose()?;
    Ok(WorkflowRequest {
        operation: args.operation,
        path,
        version,
        action_ref,
        runner,
        apply: args.apply,
    })
}

pub fn run(
    services: &Services<'_>,
    args: &WorkflowArgs,
) -> Result<Outcome<WorkflowReport>, AppError> {
    // Status and uninstall stay independent of configuration validity: an
    // uninitialized or broken project must still be able to see and remove a
    // managed workflow.
    let missing = match args.operation {
        WorkflowOperation::Install | WorkflowOperation::Upgrade => {
            prerequisites(services.files, services.config, services.state)?
        }
        WorkflowOperation::Status | WorkflowOperation::Uninstall => Vec::new(),
    };
    run_with(services.workflows, services.progress, args, &missing)
}

pub fn run_with(
    workflows: &dyn WorkflowStore,
    progress: &dyn crate::ports::Progress,
    args: &WorkflowArgs,
    missing: &[Prerequisite],
) -> Result<Outcome<WorkflowReport>, AppError> {
    let request = build_request(workflows, args)?;
    // An apply stops here, before the store opens a lock, recovers a
    // transaction, or writes anything. Memoria never initializes a project by
    // itself.
    if request.apply && !missing.is_empty() {
        let paths: Vec<String> = missing.iter().map(|item| item.path.to_string()).collect();
        return Err(AppError::new(
            ExitClass::Validation,
            Diagnostic::error(
                "github_prerequisites_missing",
                format!(
                    "this project is not initialized for `memoria check`, so the workflow would only fail: {}. \
                     Author a root README, run `memoria init --apply`, then run this command again.",
                    missing
                        .iter()
                        .map(|item| format!("{} ({})", item.path, item.problem))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            )
            .with_details(
                DetailMap::default()
                    .with("paths", Detail::texts(paths))
                    .with("prerequisites", prerequisite_detail(missing))
                    .build(),
            ),
        ));
    }
    let plan = workflows.plan(&request).map_err(workflow_error)?;
    let mut diagnostics = Vec::new();
    if !plan.siblings.is_empty() {
        diagnostics.push(
            Diagnostic::hint(
                "github_sibling_workflows",
                format!(
                    "{} other workflow files already exist beside this destination. Memoria reads none of them and can duplicate an existing documentation check.",
                    plan.siblings.len()
                ),
            )
            .with_details(
                DetailMap::default()
                    .with("workflows", Detail::texts(plan.siblings.clone()))
                    .build(),
            ),
        );
    }
    for item in missing {
        diagnostics.push(
            Diagnostic::warning(
                "github_prerequisite_missing",
                format!(
                    "{} is required before this workflow can run `memoria check`: {}",
                    item.path, item.problem
                ),
            )
            .at_path(item.path)
            .with_details(item.to_detail()),
        );
    }
    if plan.recovery_needed {
        diagnostics.push(Diagnostic::warning(
            "github_recovery_needed",
            "an interrupted Memoria workflow transaction left recovery state; inspect the reported paths before another change",
        ));
    }
    if request.operation == WorkflowOperation::Status || !request.apply {
        let preview_only = request.operation != WorkflowOperation::Status;
        if preview_only {
            diagnostics.push(Diagnostic::hint(
                "github_publication_unverified",
                format!(
                    "Memoria did not contact {ACTION_REPOSITORY}; it cannot confirm that the release assets and the Action reference in this plan are published."
                ),
            ));
        }
        let command = preview_only.then(|| apply_command(args, workflows.default_path()));
        return Ok(Outcome::new(
            WorkflowReport {
                plan,
                operation: request.operation,
                applied: false,
                dry_run: args.dry_run,
                apply_command: command,
                missing_prerequisites: missing.to_vec(),
            },
            diagnostics,
        ));
    }
    // An apply with nothing to write still has to settle an interrupted
    // transaction. Returning early here would leave that record in place.
    if plan.no_change && !plan.recovery_needed {
        return Ok(Outcome::new(
            WorkflowReport {
                plan,
                operation: request.operation,
                applied: false,
                dry_run: false,
                apply_command: None,
                missing_prerequisites: missing.to_vec(),
            },
            diagnostics,
        ));
    }
    if !plan.no_change {
        progress.note(&format!(
            "integrations github {}: {} writes [{}] removals [{}]",
            request.operation.as_str(),
            plan.path,
            plan.writes.join(", "),
            plan.removals.join(", "),
        ));
    }
    let applied = workflows.apply(&request, &plan).map_err(workflow_error)?;
    // Recovery alone changes no managed file. The report says what happened.
    let wrote = !applied.no_change;
    Ok(Outcome::new(
        WorkflowReport {
            plan: applied,
            operation: request.operation,
            applied: wrote,
            dry_run: false,
            apply_command: None,
            missing_prerequisites: missing.to_vec(),
        },
        diagnostics,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_exact_stable_versions_only() {
        assert_eq!(parse_version("0.5.0").unwrap(), "0.5.0");
        assert_eq!(parse_version("v1.10.0").unwrap(), "1.10.0");
        for raw in ["latest", "0.5", "^0.5.0", "0.5.0-rc.1", "0.05.0", ""] {
            assert!(parse_version(raw).is_err(), "{raw} must be refused");
        }
        // Every version below the floor is refused, 0.4.1 included.
        for raw in ["0.4.1", "0.4.0", "0.3.0"] {
            assert_eq!(
                parse_version(raw).unwrap_err().diagnostics[0].code,
                "version_unsupported",
                "{raw} must be refused"
            );
        }
    }

    #[test]
    fn accepts_commit_and_tag_references_only() {
        let sha = "a".repeat(40);
        assert_eq!(parse_action_ref(&sha).unwrap(), sha);
        assert_eq!(parse_action_ref("v0.5.0").unwrap(), "v0.5.0");
        for raw in ["main", "0.5.0", "v0.5", &"A".repeat(40), &"a".repeat(39)] {
            assert!(parse_action_ref(raw).is_err(), "{raw} must be refused");
        }
    }

    #[test]
    fn accepts_documented_runner_labels_only() {
        for label in RUNNERS {
            assert_eq!(parse_runner(label).unwrap(), label);
        }
        for label in [
            "ubuntu-22.04",
            "ubuntu-latest-arm",
            "macos-14",
            "windows-latest",
        ] {
            assert!(parse_runner(label).is_err(), "{label} must be refused");
        }
    }

    #[test]
    fn accepts_one_direct_workflow_child_only() {
        assert_eq!(
            parse_path(".github/workflows/docs.yaml").unwrap(),
            ".github/workflows/docs.yaml"
        );
        for raw in [
            "/etc/passwd",
            ".github/workflows/../../escape.yml",
            ".github/workflows/nested/deep.yml",
            ".github/memoria.yml",
            ".github/workflows/memoria.txt",
            ".github/workflows/.hidden.yml",
        ] {
            assert!(parse_path(raw).is_err(), "{raw} must be refused");
        }
    }

    #[test]
    fn orders_versions_numerically() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.5.0", "0.5.1"), Ordering::Less);
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
    }
}
