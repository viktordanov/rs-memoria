//! Source selection: reserved exclusions and inherited Memoria rules.

use std::fmt;

use crate::glob::Glob;
use crate::path::{DirPath, ProjectPath};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Ignore,
    Include,
}

impl fmt::Display for RuleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleKind::Ignore => f.write_str("ignore"),
            RuleKind::Include => f.write_str("include"),
        }
    }
}

/// Memoria rules declared by the root configuration or one sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleScope {
    /// Directory containing the configuration. Root rules use the root.
    pub scope: DirPath,
    pub ignore: Vec<Glob>,
    pub include: Vec<Glob>,
}

impl RuleScope {
    pub fn applies_to(&self, path: &ProjectPath) -> bool {
        path.is_within(&self.scope)
    }
}

/// Why a path is excluded from source inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusion {
    /// A tool-owned or documentation file that can never be a source input.
    Reserved(String),
    /// The last matching rule was an ignore rule.
    Rule { scope: DirPath, pattern: String },
}

/// One matched rule in the evaluation chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionStep {
    pub scope: DirPath,
    pub kind: RuleKind,
    pub pattern: String,
}

/// The explainable outcome of selection for one eligible path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionDecision {
    pub path: ProjectPath,
    pub exclusion: Option<Exclusion>,
    /// Every rule that matched, in evaluation order (root first).
    pub steps: Vec<SelectionStep>,
}

impl SelectionDecision {
    pub fn selected(&self) -> bool {
        self.exclusion.is_none()
    }
}

/// Decide selection for one Git-eligible path.
///
/// `reserved` names the reserved category when the path can never be a
/// source input. `scopes` must be ordered from the root to the nearest scope.
/// Within one scope, ignores apply first and includes second; a later scope
/// overrides an earlier one.
pub fn decide(
    path: &ProjectPath,
    reserved: Option<String>,
    scopes: &[RuleScope],
) -> SelectionDecision {
    if let Some(category) = reserved {
        return SelectionDecision {
            path: path.clone(),
            exclusion: Some(Exclusion::Reserved(category)),
            steps: Vec::new(),
        };
    }
    let mut steps = Vec::new();
    let mut exclusion: Option<Exclusion> = None;
    for scope in scopes {
        if !scope.applies_to(path) {
            continue;
        }
        let Some(relative) = path.strip_dir(&scope.scope) else {
            continue;
        };
        for glob in &scope.ignore {
            if glob.matches(relative) {
                steps.push(SelectionStep {
                    scope: scope.scope.clone(),
                    kind: RuleKind::Ignore,
                    pattern: glob.source().to_string(),
                });
                exclusion = Some(Exclusion::Rule {
                    scope: scope.scope.clone(),
                    pattern: glob.source().to_string(),
                });
            }
        }
        for glob in &scope.include {
            if glob.matches(relative) {
                steps.push(SelectionStep {
                    scope: scope.scope.clone(),
                    kind: RuleKind::Include,
                    pattern: glob.source().to_string(),
                });
                exclusion = None;
            }
        }
    }
    SelectionDecision {
        path: path.clone(),
        exclusion,
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(dir: &str, ignore: &[&str], include: &[&str]) -> RuleScope {
        RuleScope {
            scope: DirPath::parse(dir).unwrap(),
            ignore: ignore.iter().map(|p| Glob::parse(p).unwrap()).collect(),
            include: include.iter().map(|p| Glob::parse(p).unwrap()).collect(),
        }
    }

    #[test]
    fn root_ignore_excludes_and_local_include_restores() {
        let scopes = vec![
            scope("", &["**/generated/**", "**/fixtures/**"], &[]),
            scope("src/retrieval", &[], &["fixtures/**"]),
        ];
        let fixture = ProjectPath::parse("src/retrieval/fixtures/sample.txt").unwrap();
        let decision = decide(&fixture, None, &scopes);
        assert!(decision.selected());
        assert_eq!(decision.steps.len(), 2);
        assert_eq!(decision.steps[1].kind, RuleKind::Include);

        let generated = ProjectPath::parse("src/retrieval/generated/table.rs").unwrap();
        let decision = decide(&generated, None, &scopes);
        assert_eq!(
            decision.exclusion,
            Some(Exclusion::Rule {
                scope: DirPath::root(),
                pattern: "**/generated/**".to_string()
            })
        );

        let other = ProjectPath::parse("src/other/fixtures/sample.txt").unwrap();
        assert!(!decide(&other, None, &scopes).selected());
    }

    #[test]
    fn same_scope_include_wins_over_ignore() {
        let scopes = vec![scope("", &["docs/**"], &["docs/keep.md"])];
        assert!(decide(&ProjectPath::parse("docs/keep.md").unwrap(), None, &scopes).selected());
        assert!(!decide(&ProjectPath::parse("docs/drop.md").unwrap(), None, &scopes).selected());
    }

    #[test]
    fn nearer_ignore_overrides_root_include() {
        let scopes = vec![scope("", &[], &["**/*.log"]), scope("src", &["*.log"], &[])];
        assert!(!decide(&ProjectPath::parse("src/a.log").unwrap(), None, &scopes).selected());
        assert!(decide(&ProjectPath::parse("a.log").unwrap(), None, &scopes).selected());
    }

    #[test]
    fn reserved_cannot_be_restored() {
        let scopes = vec![scope("", &[], &["**"])];
        let decision = decide(
            &ProjectPath::parse("memoria.yml").unwrap(),
            Some("configuration".to_string()),
            &scopes,
        );
        assert_eq!(
            decision.exclusion,
            Some(Exclusion::Reserved("configuration".to_string()))
        );
    }
}
