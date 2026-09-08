//! Effective review policy for one owner.

use crate::path::{DirPath, DocumentId};

/// Ordered effective repository ignore rules from one logical source.
///
/// Only repository-relative `.gitignore` paths are identities. Host ignore
/// sources (`core.excludesFile`, the XDG fallback, `.git/info/exclude`) still
/// decide actual Git eligibility, but they never enter this policy: a
/// harmless host rule must not change any project's freshness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRuleScope {
    /// A root-relative `.gitignore` path.
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
    /// Repository `.gitignore` paths by depth, then by path.
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

fn git_scope_rank(identity: &str) -> (usize, String) {
    (identity.matches('/').count(), identity.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_rule_scopes_are_ordered_by_depth_then_path() {
        let scope = |identity: &str| GitRuleScope {
            identity: identity.into(),
            patterns: vec![],
        };
        let policy = EffectivePolicy::new(
            DocumentId::parse("README.md").unwrap(),
            vec![
                scope("src/retrieval/.gitignore"),
                scope("src/.gitignore"),
                scope(".gitignore"),
                scope("docs/.gitignore"),
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
            vec![
                ".gitignore",
                "docs/.gitignore",
                "src/.gitignore",
                "src/retrieval/.gitignore"
            ]
        );
    }
}
