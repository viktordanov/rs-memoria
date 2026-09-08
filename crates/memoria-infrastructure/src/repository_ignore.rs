//! Repository-only ignore matching for the deterministic policy inventory.
//!
//! This adapter answers with repository rule bytes alone. It never reads
//! `core.excludesFile`, the XDG fallback, or `.git/info/exclude`, so a
//! harmless host rule cannot hide a nested `.gitignore` and change a
//! project's policy hash. Actual file eligibility still follows the host
//! Git behavior through the Git CLI adapter.
//!
//! Matching is fixed to case-sensitive comparison. That makes the policy
//! inventory portable across hosts with different `core.ignoreCase`
//! settings. It does not normalize case-only paths or Unicode filenames.

use std::path::Path;

use gix_ignore::Search;

use memoria_application::ports::{AdapterError, IgnoreScope, RepositoryIgnoreMatcher};

/// The parser options Git itself uses. The optional precious-file syntax
/// (`$` prefixed rules) is disabled because Git does not implement it, so a
/// rule beginning with `$` stays an ordinary pattern.
const PARSE: gix_ignore::search::Ignore = gix_ignore::search::Ignore {
    support_precious: false,
};

pub struct GixRepositoryIgnore;

/// Build a search from repository rule buffers only.
///
/// `Search::from_git_dir` is deliberately not used: it loads host-local
/// exclusions. The search starts empty and receives exactly the supplied
/// buffers, each anchored at the directory that contains its `.gitignore`.
fn search(scopes: &[IgnoreScope]) -> Result<Search, AdapterError> {
    let mut search = Search::default();
    for scope in scopes {
        if scope.path.starts_with('/') || scope.path.contains("..") {
            return Err(AdapterError::new(
                "repository_ignore",
                Some(scope.path.clone()),
                "an ignore source must be a normalized project-relative path",
            ));
        }
        // Sources are project-relative, so the project root is the empty
        // path. The library then derives each source's base from its own
        // directory: `src/.gitignore` applies below `src/`, and the root
        // `.gitignore` applies everywhere.
        search.add_patterns_buffer(
            &scope.bytes,
            std::path::PathBuf::from(&scope.path),
            Some(Path::new("")),
            PARSE,
        );
    }
    Ok(search)
}

impl RepositoryIgnoreMatcher for GixRepositoryIgnore {
    fn ignored_directories(
        &self,
        scopes: &[IgnoreScope],
        directories: &[String],
    ) -> Result<Vec<String>, AdapterError> {
        if directories.is_empty() {
            return Ok(Vec::new());
        }
        let search = search(scopes)?;
        let mut out = Vec::new();
        for directory in directories {
            let relative = directory.trim_end_matches('/');
            if relative.is_empty() {
                continue;
            }
            // `Some(true)` states that the probed path is a directory, which
            // is what `dir/` rules require. Matching is fixed to
            // `Case::Sensitive` so two hosts agree.
            let matched = search.pattern_matching_relative_path(
                relative.into(),
                Some(true),
                gix_glob::pattern::Case::Sensitive,
            );
            if let Some(m) = matched
                && !m.pattern.is_negative()
            {
                out.push(relative.to_string());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str, rules: &str) -> IgnoreScope {
        IgnoreScope {
            path: path.to_string(),
            bytes: rules.as_bytes().to_vec(),
        }
    }

    fn ignored(scopes: &[IgnoreScope], dirs: &[&str]) -> Vec<String> {
        let names: Vec<String> = dirs.iter().map(|d| d.to_string()).collect();
        GixRepositoryIgnore
            .ignored_directories(scopes, &names)
            .unwrap()
    }

    #[test]
    fn root_rules_exclude_directories_by_name_and_by_path() {
        let scopes = vec![scope(".gitignore", "target\n/build\nnested/deep\n")];
        assert_eq!(
            ignored(&scopes, &["target", "src/target", "build", "src/build"]),
            vec!["target", "src/target", "build"]
        );
        assert_eq!(ignored(&scopes, &["nested/deep"]), vec!["nested/deep"]);
    }

    #[test]
    fn a_nested_source_applies_only_below_its_own_directory() {
        let scopes = vec![scope("src/.gitignore", "generated\n")];
        assert_eq!(
            ignored(&scopes, &["src/generated", "generated", "docs/generated"]),
            vec!["src/generated"]
        );
    }

    #[test]
    fn negation_restores_a_directory_that_an_earlier_rule_excluded() {
        let scopes = vec![scope(".gitignore", "build\n!build\n")];
        assert!(ignored(&scopes, &["build"]).is_empty());
        let ordered = vec![scope(".gitignore", "!build\nbuild\n")];
        assert_eq!(ignored(&ordered, &["build"]), vec!["build"]);
    }

    #[test]
    fn comments_blanks_escapes_and_double_star_keep_git_meanings() {
        let scopes = vec![scope(
            ".gitignore",
            "# a comment\n\n**/cache\nwith\\ space\n$literal\n",
        )];
        assert_eq!(
            ignored(&scopes, &["cache", "a/b/cache", "with space", "$literal"]),
            vec!["cache", "a/b/cache", "with space", "$literal"]
        );
    }

    #[test]
    fn matching_is_case_sensitive_regardless_of_the_host() {
        let scopes = vec![scope(".gitignore", "Target\n")];
        assert_eq!(ignored(&scopes, &["Target", "target"]), vec!["Target"]);
    }

    #[test]
    fn non_utf8_rule_bytes_do_not_stop_matching() {
        let mut bytes = b"good\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe, b'\n']);
        bytes.extend_from_slice(b"later\n");
        let scopes = vec![IgnoreScope {
            path: ".gitignore".into(),
            bytes,
        }];
        assert_eq!(ignored(&scopes, &["good", "later"]), vec!["good", "later"]);
    }

    #[test]
    fn an_empty_request_asks_nothing() {
        assert!(ignored(&[scope(".gitignore", "x\n")], &[]).is_empty());
    }
}
