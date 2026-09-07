//! `memoria agent install|uninstall`: managed skill packages.

use crate::error::{AppError, Detail, DetailMap, ExitClass, Outcome};
use crate::ports::{AgentTarget, Services, SkillFailure, SkillOperation, SkillPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    pub plan: SkillPlan,
    pub applied: bool,
    pub dry_run: bool,
}

pub fn plan_detail(plan: &SkillPlan) -> Detail {
    DetailMap::default()
        .text(
            "operation",
            match plan.operation {
                SkillOperation::Install => "install",
                SkillOperation::Uninstall => "uninstall",
            },
        )
        .text("target", plan.target.as_str())
        .text("destination", plan.destination.clone())
        .with("backup", Detail::option_text(plan.backup.clone()))
        .with("writes", Detail::texts(plan.writes.clone()))
        .with("removals", Detail::texts(plan.removals.clone()))
        .with("replaced", Detail::texts(plan.replaced.clone()))
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
    }
}

pub fn run(
    services: &Services<'_>,
    operation: SkillOperation,
    target: Option<AgentTarget>,
    path: Option<&str>,
    dry_run: bool,
) -> Result<Outcome<AgentReport>, AppError> {
    if path.is_some() && target.is_none() {
        return Err(AppError::usage(
            "target_required",
            "--path requires --target codex|claude",
        ));
    }
    let target = match target {
        Some(target) => target,
        None => {
            let detected: Vec<AgentTarget> = [AgentTarget::Codex, AgentTarget::Claude]
                .into_iter()
                .filter(|t| services.skills.directory_exists(t.detection_dir()))
                .collect();
            match detected.as_slice() {
                [single] => *single,
                [] => {
                    return Err(AppError::usage(
                        "target_ambiguous",
                        "no .agents or .claude directory exists in the project; run `memoria agent install --target codex` or `memoria agent install --target claude`",
                    ));
                }
                _ => {
                    return Err(AppError::usage(
                        "target_ambiguous",
                        "both .agents and .claude directories exist; choose `--target codex` or `--target claude`",
                    ));
                }
            }
        }
    };
    let parent = match path {
        Some(path) => path.to_string(),
        None => format!(
            "{}/{}",
            services.files.root_display(),
            target.default_parent()
        ),
    };
    let plan = match operation {
        SkillOperation::Install => services.skills.plan_install(target, &parent),
        SkillOperation::Uninstall => services.skills.plan_uninstall(target, &parent),
    }
    .map_err(skill_error)?;
    if dry_run || plan.no_change {
        return Ok(Outcome::new(
            AgentReport {
                plan,
                applied: false,
                dry_run,
            },
            vec![],
        ));
    }
    services.progress.note(&format!(
        "agent {}: target {} destination {} writes [{}] removals [{}] replaced [{}] backup {}",
        match operation {
            SkillOperation::Install => "install",
            SkillOperation::Uninstall => "uninstall",
        },
        target.as_str(),
        plan.destination,
        plan.writes.join(", "),
        plan.removals.join(", "),
        plan.replaced.join(", "),
        plan.backup.as_deref().unwrap_or("none"),
    ));
    match operation {
        SkillOperation::Install => services.skills.apply_install(&plan),
        SkillOperation::Uninstall => services.skills.apply_uninstall(&plan),
    }
    .map_err(skill_error)?;
    Ok(Outcome::new(
        AgentReport {
            plan,
            applied: true,
            dry_run: false,
        },
        vec![],
    ))
}
