//! Absolute skill destinations for each target and scope.
//!
//! Codex documents `.agents/skills` for repository and user installation.
//! Its user destination does not follow `CODEX_HOME`. Claude documents
//! project and personal skill directories, and its personal directory
//! follows `CLAUDE_CONFIG_DIR` when that variable holds an absolute path.

use std::path::{Path, PathBuf};

use memoria_application::ports::{AgentLocations, AgentScope, AgentTarget, LocationError};

/// The recognized legacy Codex user location. Reported, never modified.
pub const LEGACY_CODEX_PARENT: &str = ".codex/skills";

pub struct EnvAgentLocations {
    /// The selected Git worktree root, when the command runs inside one.
    worktree: Option<PathBuf>,
    /// The directory an explicit `--path` resolves against.
    invocation_dir: PathBuf,
    home: Option<PathBuf>,
    claude_config_dir: Option<String>,
}

impl EnvAgentLocations {
    pub fn from_environment(
        worktree: Option<PathBuf>,
        invocation_dir: PathBuf,
    ) -> EnvAgentLocations {
        EnvAgentLocations {
            worktree,
            invocation_dir,
            home: std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            claude_config_dir: std::env::var("CLAUDE_CONFIG_DIR")
                .ok()
                .filter(|value| !value.is_empty()),
        }
    }

    /// An explicit construction for tests and isolated runs.
    pub fn new(
        worktree: Option<PathBuf>,
        invocation_dir: PathBuf,
        home: Option<PathBuf>,
        claude_config_dir: Option<String>,
    ) -> EnvAgentLocations {
        EnvAgentLocations {
            worktree,
            invocation_dir,
            home,
            claude_config_dir,
        }
    }

    fn home(&self) -> Result<&Path, LocationError> {
        self.home.as_deref().ok_or_else(|| LocationError {
            code: "home_unset",
            message: "HOME is not set, so no global skills directory can be resolved. Set HOME or pass --path with an absolute directory."
                .to_string(),
        })
    }

    fn worktree(&self) -> Result<&Path, LocationError> {
        self.worktree.as_deref().ok_or_else(|| LocationError {
            code: "worktree_required",
            message: "a local skill operation needs a Git worktree; run it inside one or pass --scope global"
                .to_string(),
        })
    }
}

impl AgentLocations for EnvAgentLocations {
    fn skills_parent(
        &self,
        target: AgentTarget,
        scope: AgentScope,
    ) -> Result<String, LocationError> {
        let path = match (target, scope) {
            (_, AgentScope::Local) => self.worktree()?.join(target.default_parent()),
            (AgentTarget::Codex, AgentScope::Global) => {
                // Codex's user destination does not follow CODEX_HOME.
                self.home()?.join(".agents").join("skills")
            }
            (AgentTarget::Claude, AgentScope::Global) => match &self.claude_config_dir {
                Some(configured) => {
                    let configured = PathBuf::from(configured);
                    if !configured.is_absolute() {
                        return Err(LocationError {
                            code: "claude_config_dir_relative",
                            message: format!(
                                "CLAUDE_CONFIG_DIR is {}, which is not absolute. Set it to an absolute directory or pass --path.",
                                configured.display()
                            ),
                        });
                    }
                    configured.join("skills")
                }
                None => self.home()?.join(".claude").join("skills"),
            },
        };
        Ok(path.display().to_string())
    }

    fn legacy_parent(&self, target: AgentTarget) -> Option<String> {
        match target {
            AgentTarget::Codex => self
                .home
                .as_ref()
                .map(|home| home.join(LEGACY_CODEX_PARENT).display().to_string()),
            AgentTarget::Claude => None,
        }
    }

    fn resolve_custom(&self, raw: &str, scope: AgentScope) -> Result<String, LocationError> {
        let candidate = Path::new(raw);
        match scope {
            AgentScope::Global => {
                // The CLI never infers global intent from a path, and a
                // global custom path must be unambiguous.
                if !candidate.is_absolute() {
                    return Err(LocationError {
                        code: "path_not_absolute",
                        message: format!("--path {raw} must be absolute for --scope global"),
                    });
                }
                Ok(candidate.display().to_string())
            }
            AgentScope::Local => {
                let worktree = self.worktree()?;
                let absolute = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    self.invocation_dir.join(candidate)
                };
                // Resolve the deepest existing ancestor so a path cannot use
                // a symlink or `..` to leave the worktree.
                let resolved = resolve_existing_ancestor(&absolute);
                let root = worktree.canonicalize().unwrap_or_else(|_| worktree.into());
                if !resolved.starts_with(&root) {
                    return Err(LocationError {
                        code: "path_outside_worktree",
                        message: format!(
                            "--path {raw} resolves to {}, which is outside the selected worktree {}. A path outside the worktree never implies a global installation: pass --scope global explicitly.",
                            resolved.display(),
                            root.display()
                        ),
                    });
                }
                Ok(absolute.display().to_string())
            }
        }
    }

    fn worktree_root(&self) -> Option<String> {
        self.worktree
            .as_ref()
            .map(|path| path.display().to_string())
    }
}

/// Canonicalize the deepest existing ancestor and re-append the rest, so a
/// destination that does not exist yet still cannot escape through a symlink.
fn resolve_existing_ancestor(path: &Path) -> PathBuf {
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut out = canonical;
            for part in remainder.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                remainder.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locations(dir: &Path) -> EnvAgentLocations {
        EnvAgentLocations::new(
            Some(dir.join("work")),
            dir.join("work"),
            Some(dir.join("home")),
            None,
        )
    }

    #[test]
    fn local_and_global_destinations_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let l = locations(dir.path());
        assert!(
            l.skills_parent(AgentTarget::Codex, AgentScope::Local)
                .unwrap()
                .ends_with("work/.agents/skills")
        );
        assert!(
            l.skills_parent(AgentTarget::Claude, AgentScope::Local)
                .unwrap()
                .ends_with("work/.claude/skills")
        );
        assert!(
            l.skills_parent(AgentTarget::Codex, AgentScope::Global)
                .unwrap()
                .ends_with("home/.agents/skills")
        );
        assert!(
            l.skills_parent(AgentTarget::Claude, AgentScope::Global)
                .unwrap()
                .ends_with("home/.claude/skills")
        );
        assert!(
            l.legacy_parent(AgentTarget::Codex)
                .unwrap()
                .ends_with("home/.codex/skills")
        );
        assert_eq!(l.legacy_parent(AgentTarget::Claude), None);
    }

    #[test]
    fn claude_config_dir_overrides_only_when_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = EnvAgentLocations::new(
            None,
            dir.path().to_path_buf(),
            Some(dir.path().join("home")),
            Some("/opt/claude".to_string()),
        );
        assert_eq!(
            absolute
                .skills_parent(AgentTarget::Claude, AgentScope::Global)
                .unwrap(),
            "/opt/claude/skills"
        );
        let relative = EnvAgentLocations::new(
            None,
            dir.path().to_path_buf(),
            Some(dir.path().join("home")),
            Some("relative/claude".to_string()),
        );
        let err = relative
            .skills_parent(AgentTarget::Claude, AgentScope::Global)
            .unwrap_err();
        assert_eq!(err.code, "claude_config_dir_relative");
    }

    #[test]
    fn an_unset_home_is_an_actionable_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let l = EnvAgentLocations::new(None, dir.path().to_path_buf(), None, None);
        assert_eq!(
            l.skills_parent(AgentTarget::Codex, AgentScope::Global)
                .unwrap_err()
                .code,
            "home_unset"
        );
    }

    #[test]
    fn a_local_custom_path_must_stay_inside_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("tools")).unwrap();
        let l = EnvAgentLocations::new(Some(work.clone()), work.clone(), None, None);
        assert!(l.resolve_custom("tools", AgentScope::Local).is_ok());
        let outside = l
            .resolve_custom("../elsewhere", AgentScope::Local)
            .unwrap_err();
        assert_eq!(outside.code, "path_outside_worktree");
        // The same path never implies a global installation.
        assert!(
            l.resolve_custom("/tmp/anything", AgentScope::Local)
                .is_err()
        );
        assert!(
            l.resolve_custom("/tmp/anything", AgentScope::Global)
                .is_ok()
        );
        assert_eq!(
            l.resolve_custom("relative", AgentScope::Global)
                .unwrap_err()
                .code,
            "path_not_absolute"
        );
    }
}
