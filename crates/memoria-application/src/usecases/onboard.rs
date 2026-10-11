//! `memoria onboard`: where a project stands, for the onboarding hub.
//!
//! Every probe reuses a read-only use case and is best-effort: a probe that
//! fails reports what it could, and onboarding itself never fails because a
//! project is not set up yet. Nothing here writes or takes a lock.

use crate::error::{AppError, Detail, DetailMap, Outcome};
use crate::ports::{AgentScope, AgentTarget, Services, SkillOperation, WorkflowOperation};

use super::{agent, github_workflow, status};

/// Whether the project is set up, and its review queue when it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// No `memoria.toml` yet. `root_readme` says whether the root README
    /// that `memoria init --apply` needs already exists.
    Missing { root_readme: bool },
    /// Configured and readable.
    Configured {
        documents: u64,
        /// Pending and never-reviewed documents: everything that needs a review.
        needs_review: u64,
    },
    /// Configured, but the status scan reported an error with this code.
    Broken { code: String },
}

/// One agent skill that is installed in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub target: AgentTarget,
    /// The installer's state, for example `current` or `outdated`.
    pub state: String,
}

/// The managed GitHub workflow, when Memoria could read its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub path: String,
    /// The installer's state, for example `absent`, `current`, or `outdated`.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectState {
    pub root: String,
    pub setup: Setup,
    /// Installed local skills only, Claude first.
    pub skills: Vec<Skill>,
    pub workflow: Option<Workflow>,
}

impl ProjectState {
    pub fn to_detail(&self) -> Detail {
        let setup = match &self.setup {
            Setup::Missing { root_readme } => DetailMap::default()
                .text("state", "missing")
                .bool("root_readme", *root_readme)
                .build(),
            Setup::Configured {
                documents,
                needs_review,
            } => DetailMap::default()
                .text("state", "configured")
                .number("documents", *documents)
                .number("needs_review", *needs_review)
                .build(),
            Setup::Broken { code } => DetailMap::default()
                .text("state", "broken")
                .text("code", code.clone())
                .build(),
        };
        DetailMap::default()
            .text("root", self.root.clone())
            .with("setup", setup)
            .with(
                "skills",
                Detail::list(self.skills.iter().map(|skill| {
                    DetailMap::default()
                        .text("target", skill.target.as_str())
                        .text("state", skill.state.clone())
                        .build()
                })),
            )
            .with(
                "workflow",
                match &self.workflow {
                    Some(workflow) => DetailMap::default()
                        .text("path", workflow.path.clone())
                        .text("state", workflow.state.clone())
                        .build(),
                    None => Detail::Null,
                },
            )
            .build()
    }
}

pub fn run(services: &Services<'_>) -> Result<Outcome<ProjectState>, AppError> {
    let setup = match status::run_summary(services) {
        Ok(outcome) => Setup::Configured {
            documents: outcome.data.counts.documents,
            needs_review: outcome.data.pending + outcome.data.never_reviewed,
        },
        Err(error) => {
            let code = error
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.severity == crate::error::Severity::Error)
                .map(|diagnostic| diagnostic.code.clone())
                .unwrap_or_else(|| "unknown".to_string());
            if code == "configuration_missing" {
                Setup::Missing {
                    root_readme: matches!(
                        services.files.kind("README.md"),
                        Ok(crate::ports::FileKind::Regular)
                    ),
                }
            } else {
                Setup::Broken { code }
            }
        }
    };
    let mut skills = Vec::new();
    for target in [AgentTarget::Claude, AgentTarget::Codex] {
        let request = agent::AgentArgs {
            operation: SkillOperation::Status,
            target: Some(target),
            scope: AgentScope::Local,
            path: None,
            dry_run: false,
            replace_existing: false,
        };
        if let Ok(outcome) = agent::run(services, &request)
            && outcome.data.plan.state != "absent"
        {
            skills.push(Skill {
                target,
                state: outcome.data.plan.state.clone(),
            });
        }
    }
    let workflow = github_workflow::run(
        services,
        &github_workflow::WorkflowArgs {
            operation: WorkflowOperation::Status,
            path: None,
            version: None,
            action_ref: None,
            runner: None,
            apply: false,
            dry_run: false,
        },
    )
    .ok()
    .map(|outcome| Workflow {
        path: outcome.data.plan.path.clone(),
        state: outcome.data.plan.state.clone(),
    });
    Ok(Outcome::new(
        ProjectState {
            root: services.files.root_display(),
            setup,
            skills,
            workflow,
        },
        Vec::new(),
    ))
}
