//! The backbone rule end to end: every document covers its folder and below,
//! and hands a subfolder to a tracked document there only by a link or an
//! import. Plan §2.8 (the `auth/` before and after), §2.9 (the worked example
//! with changes A–G), the hints, H2 waiting, and fan-out.

mod common;
#[path = "common/counting.rs"]
mod counting;

use common::*;
use memoria_infrastructure::json::Json;

const LOGIN: &str = "auth/login.rs";

/// A root README with `text`, `app.rs`, and `auth/login.rs`, plus `extra`
/// files, all acknowledged and committed.
fn auth_project(text: &str, extra: &[(&str, &str)]) -> Project {
    let project = Project::empty_repo();
    project.write("README.md", text);
    project.write("app.rs", "fn main() {}\n");
    project.write(LOGIN, "fn login() {}\n");
    for (path, body) in extra {
        project.write(path, body);
    }
    project.commit_all("auth example");
    project.baseline();
    project.commit_all("acknowledged");
    project
}

const AUTH_README: (&str, &str) = (
    "auth/README.md",
    "# Auth\n\n<!-- memoria:export id=\"summary\" -->\nAuth summary.\n<!-- /memoria:export -->\n",
);

#[test]
fn nothing_about_auth_means_both_documents_cover_it() {
    let project = auth_project("# Root\n\nThe application.\n", &[AUTH_README]);
    assert_eq!(project.covered_by("app.rs"), "README.md");
    assert_eq!(project.covered_by(LOGIN), "README.md, auth/README.md");
    let absent = project.lint_diagnostics("handoff_absent");
    assert_eq!(absent.len(), 1);
    assert_eq!(get_str(&absent[0], &["path"]), "README.md");
    assert_eq!(get_str(&absent[0], &["severity"]), "hint");
    assert_eq!(get_str(&absent[0], &["details", "subtree"]), "auth");
    project.append(LOGIN, "// edit\n");
    assert_eq!(project.pending(), vec!["README.md", "auth/README.md"]);
    assert!(project.waiting().is_empty());
}

#[test]
fn a_link_hands_auth_off_and_adds_no_waiting() {
    let project = auth_project(
        "# Root\n\nSee [Authentication](auth/README.md).\n",
        &[AUTH_README],
    );
    assert_eq!(project.covered_by(LOGIN), "auth/README.md");
    assert!(project.lint_diagnostics("handoff_absent").is_empty());
    let (_, explain) = project.json(&["status", "--explain", LOGIN]);
    let Json::Array(handed) = get(&explain, &["data", "explanation", "handed_off"]) else {
        panic!()
    };
    assert_eq!(get_str(&handed[0], &["by"]), "README.md");
    assert_eq!(get_str(&handed[0], &["to"]), "auth/README.md");
    assert_eq!(get_str(&handed[0], &["via"]), "link");
    assert_eq!(get_u64(&handed[0], &["line"]), 3);
    project.append(LOGIN, "// edit\n");
    assert_eq!(project.pending(), vec!["auth/README.md"]);
    // H2: a link-only handoff creates no scheduling edge.
    assert!(project.waiting().is_empty());
}

#[test]
fn an_import_hands_auth_off_and_the_parent_waits() {
    let project = auth_project(
        "# Root\n\n<!-- memoria:import src=\"auth/README.md#summary\" -->\n<!-- /memoria:import -->\n",
        &[AUTH_README],
    );
    assert_eq!(project.covered_by(LOGIN), "auth/README.md");
    project.append(LOGIN, "// edit\n");
    assert_eq!(project.pending(), vec!["auth/README.md"]);
    // The import keeps its waiting edge, and the summary did not change, so
    // the root is not pending.
    assert_eq!(
        project.waiting(),
        vec![("README.md".to_string(), vec!["auth/README.md".to_string()])]
    );
    assert_eq!(project.status_label("README.md"), "current");
    // Changing the imported summary makes the root pending too.
    project.write(
        "auth/README.md",
        "# Auth\n\n<!-- memoria:export id=\"summary\" -->\nA new summary.\n<!-- /memoria:export -->\n",
    );
    assert_eq!(project.pending(), vec!["README.md", "auth/README.md"]);
}

#[test]
fn a_link_to_unmarked_markdown_is_not_a_handoff() {
    let project = auth_project(
        "# Root\n\nSee [Auth notes](auth/notes.md).\n",
        &[AUTH_README, ("auth/notes.md", "# Notes\n")],
    );
    assert_eq!(project.covered_by(LOGIN), "README.md, auth/README.md");
    assert_eq!(
        project.covered_by("auth/notes.md"),
        "README.md, auth/README.md"
    );
    let hints = project.lint_diagnostics("handoff_not_applied");
    assert_eq!(hints.len(), 1);
    assert_eq!(
        get_str(&hints[0], &["details", "reason"]),
        "untracked_markdown"
    );
    assert_eq!(get_str(&hints[0], &["details", "target"]), "auth/notes.md");
    assert_eq!(get_str(&hints[0], &["details", "via"]), "link");
    assert_eq!(get_u64(&hints[0], &["line"]), 3);
    project.append(LOGIN, "// edit\n");
    assert_eq!(project.pending(), vec!["README.md", "auth/README.md"]);
}

#[test]
fn a_link_to_an_opted_in_guide_hands_its_whole_folder_off() {
    let project = auth_project(
        "# Root\n\nSee [the guide](auth/guide.md).\n",
        &[(
            "auth/guide.md",
            "# Guide\n\n<!-- memoria:export id=\"summary\" -->\nThe guide.\n<!-- /memoria:export -->\n",
        )],
    );
    assert_eq!(project.covered_by(LOGIN), "auth/guide.md");
    project.append(LOGIN, "// edit\n");
    assert_eq!(project.pending(), vec!["auth/guide.md"]);
}

#[test]
fn removing_the_link_or_moving_the_target_ends_the_handoff() {
    let project = auth_project(
        "# Root\n\nSee [Authentication](auth/README.md).\n",
        &[AUTH_README],
    );
    // The root deletes the link: it covers auth/ again, with a full baseline.
    project.write("README.md", "# Root\n\nThe application.\n");
    assert_eq!(project.pending(), vec!["README.md"]);
    assert!(
        project
            .fallbacks("README.md")
            .contains(&("handoff_changed".to_string(), "auth".to_string()))
    );
    assert_eq!(project.covered_by(LOGIN), "README.md, auth/README.md");

    // auth/README.md moves to security/README.md: the root link is broken,
    // so the root is pending without an edit and the hint names the cause.
    let project = auth_project(
        "# Root\n\nSee [Authentication](auth/README.md).\n",
        &[AUTH_README],
    );
    project.write("security/README.md", AUTH_README.1);
    project.remove("auth/README.md");
    let codes = project.cause_codes("README.md");
    assert_eq!(codes, vec!["input_changed"]);
    assert!(
        project
            .fallbacks("README.md")
            .contains(&("handoff_changed".to_string(), "auth".to_string()))
    );
    let hints = project.lint_diagnostics("handoff_not_applied");
    assert_eq!(hints.len(), 1);
    assert_eq!(get_str(&hints[0], &["details", "reason"]), "missing");
    assert_eq!(project.status_label("security/README.md"), "never_reviewed");
}

#[test]
fn the_worked_example_has_the_documented_scopes() {
    let project = Project::worked_example();
    assert_eq!(project.covered_by("app.rs"), "README.md");
    assert_eq!(
        project.covered_by("legacy/old.rs"),
        "README.md, legacy/README.md"
    );
    assert_eq!(project.covered_by("docs/notes.md"), "docs/guide.md");
    for source in ["auth/login.rs", "auth/scratch.md", "auth/session.rs"] {
        assert_eq!(project.covered_by(source), "auth/README.md, auth/flows.md");
    }
    let (_, graph) = project.json(&["graph"]);
    let Json::Array(handoffs) = get(&graph, &["data", "handoffs"]) else {
        panic!()
    };
    let described: Vec<(String, String, String)> = handoffs
        .iter()
        .map(|h| {
            (
                get_str(h, &["subtree"]).to_string(),
                get_str(h, &["target"]).to_string(),
                get_str(h, &["via"]).to_string(),
            )
        })
        .collect();
    assert_eq!(
        described,
        vec![
            ("auth".into(), "auth/README.md".into(), "link".into()),
            ("docs".into(), "docs/guide.md".into(), "both".into()),
        ]
    );
    let (_, summary) = project.json(&["status", "--summary"]);
    assert_eq!(get_u64(&summary, &["data", "documents"]), 5);
    assert_eq!(get_u64(&summary, &["data", "readmes"]), 3);
    assert_eq!(get_u64(&summary, &["data", "opted_in_documents"]), 2);
    assert_eq!(get_u64(&summary, &["data", "handoffs"]), 2);
    assert_eq!(get_u64(&summary, &["data", "overlapping_sources"]), 4);
    // Only the unhanded nested README gets a hint; same-folder documents never do.
    let absent = project.lint_diagnostics("handoff_absent");
    assert_eq!(absent.len(), 1);
    assert_eq!(
        get_str(&absent[0], &["details", "target"]),
        "legacy/README.md"
    );
}

#[test]
fn change_a_source_in_a_shared_folder() {
    let project = Project::worked_example();
    project.append("auth/login.rs", "// A\n");
    assert_eq!(project.pending(), vec!["auth/README.md", "auth/flows.md"]);
    assert!(project.waiting().is_empty());
    // The guide's section describes the change, so it is a focused candidate.
    assert!(project.fallbacks("auth/flows.md").is_empty());
    // The two acknowledgements are independent.
    project.ack_ok("auth/README.md");
    assert_eq!(project.pending(), vec!["auth/flows.md"]);
}

#[test]
fn change_b_an_unhanded_folder_reaches_both_documents() {
    let project = Project::worked_example();
    project.append("legacy/old.rs", "// B\n");
    assert_eq!(project.pending(), vec!["README.md", "legacy/README.md"]);
}

#[test]
fn change_c_an_import_makes_the_parent_wait() {
    let project = Project::worked_example();
    project.append("docs/notes.md", "C\n");
    assert_eq!(project.pending(), vec!["docs/guide.md"]);
    assert_eq!(
        project.waiting(),
        vec![("README.md".to_string(), vec!["docs/guide.md".to_string()])]
    );
}

#[test]
fn change_d_a_root_source() {
    let project = Project::worked_example();
    project.append("app.rs", "// D\n");
    assert_eq!(project.pending(), vec!["README.md"]);
}

#[test]
fn change_e_adding_a_link_hands_a_folder_off() {
    let project = Project::worked_example();
    let text = project.read_string("README.md").replace(
        "See [authentication]",
        "See [legacy](legacy/README.md) and [authentication]",
    );
    project.write("README.md", text);
    assert_eq!(project.pending(), vec!["README.md"]);
    assert_eq!(
        project.cause_codes("README.md"),
        vec!["input_changed", "document_changed"]
    );
    assert!(
        project
            .fallbacks("README.md")
            .contains(&("handoff_changed".to_string(), "legacy".to_string()))
    );
    assert_eq!(project.covered_by("legacy/old.rs"), "legacy/README.md");
}

#[test]
fn change_f_removing_a_link_returns_the_folder() {
    let project = Project::worked_example();
    let text = project
        .read_string("README.md")
        .replace("See [authentication](auth/README.md) and", "See");
    project.write("README.md", text);
    assert_eq!(project.pending(), vec!["README.md"]);
    let reasons = project.fallbacks("README.md");
    // One reason for the subtree, not one per file.
    assert_eq!(
        reasons
            .iter()
            .filter(|(code, _)| code == "handoff_changed")
            .collect::<Vec<_>>(),
        vec![&("handoff_changed".to_string(), "auth".to_string())]
    );
    let (_, manifest) = project.json(&["review", "README.md"]);
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    let entered: Vec<&str> = changes
        .iter()
        .filter(|c| get_str(c, &["relationship", "kind"]) == "handoff")
        .map(|c| get_str(c, &["identity"]))
        .collect();
    assert_eq!(
        entered,
        vec!["auth/login.rs", "auth/scratch.md", "auth/session.rs"]
    );
}

#[test]
fn change_g_a_guide_that_loses_its_markers_joins_the_scope() {
    let project = Project::worked_example();
    project.write("auth/flows.md", "# Flows\n\nNo markers now.\n");
    assert_eq!(project.pending(), vec!["auth/README.md"]);
    assert!(project.fallbacks("auth/README.md").contains(&(
        "document_classification_changed".to_string(),
        "auth/flows.md".to_string()
    )));
    // The retained record stays in the lock, byte-unchanged.
    assert!(project.state_text().contains("auth/flows.md"));
}

#[test]
fn a_target_that_stops_being_tracked_ends_the_handoff() {
    let project = auth_project(
        "# Root\n\nSee [the guide](auth/guide.md).\n",
        &[(
            "auth/guide.md",
            "# Guide\n\n<!-- memoria:export id=\"summary\" -->\nThe guide.\n<!-- /memoria:export -->\n",
        )],
    );
    // The guide loses its only marker: no edit to the root, but the root
    // covers auth/ again.
    project.write("auth/guide.md", "# Guide\n\nThe guide.\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    let reasons = project.fallbacks("README.md");
    assert!(
        reasons.contains(&("handoff_changed".to_string(), "auth".to_string())),
        "{reasons:?}"
    );
    assert!(
        project
            .lint_diagnostics("handoff_not_applied")
            .iter()
            .any(|h| get_str(h, &["details", "reason"]) == "untracked_markdown")
    );
    // Gaining the marker again restores the handoff and the recorded
    // baselines: both documents are current once more.
    project.write(
        "auth/guide.md",
        "# Guide\n\n<!-- memoria:export id=\"summary\" -->\nThe guide.\n<!-- /memoria:export -->\n",
    );
    assert!(project.pending().is_empty());
}

#[test]
fn a_missing_target_that_appears_becomes_a_handoff() {
    let project = auth_project("# Root\n\nSee [Authentication](auth/README.md).\n", &[]);
    assert_eq!(
        get_str(
            &project.lint_diagnostics("handoff_not_applied")[0],
            &["details", "reason"]
        ),
        "missing"
    );
    project.write("auth/README.md", AUTH_README.1);
    assert_eq!(project.covered_by(LOGIN), "auth/README.md");
    assert!(
        project
            .fallbacks("README.md")
            .contains(&("handoff_changed".to_string(), "auth".to_string()))
    );
    assert_eq!(project.status_label("auth/README.md"), "never_reviewed");
}

#[test]
fn every_not_applied_reason_is_explained() {
    let project = auth_project(
        concat!(
            "# Root\n\n",
            "[a](missing/README.md) [b](empty/) [c](ignored/notes.md) [d](auth/notes.md)\n",
            "[code](auth/login.rs) [web](https://example.com/x/README.md) [mail](mailto:a@b.c) [frag](#top)\n",
            "[self](README.md) [same](./)\n",
            "```\n[in code](other/README.md)\n```\n",
            "`[inline](other2/README.md)`\n",
        ),
        &[
            ("auth/notes.md", "# Notes\n"),
            ("empty/data.rs", "fn data() {}\n"),
            ("ignored/notes.md", "# Ignored\n"),
            (".gitignore", "ignored/\n"),
        ],
    );
    let hints = project.lint_diagnostics("handoff_not_applied");
    let mut reasons: Vec<(String, String)> = hints
        .iter()
        .map(|h| {
            (
                get_str(h, &["details", "target"]).to_string(),
                get_str(h, &["details", "reason"]).to_string(),
            )
        })
        .collect();
    reasons.sort();
    assert_eq!(
        reasons,
        vec![
            (
                "auth/notes.md".to_string(),
                "untracked_markdown".to_string()
            ),
            (
                "empty/README.md".to_string(),
                "no_document_in_directory".to_string()
            ),
            ("ignored/notes.md".to_string(), "not_selected".to_string()),
            ("missing/README.md".to_string(), "missing".to_string()),
        ]
    );
    // None of these references hands anything off.
    let (_, summary) = project.json(&["status", "--summary"]);
    assert_eq!(get_u64(&summary, &["data", "handoffs"]), 0);
}

#[test]
fn an_import_of_untracked_markdown_says_so() {
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\n<!-- memoria:import src=\"auth/notes.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    project.write("auth/notes.md", "# Notes\n");
    project.write("memoria.toml", "version = 3\n");
    project.commit_all("untracked provider");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let missing = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "import_missing_document")
        .unwrap();
    assert!(get_bool(missing, &["details", "untracked_markdown"]));
    assert!(diagnostic_codes(&lint).contains(&"handoff_not_applied".to_string()));
    // A non-Markdown provider is an invalid import.
    project.write(
        "README.md",
        "# Root\n\n<!-- memoria:import src=\"auth/data.rs#summary\" -->\n<!-- /memoria:import -->\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"import_invalid".to_string()));
}

#[test]
fn unhanded_nested_documents_multiply_reviews() {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n");
    for dir in ["a", "a/b", "a/b/c"] {
        project.write(&format!("{dir}/README.md"), format!("# {dir}\n"));
    }
    project.write("a/b/c/deep.rs", "fn deep() {}\n");
    project.commit_all("nested");
    project.baseline();
    project.append("a/b/c/deep.rs", "// edit\n");
    // k = 3 unhanded nested documents give k + 1 pending documents.
    assert_eq!(project.pending().len(), 4);
    // Linking each level hands the folder down to exactly one document.
    project.append("README.md", "\n[a](a/README.md)\n");
    project.append("a/README.md", "\n[b](b/README.md)\n");
    project.append("a/b/README.md", "\n[c](c/README.md)\n");
    assert_eq!(project.covered_by("a/b/c/deep.rs"), "a/b/c/README.md");
}

#[test]
fn a_fresh_artifact_is_needed_after_a_handoff_edit() {
    let project = Project::worked_example();
    project.append("app.rs", "// change\n");
    let (packet, token) = project.review_packet("README.md");
    // Adding a link changes the handoff set bound into the context.
    let text = project.read_string("README.md").replace(
        "See [authentication]",
        "See [legacy](legacy/README.md) and [authentication]",
    );
    project.write("README.md", text);
    let before = project.state();
    let (code, value) = project.ack_json("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    assert_eq!(project.state(), before);
}

/// Review one document and return `(codes with identities, (identity,
/// relationship kind) per changed file, scope files, mode, whole pass)`.
type ReviewFacts = (
    Vec<(String, String)>,
    Vec<(String, String)>,
    u64,
    String,
    bool,
);

fn review_facts(project: &Project, document: &str) -> ReviewFacts {
    let (code, value) = project.json(&["review", document]);
    assert_eq!(code, 0, "{value:?}");
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    let files = changes
        .iter()
        .filter(|c| get_str(c, &["kind"]) == "file")
        .map(|c| {
            (
                get_str(c, &["identity"]).to_string(),
                get_str(c, &["relationship", "kind"]).to_string(),
            )
        })
        .collect();
    (
        project.fallbacks(document),
        files,
        get_u64(&value, &["data", "scope", "files"]),
        get_str(&value, &["data", "review", "mode"]).to_string(),
        get_bool(&value, &["data", "review", "whole_document_pass"]),
    )
}

/// A root README with `text`, `auth/README.md`, and `auth/login.rs`, with
/// nothing acknowledged yet.
fn r1_project(text: &str) -> Project {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    project.write("README.md", text);
    project.write("auth/README.md", "# Auth\n");
    project.write("auth/login.rs", "fn login() {}\n");
    project
}

// R1 case A: the handoff to a never-reviewed target ends when the target is
// deleted; the root's text is unchanged.
#[test]
fn a_deleted_never_reviewed_target_ends_the_handoff() {
    let project = r1_project("# Root\n\n[Auth](auth/README.md)\n");
    // H2 permits acknowledging the parent while the child is never reviewed.
    project.ack_ok("README.md");
    assert_eq!(project.status_label("auth/README.md"), "never_reviewed");
    project.remove("auth/README.md");
    let (reasons, files, scope, mode, whole) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "auth".to_string())]
    );
    assert_eq!(
        files,
        vec![("auth/login.rs".to_string(), "handoff".to_string())]
    );
    assert_eq!(scope, 1);
    assert_eq!(project.covered_by("auth/login.rs"), "README.md");
    assert_eq!(mode, "full_baseline");
    assert!(whole);
}

// R1 case B: no handoff was ever declared; acknowledging the child must not
// invent one for the parent.
#[test]
fn a_child_acknowledgement_never_invents_a_parent_handoff() {
    let project = r1_project("# Root\n");
    project.ack_ok("README.md");
    project.ack_ok("auth/README.md");
    project.write("auth/new.rs", "fn new() {}\n");
    let expected_reasons = vec![("path_set_changed".to_string(), "auth/new.rs".to_string())];
    let expected_files = vec![("auth/new.rs".to_string(), "scope_source".to_string())];
    let before = review_facts(&project, "README.md");
    assert_eq!(before.0, expected_reasons);
    assert_eq!(before.1, expected_files);
    assert_eq!(before.2, 2);
    assert_eq!((before.3.as_str(), before.4), ("full_baseline", true));
    project.ack_ok("auth/README.md");
    let after = review_facts(&project, "README.md");
    assert_eq!(
        after, before,
        "the child acknowledgement changes nothing for the root"
    );
    assert_eq!(
        project.covered_by("auth/new.rs"),
        "README.md, auth/README.md"
    );
}

// Guard: a broken reference into a folder that the parent already covered is
// not a former handoff.
#[test]
fn a_stale_link_into_a_covered_folder_is_an_ordinary_addition() {
    let project = r1_project("# Root\n\n[Notes](auth/notes.md)\n");
    project.ack_ok("README.md");
    project.ack_ok("auth/README.md");
    project.write("auth/new.rs", "fn new() {}\n");
    let (reasons, files, _, _, _) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("path_set_changed".to_string(), "auth/new.rs".to_string())]
    );
    assert_eq!(
        files,
        vec![("auth/new.rs".to_string(), "scope_source".to_string())]
    );
}

// R1 case C: a broken reference into a folder that did not exist at the
// root's review is not a former handoff. The first source there is an
// ordinary addition.
#[test]
fn a_stale_link_into_a_new_folder_is_an_ordinary_addition() {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    project.write("README.md", "# Root\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.write("auth/new.rs", "fn new() {}\n");
    let (reasons, files, scope, mode, whole) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("path_set_changed".to_string(), "auth/new.rs".to_string())]
    );
    assert_eq!(
        files,
        vec![("auth/new.rs".to_string(), "scope_source".to_string())]
    );
    assert_eq!(scope, 1);
    assert_eq!(project.covered_by("auth/new.rs"), "README.md");
    assert_eq!(mode, "full_baseline");
    assert!(whole);
    // The same holds after a second review of the root: without proof of a
    // former handoff, nothing is claimed.
    project.ack_ok("README.md");
    project.write("auth/other.rs", "fn other() {}\n");
    let (reasons, files, _, _, _) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("path_set_changed".to_string(), "auth/other.rs".to_string())]
    );
    assert_eq!(
        files,
        vec![("auth/other.rs".to_string(), "scope_source".to_string())]
    );
}

// ---------------------------------------------------------------------------
// Recorded coverage evidence (plan §17.8 items 1-8). Every record below is
// written by this build's `ack`, so it carries the handed-off folders of the
// final revalidated snapshot.

/// The recorded coverage evidence of one document, from `state inspect`.
fn coverage_evidence(project: &Project, document: &str) -> Option<Vec<String>> {
    let (code, value) = project.json(&["state", "inspect"]);
    assert_eq!(code, 0, "{value:?}");
    match get(
        &value,
        &["data", "state", "reviews", document, "coverage_evidence"],
    ) {
        Json::Null => None,
        list => Some(strings(list)),
    }
}

/// Review the root and require the exact R1 result: the handoff of `auth/`
/// ended, and reading changed no state byte.
fn assert_auth_handoff_ended(project: &Project) {
    let before = project.state();
    let (reasons, files, scope, mode, whole) = review_facts(project, "README.md");
    assert_eq!(project.state(), before, "a review never writes state");
    assert!(
        reasons.contains(&("handoff_changed".to_string(), "auth".to_string())),
        "{reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|(code, _)| code == "path_set_changed" || code == "coverage_unrecorded"),
        "{reasons:?}"
    );
    assert_eq!(
        files,
        vec![("auth/login.rs".to_string(), "handoff".to_string())]
    );
    assert_eq!(scope, 1);
    assert_eq!(mode, "full_baseline");
    assert!(whole);
}

// §17.8 item 1: the reproduced R1 sequence at revision 2.
#[test]
fn a_handoff_that_ends_after_a_second_review_is_reported() {
    let project = r1_project("# Root\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.write("README.md", "# Root\n\nEdited.\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    assert_eq!(
        coverage_evidence(&project, "README.md"),
        Some(vec!["auth".to_string()])
    );
    project.remove("auth/README.md");
    let (reasons, ..) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "auth".to_string())]
    );
    assert_auth_handoff_ended(&project);
}

// §17.8 item 2: the same at revision 3.
#[test]
fn a_handoff_that_ends_after_a_third_review_is_reported() {
    let project = r1_project("# Root\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.write("README.md", "# Root\n\nOne.\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.write("README.md", "# Root\n\nTwo.\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.remove("auth/README.md");
    let (reasons, ..) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "auth".to_string())]
    );
    assert_auth_handoff_ended(&project);
}

// §17.8 item 3: an acknowledgement that covered an invalidation.
#[test]
fn a_handoff_that_ends_after_an_acknowledged_invalidation_is_reported() {
    let project = r1_project("# Root\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    let (code, value) = project.json(&[
        "invalidate",
        "doc:README.md",
        "--reason",
        "the root summary needs a semantic look",
    ]);
    assert_eq!(code, 0, "{value:?}");
    project.ack_ok("README.md");
    let (_, inspect) = project.json(&["state", "inspect"]);
    assert!(
        matches!(
            get(
                &inspect,
                &["data", "state", "reviews", "README.md", "acknowledged_invalidations"]
            ),
            Json::Array(ids) if !ids.is_empty()
        ),
        "the acknowledgement covered the invalidation"
    );
    project.remove("auth/README.md");
    assert_auth_handoff_ended(&project);
}

// §17.8 item 4: the previous text is unavailable (edited after the last
// acknowledgement, no commit, dirty worktree).
#[test]
fn a_handoff_that_ends_without_history_is_reported() {
    let project = r1_project("# Root\n\n[Auth](auth/README.md)\n");
    project.ack_ok("README.md");
    project.write(
        "README.md",
        "# Root\n\nChanged later.\n\n[Auth](auth/README.md)\n",
    );
    project.remove("auth/README.md");
    let before = project.state();
    let (code, value) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{value:?}");
    assert_eq!(project.state(), before);
    let reasons = project.fallbacks("README.md");
    assert!(
        reasons.contains(&("handoff_changed".to_string(), "auth".to_string())),
        "{reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|(code, _)| code == "path_set_changed" || code == "coverage_unrecorded"),
        "{reasons:?}"
    );
    let (_, files, scope, _, _) = review_facts(&project, "README.md");
    assert!(files.contains(&("auth/login.rs".to_string(), "handoff".to_string())));
    assert_eq!(scope, 1);
    // The root's own text change is reported too.
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    assert!(
        changes.iter().any(|c| get_str(c, &["kind"]) == "document"
            && get_str(c, &["relationship", "kind"]) == "own_text"),
        "{changes:?}"
    );
    assert!(
        project
            .cause_codes("README.md")
            .contains(&"document_changed".to_string()),
        "{:?}",
        project.cause_codes("README.md")
    );
}

// §17.8 item 5: the provider and consumer context changed between the
// acknowledgement and the deletion.
#[test]
fn a_handoff_that_ends_after_its_context_changed_is_reported() {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    project.write(
        "README.md",
        "# Root\n\n[Auth](auth/README.md)\n\n<!-- memoria:export id=\"e\" -->\nRoot summary.\n<!-- /memoria:export -->\n\n<!-- memoria:import src=\"lib/README.md#s\" -->\n<!-- /memoria:import -->\n",
    );
    project.write(
        "lib/README.md",
        "# Lib\n\n<!-- memoria:export id=\"s\" -->\nLib summary.\n<!-- /memoria:export -->\n",
    );
    project.write("lib/lib.rs", "fn lib() {}\n");
    project.write("auth/README.md", "# Auth\n");
    project.write("auth/login.rs", "fn login() {}\n");
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    project.ack_ok("lib/README.md");
    project.ack_ok("README.md");
    assert_eq!(
        coverage_evidence(&project, "README.md"),
        Some(vec!["auth".to_string(), "lib".to_string()])
    );
    // A new consumer of the root's export, and a changed provider export.
    project.write(
        "other/README.md",
        "# Other\n\n<!-- memoria:import src=\"../README.md#e\" -->\n<!-- /memoria:import -->\n",
    );
    project.write(
        "lib/README.md",
        "# Lib\n\n<!-- memoria:export id=\"s\" -->\nLib summary, revised.\n<!-- /memoria:export -->\n",
    );
    project.ack_ok("lib/README.md");
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    project.remove("auth/README.md");
    let before = project.state();
    let (reasons, files, _, mode, whole) = review_facts(&project, "README.md");
    assert_eq!(project.state(), before);
    assert!(
        reasons.contains(&("handoff_changed".to_string(), "auth".to_string())),
        "{reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|(code, _)| code == "coverage_unrecorded"),
        "{reasons:?}"
    );
    assert!(
        files.contains(&("auth/login.rs".to_string(), "handoff".to_string())),
        "{files:?}"
    );
    assert_eq!((mode.as_str(), whole), ("full_baseline", true));
}

// §17.8 item 6: twenty handed-off folders end together. Recorded evidence
// answers each one without a single proof recomputation.
#[test]
fn twenty_ended_handoffs_need_no_proof_recomputation() {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    let mut text = "# Root\n\n".to_string();
    for index in 0..20 {
        let folder = format!("d{index:02}");
        text.push_str(&format!("- [{folder}]({folder}/README.md)\n"));
        project.write(&format!("{folder}/README.md"), format!("# {folder}\n"));
        project.write(&format!("{folder}/src.rs"), format!("fn f{index}() {{}}\n"));
    }
    project.write("README.md", &text);
    project.ack_ok("README.md");
    project.write("README.md", format!("{text}\nSecond review.\n"));
    project.ack_ok("README.md");
    let expected: Vec<String> = (0..20).map(|i| format!("d{i:02}")).collect();
    assert_eq!(
        coverage_evidence(&project, "README.md"),
        Some(expected.clone())
    );
    for folder in &expected {
        project.remove(&format!("{folder}/README.md"));
    }
    let before = project.state();
    let (reasons, files, scope, _, _) = review_facts(&project, "README.md");
    assert_eq!(project.state(), before);
    let mut handoffs: Vec<String> = reasons
        .iter()
        .filter(|(code, _)| code == "handoff_changed")
        .map(|(_, identity)| identity.clone())
        .collect();
    handoffs.sort();
    handoffs.dedup();
    assert_eq!(handoffs, expected);
    assert_eq!(
        reasons.len(),
        20,
        "exactly one reason per ended handoff: {reasons:?}"
    );
    assert_eq!(files.len(), 20);
    assert!(files.iter().all(|(_, kind)| kind == "handoff"), "{files:?}");
    assert_eq!(scope, 20);
    let counts = counting::review_proof_counts(&project.root, project.home.path(), "README.md");
    eprintln!("proof counts, 20 recorded handoffs: {counts:?}");
    assert_eq!(counts, counting::ProofCounts::default(), "no proof ran");
    assert_eq!(project.state(), before);
}

// §17.8 item 7: nested handoffs `a/` and `a/b/`.
#[test]
fn nested_handoffs_report_the_shallowest_ended_one() {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    project.write(
        "README.md",
        "# Root\n\n[A](a/README.md) and [B](a/b/README.md)\n",
    );
    project.write("a/README.md", "# A\n");
    project.write("a/x.rs", "fn x() {}\n");
    project.write("a/b/README.md", "# B\n");
    project.write("a/b/y.rs", "fn y() {}\n");
    project.ack_ok("README.md");
    assert_eq!(
        coverage_evidence(&project, "README.md"),
        Some(vec!["a".to_string(), "a/b".to_string()])
    );
    // Only `a/` ends: its source enters, `a/b/` stays handed off.
    project.remove("a/README.md");
    let (reasons, files, scope, _, _) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "a".to_string())]
    );
    assert_eq!(files, vec![("a/x.rs".to_string(), "handoff".to_string())]);
    assert_eq!(scope, 1);
    assert_eq!(project.covered_by("a/b/y.rs"), "a/b/README.md");
    // Both end: a file in `a/b/` reports the shallowest ended handoff.
    project.remove("a/b/README.md");
    let (reasons, files, scope, _, _) = review_facts(&project, "README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "a".to_string())]
    );
    assert_eq!(
        files,
        vec![
            ("a/b/y.rs".to_string(), "handoff".to_string()),
            ("a/x.rs".to_string(), "handoff".to_string()),
        ]
    );
    assert_eq!(scope, 2);
}

// §17.8 item 8: the recorded evidence equals the handoff subtrees of the
// acknowledged artifact.
#[test]
fn recorded_evidence_equals_the_acknowledged_handoffs() {
    let project = Project::empty_repo();
    project.write("memoria.toml", "version = 3\n");
    project.write(
        "README.md",
        "# Root\n\n[Auth](auth/README.md), [Guide](docs/guide.md)\n\n<!-- memoria:import src=\"lib/README.md#s\" -->\n<!-- /memoria:import -->\n",
    );
    project.write("auth/README.md", "# Auth\n");
    project.write("auth/login.rs", "fn login() {}\n");
    project.write(
        "docs/guide.md",
        "# Guide\n\n<!-- memoria:export id=\"summary\" -->\nThe guide.\n<!-- /memoria:export -->\n",
    );
    project.write(
        "lib/README.md",
        "# Lib\n\n<!-- memoria:export id=\"s\" -->\nLib.\n<!-- /memoria:export -->\n",
    );
    project.write("src/main.rs", "fn main() {}\n");
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    project.ack_ok("lib/README.md");
    let (packet, token) = project.review_packet("README.md");
    let artifact = memoria_infrastructure::json::parse(
        &std::fs::read(&packet).unwrap(),
        memoria_infrastructure::json::Limits::STATE,
    )
    .unwrap();
    let Json::Array(handoffs) = get(&artifact, &["data", "scope", "handoffs"]) else {
        panic!()
    };
    let mut subtrees: Vec<String> = handoffs
        .iter()
        .map(|h| get_str(h, &["subtree"]).to_string())
        .collect();
    subtrees.sort();
    subtrees.dedup();
    assert_eq!(subtrees, ["auth", "docs", "lib"]);
    let output = project.ack("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(coverage_evidence(&project, "README.md"), Some(subtrees));
    // A document that hands nothing off records the empty set.
    assert_eq!(coverage_evidence(&project, "lib/README.md"), Some(vec![]));
}
