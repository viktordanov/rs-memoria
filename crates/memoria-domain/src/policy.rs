//! Effective review policy for one owner.

use crate::path::{DirPath, DocumentId};

/// Ordered effective Git ignore rules from one logical source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRuleScope {
    /// `global`, `repository`, or a root-relative `.gitignore` path.
    pub identity: String,
    /// Effective pattern lines in Git order, without blanks or comments,
    /// as their exact bytes: Git matches rule bytes, so no decoding may
    /// merge distinct rules before hashing.
    pub patterns: Vec<Vec<u8>>,
}

/// Normalized Memoria rules from one configuration scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRuleScope {
    pub scope: DirPath,
    /// Sorted unique ignore patterns.
    pub ignore: Vec<String>,
    /// Sorted unique include patterns.
    pub include: Vec<String>,
}

impl PolicyRuleScope {
    pub fn new(
        scope: DirPath,
        mut ignore: Vec<String>,
        mut include: Vec<String>,
    ) -> PolicyRuleScope {
        ignore.sort();
        ignore.dedup();
        include.sort();
        include.dedup();
        PolicyRuleScope {
            scope,
            ignore,
            include,
        }
    }
}

/// The policy inputs that decide what an owner's review covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicy {
    pub owner: DocumentId,
    /// Global first, repository second, then `.gitignore` paths by depth and path.
    pub git_scopes: Vec<GitRuleScope>,
    /// From the root to the nearest ancestor scope.
    pub memoria_scopes: Vec<PolicyRuleScope>,
}

impl EffectivePolicy {
    pub fn new(
        owner: DocumentId,
        mut git_scopes: Vec<GitRuleScope>,
        mut memoria_scopes: Vec<PolicyRuleScope>,
    ) -> EffectivePolicy {
        git_scopes.sort_by_key(|scope| git_scope_rank(&scope.identity));
        memoria_scopes.sort_by_key(|scope| (scope.scope.depth(), scope.scope.clone()));
        EffectivePolicy {
            owner,
            git_scopes,
            memoria_scopes,
        }
    }
}

fn git_scope_rank(identity: &str) -> (u8, usize, String) {
    match identity {
        "global" => (0, 0, String::new()),
        "repository" => (1, 0, String::new()),
        path => (2, path.matches('/').count(), path.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_scopes_are_ordered_global_repository_then_paths() {
        let policy = EffectivePolicy::new(
            DocumentId::parse("README.md").unwrap(),
            vec![
                GitRuleScope {
                    identity: "src/.gitignore".into(),
                    patterns: vec![],
                },
                GitRuleScope {
                    identity: ".gitignore".into(),
                    patterns: vec![],
                },
                GitRuleScope {
                    identity: "repository".into(),
                    patterns: vec![],
                },
                GitRuleScope {
                    identity: "global".into(),
                    patterns: vec![],
                },
            ],
            vec![],
        );
        let ids: Vec<&str> = policy
            .git_scopes
            .iter()
            .map(|s| s.identity.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["global", "repository", ".gitignore", "src/.gitignore"]
        );
    }
}
