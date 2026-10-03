//! Default human output carries results, failures, and the next action.
//!
//! Advisory hints and progress notes print only on request. JSON carries
//! every diagnostic either way. Each case checks stdout and stderr together.

mod common;

use common::*;
use memoria_infrastructure::json::Json;

/// Root links `a/` (a normal link with no import: a `missing_import_hint`)
/// and an ordinary Markdown file in `notes/` (a `handoff_not_applied` hint).
/// `a/` has its own README with a source file.
fn hinted() -> Project {
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\nSee [A](a/README.md) and [the notes](notes/plan.md).\n",
    );
    project.write("notes/plan.md", "# Plan\n\nOrdinary notes.\n");
    project.write("a/README.md", "# A\n\nA owns its source.\n");
    project.write("a/lib.rs", "fn a() {}\n");
    project.commit_all("fixture");
    project.baseline();
    project.commit_all("baseline");
    project
}

fn hint_codes(value: &Json) -> Vec<String> {
    match get(value, &["diagnostics"]) {
        Json::Array(items) => items
            .iter()
            .filter(|d| get_str(d, &["severity"]) == "hint")
            .map(|d| get_str(d, &["code"]).to_string())
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn routine_commands_print_no_hints_by_default() {
    let project = hinted();
    for args in [
        vec!["review"],
        vec!["status"],
        vec!["status", "--summary"],
        vec!["check"],
        vec!["graph"],
        vec!["guidance"],
        vec!["render", "--dry-run"],
    ] {
        let output = project.run(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(stderr(&output), "", "{args:?}");
        assert!(!stdout(&output).is_empty(), "{args:?}");
        // The same hints remain in the machine output.
        let (_, value) = project.json(&args);
        let hints = hint_codes(&value);
        assert!(
            hints.contains(&"missing_import_hint".to_string()),
            "{args:?}"
        );
        assert!(
            hints.contains(&"handoff_not_applied".to_string()),
            "{args:?}"
        );
    }
}

#[test]
fn verbose_restores_hints_and_never_changes_json() {
    let project = hinted();
    let output = project.run(&["review", "--verbose"]);
    let err = stderr(&output);
    assert!(err.contains("hint [missing_import_hint]"), "{err}");
    assert!(err.contains("hint [handoff_not_applied]"), "{err}");
    let quiet = project.run(&["check", "--format", "json"]);
    let verbose = project.run(&["check", "--format", "json", "--verbose"]);
    assert_eq!(quiet.stdout, verbose.stdout);
}

#[test]
fn lint_reports_hints_because_that_is_its_purpose() {
    let project = hinted();
    let output = project.run(&["lint"]);
    assert_eq!(output.status.code(), Some(0));
    let err = stderr(&output);
    assert!(err.contains("hint [missing_import_hint]"), "{err}");
    assert!(err.contains("hint [handoff_not_applied]"), "{err}");
}

#[test]
fn document_reviews_and_explanations_print_no_link_advice() {
    let project = hinted();
    // A current explanation: the root's own links carry optional advice.
    let current = project.run(&["explain", "README.md"]);
    assert_eq!(current.status.code(), Some(0));
    assert_eq!(stderr(&current), "");
    assert!(stdout(&current).contains("no review needed"));
    // A pending review for a reason unrelated to the links.
    project.run(&[
        "invalidate",
        "doc:README.md",
        "--reason",
        "Review the root after a decision.",
    ]);
    let pending = project.run(&["review", "README.md"]);
    assert_eq!(pending.status.code(), Some(0), "{}", stderr(&pending));
    assert_eq!(stderr(&pending), "");
    assert!(stdout(&pending).starts_with("Review README.md"));
    // The advice stays available on request and in JSON.
    for args in [
        vec!["explain", "README.md", "--verbose"],
        vec!["review", "README.md", "--verbose"],
        vec!["lint"],
    ] {
        let err = stderr(&project.run(&args));
        assert!(
            err.contains("hint [missing_import_hint]"),
            "{args:?}: {err}"
        );
        assert!(
            err.contains("hint [handoff_not_applied]"),
            "{args:?}: {err}"
        );
    }
    for args in [vec!["explain", "README.md"], vec!["review", "README.md"]] {
        let (_, value) = project.json(&args);
        let hints = hint_codes(&value);
        assert!(
            hints.contains(&"missing_import_hint".to_string()),
            "{args:?}"
        );
        assert!(
            hints.contains(&"handoff_not_applied".to_string()),
            "{args:?}"
        );
    }
}

/// `c/README.md` imports `b/README.md#summary`. Both are reviewed and current.
fn provider_consumer() -> Project {
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\nSee [B](b/README.md) and [C](c/README.md).\n",
    );
    project.write(
        "b/README.md",
        "# B\n\n<!-- memoria:export id=\"summary\" -->\nB summary one.\n<!-- /memoria:export -->\n",
    );
    project.write(
        "c/README.md",
        "# C\n\n<!-- memoria:import src=\"../b/README.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    project.commit_all("fixture");
    project.baseline();
    project.commit_all("baseline");
    project
}

/// The render-first next step of `explain c/README.md`.
const RENDER_NEXT: &str = "Next: memoria render c/README.md. Then run memoria explain c/README.md again, because the render can leave it current.";

#[test]
fn explain_names_render_before_review_for_an_outdated_import() {
    let project = provider_consumer();
    let text = project.read_string("b/README.md").replace("one", "two");
    project.write("b/README.md", text);

    // The consumer waits for its provider first.
    let waiting = project.run(&["explain", "c/README.md"]);
    assert_eq!(waiting.status.code(), Some(0));
    assert!(
        stdout(&waiting).contains("Next: review b/README.md first"),
        "{}",
        stdout(&waiting)
    );
    let (code, refused) = project.json(&["review", "c/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["dependencies_pending"]);

    // After the provider's acknowledgement, the import must be rendered.
    project.ack_ok("b/README.md");
    let outdated = project.run(&["explain", "c/README.md"]);
    assert_eq!(outdated.status.code(), Some(0));
    let view = stdout(&outdated);
    assert!(view.contains(RENDER_NEXT), "{view}");
    assert!(!view.contains("memoria review"), "{view}");
    assert!(stderr(&outdated).contains("warning [imports_outdated]"));
    let (code, refused) = project.json(&["review", "c/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["imports_outdated"]);
    let (_, plan) = project.json(&["review"]);
    assert_eq!(get_str(&plan, &["data", "next_action", "kind"]), "render");

    // After render, the review proceeds and the explanation says so.
    assert_eq!(
        project.run(&["render", "c/README.md"]).status.code(),
        Some(0)
    );
    let ready = stdout(&project.run(&["explain", "c/README.md"]));
    assert!(
        ready.contains("Next: memoria review c/README.md\n"),
        "{ready}"
    );
    project.ack_ok("c/README.md");
    let (code, _) = project.json(&["check"]);
    assert_eq!(code, 0);
}

#[test]
fn mutations_print_progress_only_on_request() {
    let project = hinted();
    let quiet = project.run(&[
        "invalidate",
        "doc:a/README.md",
        "--reason",
        "Check the quiet mutation output.",
    ]);
    assert_eq!(quiet.status.code(), Some(0));
    assert_eq!(stderr(&quiet), "");
    assert!(stdout(&quiet).contains("pending  a/README.md"));
    let (packet, _) = project.review_packet("a/README.md");
    let verbose = project.run(&[
        "ack",
        "a/README.md",
        "--packet",
        packet.to_str().unwrap(),
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        "The folder text still matches its source.",
        "--verbose",
    ]);
    assert_eq!(verbose.status.code(), Some(0));
    assert!(stderr(&verbose).contains("memoria: ack: a/README.md"));
    assert!(stdout(&verbose).starts_with("Recorded a/README.md"));
}

#[test]
fn failures_stay_visible_without_verbose() {
    let project = hinted();
    project.append("a/lib.rs", "// change\n");
    let output = project.run(&["check"]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("error [review_pending]"), "{err}");
    assert!(!err.contains("hint ["), "{err}");
}

#[test]
fn the_review_view_shows_bounded_hunks_and_details_shows_them_whole() {
    let project = hinted();
    let lines: String = (0..120).map(|i| format!("// line {i}\n")).collect();
    project.append("a/lib.rs", &lines);
    let view = stdout(&project.run(&["review", "a/README.md"]));
    assert!(view.contains("@@"), "{view}");
    assert!(view.contains("+// line 0"), "{view}");
    assert!(!view.contains("+// line 119"), "{view}");
    assert!(view.contains("more hunk lines"), "{view}");
    assert!(view.contains("Complete hunks: memoria review a/README.md --details"));
    let details = stdout(&project.run(&["review", "a/README.md", "--details"]));
    assert!(details.contains("+// line 119"), "{details}");
    assert!(!details.contains("more hunk lines"), "{details}");
}

#[test]
fn explain_never_suggests_a_review_for_a_current_document() {
    let project = hinted();
    let view = stdout(&project.run(&["explain", "a/README.md"]));
    assert!(
        view.starts_with("Freshness: a/README.md — current\n"),
        "{view}"
    );
    assert!(view.contains("no review needed"), "{view}");
    assert!(!view.contains("memoria review"), "{view}");
    project.append("a/lib.rs", "// change\n");
    let view = stdout(&project.run(&["explain", "a/README.md"]));
    assert!(view.contains("+// change"), "{view}");
    assert!(view.contains("Next: memoria review a/README.md"), "{view}");
}

#[test]
fn a_guidance_change_is_one_assessment_item() {
    let project = hinted();
    let empty = project.run(&["guidance", "--changed"]);
    assert_eq!(empty.status.code(), Some(0));
    assert_eq!(
        stdout(&empty),
        "No reviewed document has changed guidance.\n"
    );
    let (_, plan) = project.json(&["review"]);
    assert_eq!(get(&plan, &["data", "guidance_assessment"]), &Json::Null);

    let config = project
        .read_string("memoria.toml")
        .replace("guidance = []", "guidance = [\"Write for operators.\"]");
    project.write("memoria.toml", config);
    let before = project.state();

    let (code, plan) = project.json(&["review"]);
    assert_eq!(code, 0);
    assert_eq!(
        get_u64(&plan, &["data", "guidance_assessment", "changed_documents"]),
        2
    );
    let human = project.run(&["review"]);
    assert!(stdout(&human).contains(
        "Guidance changed since review for 2 documents. Assess it: memoria guidance --changed"
    ));

    let (code, assessment) = project.json(&["guidance", "--changed"]);
    assert_eq!(code, 0);
    assert_eq!(
        get_str(&assessment, &["data", "kind"]),
        "guidance_assessment"
    );
    let Json::Array(groups) = get(&assessment, &["data", "groups"]) else {
        panic!()
    };
    assert_eq!(groups.len(), 1);
    assert_eq!(
        strings(get(&groups[0], &["documents"])),
        vec!["README.md", "a/README.md"]
    );
    let view = stdout(&project.run(&["guidance", "--changed"]));
    assert!(view.contains("memoria invalidate doc:<DOCUMENT>"), "{view}");
    assert!(view.contains("Do not acknowledge them"), "{view}");

    // check passes, keeps the hint in JSON, and prints none by default.
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 0);
    assert!(hint_codes(&check).contains(&"guidance_changed".to_string()));
    assert_eq!(stderr(&project.run(&["check"])), "");

    // The assessment wrote nothing and no document became pending.
    assert_eq!(project.state(), before);
    assert_eq!(
        get_str(&project.doc_status("a/README.md"), &["status"]),
        "current"
    );

    let usage = project.run(&["guidance", "README.md", "--changed"]);
    assert_eq!(usage.status.code(), Some(2));
}

#[test]
fn a_render_that_restores_a_reviewed_import_leaves_the_consumer_current() {
    let project = provider_consumer();
    let original = project.read_string("b/README.md");
    // The provider changes its export, is acknowledged, and the consumer renders.
    project.write("b/README.md", original.replace("one", "two"));
    project.ack_ok("b/README.md");
    assert_eq!(
        project.run(&["render", "c/README.md"]).status.code(),
        Some(0)
    );
    // The provider restores its earlier export and is acknowledged again.
    project.write("b/README.md", original.clone());
    project.ack_ok("b/README.md");

    // The consumer's import is outdated: render first, no predicted review.
    let before = project.run(&["explain", "c/README.md"]);
    assert_eq!(before.status.code(), Some(0));
    let view = stdout(&before);
    assert!(view.contains(RENDER_NEXT), "{view}");
    assert!(!view.contains("memoria review"), "{view}");
    assert!(stderr(&before).contains("warning [imports_outdated]"));
    let (_, plan) = project.json(&["review"]);
    assert_eq!(get_str(&plan, &["data", "next_action", "kind"]), "render");

    // The render restores the reviewed bytes, so nothing remains to review.
    assert_eq!(
        project.run(&["render", "c/README.md"]).status.code(),
        Some(0)
    );
    let after = project.run(&["explain", "c/README.md"]);
    assert_eq!(after.status.code(), Some(0));
    assert_eq!(stderr(&after), "");
    let view = stdout(&after);
    assert!(
        view.starts_with("Freshness: c/README.md — current\n"),
        "{view}"
    );
    assert!(view.contains("no review needed"), "{view}");
    let (code, plan) = project.json(&["review"]);
    assert_eq!(code, 0);
    assert_eq!(get(&plan, &["data", "next_action"]), &Json::Null);
    assert_eq!(get(&plan, &["data", "tasks"]), &Json::Array(vec![]));
    // The current document is still refused for review, and check passes.
    let (code, refused) = project.json(&["review", "c/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["review_not_pending"]);
    let (code, _) = project.json(&["check"]);
    assert_eq!(code, 0);
}
