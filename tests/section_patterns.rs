//! Section `files` patterns: includes expand over the document's effective
//! scope, `!` exclusions subtract, and the review reads only the pattern
//! matches that changed. Freshness, scope, and acknowledgement never depend
//! on a pattern.

mod common;

use common::*;
use memoria_infrastructure::json::Json;

const README: &str = "# App

The [service](service/README.md) has its own README.

<!-- memoria:section id=\"auth\" files=\"src/auth/** !src/auth/tests/** !src/auth/generated.rs\" -->
## Authentication

Login and passkeys.
<!-- /memoria:section -->

<!-- memoria:section id=\"entry\" files=\"src/main.rs\" -->
## Entry point

The program starts in main.
<!-- /memoria:section -->
";

const AUTH_TOKENS: &str = "src/auth/** !src/auth/tests/** !src/auth/generated.rs";

/// A root README with one pattern section and one literal section. The
/// project also has an ignored build folder, an opted-in page, and a
/// handed-off folder, so the pattern has things it must not match.
fn project() -> Project {
    let project = Project::empty_repo();
    project.write(
        "memoria.toml",
        "version = 3\nignore = [\"src/auth/build/**\"]\n",
    );
    project.write("README.md", README);
    project.write("service/README.md", "# Service\n\nThe service.\n");
    project.write("service/api.rs", "// api\n");
    project.write(
        "src/auth/notes.md",
        "# Notes\n\n<!-- memoria:export id=\"summary\" -->\nNotes.\n<!-- /memoria:export -->\n",
    );
    for (path, text) in [
        ("src/main.rs", "fn main() {}\n"),
        ("src/auth/login.rs", "// login\n"),
        ("src/auth/passkeys.rs", "// passkeys\n"),
        ("src/auth/generated.rs", "// generated\n"),
        ("src/auth/tests/login.rs", "// test\n"),
        ("src/auth/build/out.rs", "// build output\n"),
    ] {
        project.write(path, text);
    }
    project.canonical_loop();
    assert!(project.pending().is_empty());
    project.commit_all("baseline");
    project
}

fn review(project: &Project, document: &str) -> Json {
    let (code, value) = project.json(&["review", document]);
    assert_eq!(code, 0, "{value:?}");
    value
}

fn array<'a>(value: &'a Json, path: &[&str]) -> &'a [Json] {
    match get(value, path) {
        Json::Array(items) => items,
        other => panic!("{path:?} is not an array: {other:?}"),
    }
}

fn strings(value: &Json, path: &[&str]) -> Vec<String> {
    array(value, path)
        .iter()
        .map(|item| match item {
            Json::String(text) => text.clone(),
            other => panic!("not a string: {other:?}"),
        })
        .collect()
}

fn fallback_codes(project: &Project, document: &str) -> Vec<String> {
    let mut codes: Vec<String> = project
        .fallbacks(document)
        .into_iter()
        .map(|(code, _)| code)
        .collect();
    codes.dedup();
    codes
}

/// The sections that the change of `identity` names in its relationship.
fn related_sections(value: &Json, identity: &str) -> Vec<String> {
    let change = array(value, &["data", "changes"])
        .iter()
        .find(|change| get_str(change, &["identity"]) == identity)
        .unwrap_or_else(|| panic!("no change {identity}"));
    strings(change, &["relationship", "sections"])
}

/// The complete expansion of one section, from `status --explain`.
fn expansion(project: &Project, id: &str) -> Vec<String> {
    let (code, value) = project.json(&["status", "--explain", "README.md"]);
    assert_eq!(code, 0, "{value:?}");
    let sections = array(&value, &["data", "explanation", "document", "sections"]);
    let section = sections
        .iter()
        .find(|section| get_str(section, &["id"]) == id)
        .unwrap();
    strings(section, &["sources"])
}

#[test]
fn a_pattern_expands_over_the_effective_scope_only() {
    let project = project();
    // The ignored build file, the excluded tests, the excluded literal, the
    // opted-in page, and the handed-off folder are all absent.
    assert_eq!(
        expansion(&project, "auth"),
        ["src/auth/login.rs", "src/auth/passkeys.rs"]
    );
    assert_eq!(expansion(&project, "entry"), ["src/main.rs"]);
    // A source names the section and the token that maps it.
    let (_, value) = project.json(&["status", "--explain", "src/auth/passkeys.rs"]);
    let sections = array(&value, &["data", "explanation", "sections"]);
    assert_eq!(sections.len(), 1);
    assert_eq!(get_str(&sections[0], &["document"]), "README.md");
    assert_eq!(get_str(&sections[0], &["id"]), "auth");
    assert_eq!(strings(&sections[0], &["via"]), ["src/auth/**"]);
    let (_, value) = project.json(&["status", "--explain", "src/auth/tests/login.rs"]);
    assert!(array(&value, &["data", "explanation", "sections"]).is_empty());
    // The human view lists the tokens and every match.
    let output = project.run(&["status", "--explain", "README.md"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(&format!("  section  auth: {AUTH_TOKENS} -> 2 source(s)\n             src/auth/login.rs\n             src/auth/passkeys.rs\n")),
        "{text}"
    );
}

#[test]
fn a_focused_review_reads_literals_and_changed_matches_only() {
    let project = project();
    project.append("src/auth/login.rs", "// changed\n");
    project.append("src/main.rs", "// changed\n");
    let value = review(&project, "README.md");
    assert_eq!(
        get_str(&value, &["data", "review", "mode"]),
        "focused_candidate"
    );
    let sections = array(&value, &["data", "review", "sections"]);
    assert_eq!(sections.len(), 2);
    let auth = &sections[0];
    assert_eq!(get_str(auth, &["id"]), "auth");
    assert_eq!(
        strings(auth, &["files"]),
        AUTH_TOKENS.split(' ').collect::<Vec<_>>()
    );
    assert_eq!(get_u64(auth, &["matched"]), 2);
    // passkeys.rs matches but did not change, so it is not a read.
    assert_eq!(strings(auth, &["sources"]), ["src/auth/login.rs"]);
    let entry = &sections[1];
    assert_eq!(strings(entry, &["files"]), ["src/main.rs"]);
    assert_eq!(strings(entry, &["sources"]), ["src/main.rs"]);
    let reads: Vec<&str> = array(&value, &["data", "inputs"])
        .iter()
        .map(|input| get_str(input, &["path"]))
        .collect();
    assert_eq!(reads, ["README.md", "src/auth/login.rs", "src/main.rs"]);
    assert_eq!(get_u64(&value, &["data", "counts", "suggested_sources"]), 2);
    // The human view shows the tokens, the count, and the read.
    let output = project.run(&["review", "README.md"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(&format!(
            "Suggested section auth \"Authentication\" lines 6-8: {AUTH_TOKENS} (2 matches; read src/auth/login.rs)\n"
        )),
        "{text}"
    );
    assert!(
        text.contains("Suggested section entry \"Entry point\" lines 12-14: src/main.rs\n"),
        "{text}"
    );
    // The saved artifact round-trips through the strict decoder.
    project.ack_ok("README.md");
}

#[test]
fn added_deleted_and_renamed_matches_keep_the_mapping_valid() {
    let project = project();
    // An added match: a full baseline for the path set only. The change
    // already names the section that maps it.
    project.write("src/auth/totp.rs", "// totp\n");
    assert_eq!(fallback_codes(&project, "README.md"), ["path_set_changed"]);
    let value = review(&project, "README.md");
    assert_eq!(related_sections(&value, "src/auth/totp.rs"), ["auth"]);
    project.ack_ok("README.md");
    project.commit_all("totp");
    assert_eq!(
        expansion(&project, "auth"),
        [
            "src/auth/login.rs",
            "src/auth/passkeys.rs",
            "src/auth/totp.rs"
        ]
    );

    // A deleted match.
    project.remove("src/auth/passkeys.rs");
    assert_eq!(fallback_codes(&project, "README.md"), ["path_set_changed"]);
    project.ack_ok("README.md");
    project.commit_all("drop passkeys");

    // A renamed match is a removal and an addition; the mapping follows.
    project.git(&["mv", "src/auth/login.rs", "src/auth/sign_in.rs"]);
    assert_eq!(fallback_codes(&project, "README.md"), ["path_set_changed"]);
    project.ack_ok("README.md");
    project.commit_all("rename");
    assert_eq!(
        expansion(&project, "auth"),
        ["src/auth/sign_in.rs", "src/auth/totp.rs"]
    );

    // The next edit of the renamed file is focused again.
    project.append("src/auth/sign_in.rs", "// edit\n");
    let value = review(&project, "README.md");
    assert_eq!(
        get_str(&value, &["data", "review", "mode"]),
        "focused_candidate"
    );
    let sections = array(&value, &["data", "review", "sections"]);
    assert_eq!(strings(&sections[0], &["sources"]), ["src/auth/sign_in.rs"]);

    // A literal mapping breaks on the same rename.
    project.ack_ok("README.md");
    project.git(&["mv", "src/main.rs", "src/start.rs"]);
    let codes = fallback_codes(&project, "README.md");
    assert!(codes.contains(&"mapping_invalid".to_string()), "{codes:?}");
}

#[test]
fn excluded_files_are_unmapped_changes() {
    let project = project();
    for path in ["src/auth/tests/login.rs", "src/auth/generated.rs"] {
        project.append(path, "// edit\n");
        let codes = fallback_codes(&project, "README.md");
        assert_eq!(codes, ["unmapped_change"], "{path}");
        // `src/auth/notes.md` also covers the folder, so review both.
        project.canonical_loop();
    }
    // An ignored file is not an input at all.
    assert!(project.pending().is_empty());
    project.append("src/auth/build/out.rs", "// rebuilt\n");
    let pending = project.pending();
    let (_, plan) = project.json(&["review"]);
    assert!(pending.is_empty(), "{pending:?} {plan:?}");
}

#[test]
fn an_equivalent_literal_list_is_the_same_association() {
    let project = project();
    // Review twice without a change: one token.
    project.append("src/auth/login.rs", "// edit\n");
    let first = review(&project, "README.md");
    let second = review(&project, "README.md");
    assert_eq!(
        get_str(&first, &["data", "token"]),
        get_str(&second, &["data", "token"])
    );
    project.ack_ok("README.md");
    project.commit_all("edit");

    // The same files as a literal list: the text changed, the association
    // did not.
    project.write(
        "README.md",
        README.replace(AUTH_TOKENS, "src/auth/passkeys.rs src/auth/login.rs"),
    );
    let codes = fallback_codes(&project, "README.md");
    assert!(!codes.contains(&"mapping_changed".to_string()), "{codes:?}");
    project.ack_ok("README.md");
    project.commit_all("literal list");

    // A pattern that also matches generated.rs is a changed association.
    project.write("README.md", README.replace(AUTH_TOKENS, "src/auth/*.rs"));
    let codes = fallback_codes(&project, "README.md");
    assert!(codes.contains(&"mapping_changed".to_string()), "{codes:?}");
}

#[test]
fn a_token_that_matches_nothing_is_a_lint_hint_only() {
    let project = project();
    project.write(
        "README.md",
        README.replace(
            AUTH_TOKENS,
            &format!("{AUTH_TOKENS} docs/** !src/auth/old.rs service/**"),
        ),
    );
    let hints = project.lint_diagnostics("section_pattern_empty");
    let mut tokens: Vec<&str> = hints
        .iter()
        .map(|hint| get_str(hint, &["details", "token"]))
        .collect();
    tokens.sort();
    assert_eq!(tokens, ["!src/auth/old.rs", "docs/**", "service/**"]);
    assert_eq!(get_str(&hints[0], &["severity"]), "hint");
    assert_eq!(get_u64(&hints[0], &["line"]), 5);
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);
    // Routine commands do not repeat it, and the advice stays valid.
    for args in [vec!["status"], vec!["check"], vec!["review", "README.md"]] {
        let (_, value) = project.json(&args);
        assert!(
            !diagnostic_codes(&value).contains(&"section_pattern_empty".to_string()),
            "{args:?}"
        );
    }
    assert!(
        project
            .lint_diagnostics("section_mapping_invalid")
            .is_empty()
    );
    let (_, value) = project.json(&["status", "--explain", "README.md"]);
    let auth = &array(&value, &["data", "explanation", "document", "sections"])[0];
    assert_eq!(
        strings(auth, &["unmatched"]),
        ["docs/**", "service/**", "!src/auth/old.rs"]
    );
}

#[test]
fn malformed_and_exclusion_only_tokens_withdraw_all_advice() {
    for (tokens, needle) in [
        (
            "!src/auth/**",
            "needs at least one path or pattern to include",
        ),
        ("src/auth/[ab.rs", "unterminated character class"),
        ("src/{auth,lib}/**", "not permitted"),
        ("src/** !!src/x.rs", "one `!`"),
        ("../src/**", "`.` or `..` component"),
        // A literal file named with a leading `!` cannot be mapped; the
        // token is an exclusion.
        ("!keep.rs", "needs at least one path or pattern to include"),
    ] {
        let project = project();
        project.write("!keep.rs", "// a leading exclamation mark\n");
        project.write("README.md", README.replace(AUTH_TOKENS, tokens));
        let warnings = project.lint_diagnostics("section_mapping_invalid");
        assert_eq!(warnings.len(), 1, "{tokens}");
        let message = get_str(&warnings[0], &["message"]);
        assert!(message.contains(needle), "{tokens}: {message}");
        let (_, value) = project.json(&["status", "--explain", "README.md"]);
        assert_eq!(
            get_str(
                &value,
                &["data", "explanation", "document", "section_state"]
            ),
            "invalid",
            "{tokens}"
        );
        assert!(
            fallback_codes(&project, "README.md").contains(&"mapping_invalid".to_string()),
            "{tokens}"
        );
    }
}

#[test]
fn a_pattern_can_name_a_file_that_starts_with_an_exclamation_mark() {
    let project = project();
    project.write("!keep.rs", "// a leading exclamation mark\n");
    project.write("README.md", README.replace(AUTH_TOKENS, "?keep.rs"));
    assert!(
        project
            .lint_diagnostics("section_mapping_invalid")
            .is_empty()
    );
    assert_eq!(expansion(&project, "auth"), ["!keep.rs"]);
}

#[test]
fn a_full_baseline_says_what_else_the_review_must_read() {
    let project = project();
    // An excluded file is an unmapped change, so the review is a full
    // baseline over five sources: the change and four unchanged sources.
    project.append("src/auth/generated.rs", "// edit\n");
    let text = String::from_utf8(project.run(&["review", "README.md"]).stdout).unwrap();
    assert!(
        text.contains("  Mode: full baseline. Review the whole document against its complete current scope, not only the changes.\n"),
        "{text}"
    );
    assert!(
        text.contains("  Read first:\n    README.md (whole_document)\n    src/auth/generated.rs (changed_source)\n  Then read the rest of the scope: 4 unchanged sources. List the sources: memoria status --explain README.md\n"),
        "{text}"
    );
    assert!(
        text.contains("  1. Read the whole document and the listed inputs, then the rest of the scope; edit README.md if it is wrong.\n"),
        "{text}"
    );
    // The listing command names every source in the scope.
    let (_, value) = project.json(&["status", "--explain", "README.md"]);
    assert_eq!(
        strings(&value, &["data", "explanation", "document", "scope"]),
        [
            "src/auth/generated.rs",
            "src/auth/login.rs",
            "src/auth/passkeys.rs",
            "src/auth/tests/login.rs",
            "src/main.rs"
        ]
    );
    // A focused review keeps its plain read list and the plain next step.
    project.canonical_loop();
    project.append("src/auth/login.rs", "// edit\n");
    let text = String::from_utf8(project.run(&["review", "README.md"]).stdout).unwrap();
    assert!(text.contains("  Read:\n"), "{text}");
    assert!(!text.contains("rest of the scope"), "{text}");
}
