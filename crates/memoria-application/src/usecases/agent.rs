//! `memoria agent install|status|upgrade|uninstall`: managed skill packages.
//!
//! Every operation has one explicit destination and preserves user files.
//! The default scope is local. Global operations require an explicit target
//! and scope, work outside Git, and never infer global intent from a path.

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome};
use crate::ports::{
    AgentLocations, AgentScope, AgentTarget, Progress, Services, SkillFailure, SkillOperation,
    SkillPackageStore, SkillPlan, SkillRequest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    pub plan: SkillPlan,
    pub applied: bool,
    pub dry_run: bool,
}

pub fn plan_detail(plan: &SkillPlan) -> Detail {
    DetailMap::default()
        .text("operation", plan.operation.as_str())
        .text("target", plan.target.as_str())
        .text("scope", plan.scope.as_str())
        .text("destination", plan.destination.clone())
        .text("state", plan.state.clone())
        .with(
            "package_version",
            Detail::option_text(plan.package_version.clone()),
        )
        .text("embedded_version", plan.embedded_version.clone())
        .with("backup", Detail::option_text(plan.backup.clone()))
        .with("writes", Detail::texts(plan.writes.clone()))
        .with("removals", Detail::texts(plan.removals.clone()))
        .with("replaced", Detail::texts(plan.replaced.clone()))
        .with("modified_paths", Detail::texts(plan.modified_paths.clone()))
        .with("unknown_paths", Detail::texts(plan.unknown_paths.clone()))
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
        .with(
            "overlapping",
            Detail::list(plan.overlapping.iter().map(|package| {
                DetailMap::default()
                    .text("scope", package.scope.clone())
                    .text("destination", package.destination.clone())
                    .text("state", package.state.clone())
                    .text("note", package.note.clone())
                    .build()
            })),
        )
        .bool("no_change", plan.no_change)
        .bool("recovery_needed", plan.recovery_needed)
        .text("existing", plan.existing.clone())
        .build()
}

impl AgentReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .with("plan", plan_detail(&self.plan))
            .bool("applied", self.applied)
            .bool("dry_run", self.dry_run)
            .build()
    }
}

fn skill_error(failure: SkillFailure) -> AppError {
    match failure {
        SkillFailure::Conflict { message, paths } => AppError::new(
            ExitClass::Conflict,
            crate::error::Diagnostic::error("skill_conflict", message).with_details(
                DetailMap::default()
                    .with("paths", Detail::texts(paths))
                    .build(),
            ),
        ),
        SkillFailure::Io(err) => AppError::io("io_error", err.to_string()),
        SkillFailure::NotInstalled(path) => AppError::validation(
            "skill_not_installed",
            format!("no managed Memoria skill package at {path}"),
        ),
        SkillFailure::UpgradeRequired(message) => {
            AppError::validation("skill_upgrade_required", message)
        }
        SkillFailure::ReplacementRequired(message) => {
            AppError::validation("skill_replace_required", message)
        }
    }
}

/// Everything a lifecycle command needs after argument parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentArgs {
    pub operation: SkillOperation,
    pub target: Option<AgentTarget>,
    pub scope: AgentScope,
    pub path: Option<String>,
    pub dry_run: bool,
    pub replace_existing: bool,
}

/// Resolve the target, honoring local detection only when the scope is local
/// and no explicit target was given.
fn resolve_target(
    skills: &dyn SkillPackageStore,
    args: &AgentArgs,
) -> Result<AgentTarget, AppError> {
    if let Some(target) = args.target {
        return Ok(target);
    }
    if args.scope == AgentScope::Global {
        return Err(AppError::usage(
            "target_required",
            "--scope global requires an explicit --target codex|claude",
        ));
    }
    if args.path.is_some() {
        return Err(AppError::usage(
            "target_required",
            "--path requires --target codex|claude",
        ));
    }
    let detected: Vec<AgentTarget> = [AgentTarget::Codex, AgentTarget::Claude]
        .into_iter()
        .filter(|t| skills.directory_exists(t.detection_dir()))
        .collect();
    match detected.as_slice() {
        [single] => Ok(*single),
        [] => Err(AppError::usage(
            "target_ambiguous",
            "no .agents or .claude directory exists in the project; pass `--target codex` or `--target claude`",
        )),
        _ => Err(AppError::usage(
            "target_ambiguous",
            "both .agents and .claude directories exist; choose `--target codex` or `--target claude`",
        )),
    }
}

fn location_error(err: crate::ports::LocationError) -> AppError {
    AppError::usage(err.code, err.message)
}

/// Build the request: the absolute destination parent plus the other scopes
/// to report as overlapping installations.
pub fn build_request(
    skills: &dyn SkillPackageStore,
    locations: &dyn AgentLocations,
    args: &AgentArgs,
) -> Result<SkillRequest, AppError> {
    let target = resolve_target(skills, args)?;
    let parent = match &args.path {
        Some(path) => locations
            .resolve_custom(path, args.scope)
            .map_err(location_error)?,
        None => locations
            .skills_parent(target, args.scope)
            .map_err(location_error)?,
    };
    // Report the other scope and any recognized legacy location without
    // touching either one.
    let mut other_parents = Vec::new();
    let other_scope = match args.scope {
        AgentScope::Local => AgentScope::Global,
        AgentScope::Global => AgentScope::Local,
    };
    if let Ok(other) = locations.skills_parent(target, other_scope)
        && other != parent
    {
        other_parents.push((other_scope.as_str().to_string(), other));
    }
    if let Some(legacy) = locations.legacy_parent(target)
        && legacy != parent
    {
        other_parents.push(("legacy".to_string(), legacy));
    }
    Ok(SkillRequest {
        operation: args.operation,
        target,
        scope: args.scope,
        parent,
        replace_existing: args.replace_existing,
        other_parents,
    })
}

pub fn run(services: &Services<'_>, args: &AgentArgs) -> Result<Outcome<AgentReport>, AppError> {
    run_with(services.skills, services.locations, services.progress, args)
}

/// The narrow entry point. A global operation needs only these three ports,
/// so composition can resolve it before project discovery.
pub fn run_with(
    skills: &dyn SkillPackageStore,
    locations: &dyn AgentLocations,
    progress: &dyn Progress,
    args: &AgentArgs,
) -> Result<Outcome<AgentReport>, AppError> {
    let request = build_request(skills, locations, args)?;
    let plan = skills.plan(&request).map_err(skill_error)?;
    if args.operation == SkillOperation::Status || args.dry_run || plan.no_change {
        return Ok(Outcome::new(
            AgentReport {
                plan,
                applied: false,
                dry_run: args.dry_run,
            },
            vec![],
        ));
    }
    progress.note(&format!(
        "agent {}: target {} scope {} destination {} writes [{}] removals [{}] replaced [{}] backup {}",
        request.operation.as_str(),
        request.target.as_str(),
        request.scope.as_str(),
        plan.destination,
        plan.writes.join(", "),
        plan.removals.join(", "),
        plan.replaced.join(", "),
        plan.backup.as_deref().unwrap_or("none"),
    ));
    skills.apply(&request, &plan).map_err(skill_error)?;
    Ok(Outcome::new(
        AgentReport {
            plan,
            applied: true,
            dry_run: false,
        },
        vec![],
    ))
}
