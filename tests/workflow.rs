//! Section 6.4 acceptance scenarios on the three-level fixture.

mod common;

use common::*;
use memoria_infrastructure::json::{Json, to_pretty};

#[test]
fn baseline_workflow_reaches_a_passing_check() {
    let project = Project::seed();
    let (code, init) = project.json(&["init"]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&init, &["data", "created"])),
        vec![".memoria/state.json"]
    );
    assert!(project.exists("memoria.toml"));

    // Render first: every import block is empty in the seed.
    let (code, render) = project.json(&["render"]);
    assert_eq!(code, 0);
    assert_eq!(get_u64(&render, &["data", "documents_changed"]), 3);

    // Dependency order: corpus, disconnected, execution, naive, retrieval, root.
    let plan = project.plan();
    let Json::Array(tasks) = get(&plan, &["data", "tasks"]) else {
        panic!()
    };
    let order: Vec<&str> = tasks.iter().map(|t| get_str(t, &["document"])).collect();
    assert_eq!(
        order,
        vec![
            "src/corpus/README.md",
            "src/disconnected/README.md",
            "src/execution/README.md",
            "src/retrieval/naive/README.md",
            "src/retrieval/README.md",
            "README.md"
        ]
    );
    assert_eq!(
        get_str(&plan, &["data", "next_ready"]),
        "src/corpus/README.md"
    );
    assert!(get_bool(&tasks[3], &["waiting"]));
    assert_eq!(
        strings(get(&tasks[3], &["waiting_on"])),
        vec!["src/execution/README.md"]
    );

    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 1);
    assert!(
        diagnostic_codes(&check)
            .iter()
            .any(|c| c == "review_pending")
    );

    let mut acknowledged = Vec::new();
    while let Some(next) = project.next_ready() {
        project.ack_ok(&next);
        acknowledged.push(next);
    }
    assert_eq!(acknowledged.len(), 6);
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 0, "{}", to_pretty(&check));
    assert!(get_bool(&check, &["data", "ok"]));
    // Warnings and hints remain visible but do not fail.
    let codes = diagnostic_codes(&check);
    assert!(codes.contains(&"navigation_disconnected".to_string()));
    assert!(codes.contains(&"missing_import_hint".to_string()));

    let (_, status) = project.json(&["status"]);
    assert_eq!(get_u64(&status, &["data", "readmes"]), 6);
    assert_eq!(get_u64(&status, &["data", "selected_files"]), 7);
    assert_eq!(get_u64(&status, &["data", "reviews", "current"]), 6);
    assert_eq!(get_u64(&status, &["data", "reviews", "waiting"]), 0);
    assert_eq!(
        strings(get(&status, &["data", "disconnected"])),
        vec!["src/disconnected/README.md"]
    );
    assert_eq!(get_u64(&status, &["data", "exclusions", "memoria-rule"]), 1);
}

#[test]
fn selected_source_change_stops_at_the_owner() {
    let project = Project::seed();
    project.baseline();
    project.append("src/retrieval/naive/search.rs", "// changed\n");
    assert_eq!(
        project.status_label("src/retrieval/naive/README.md"),
        "pending"
    );
    assert_eq!(
        project.cause_codes("src/retrieval/naive/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(project.status_label("src/retrieval/README.md"), "current");
    assert_eq!(
        project.waiting_on("src/retrieval/README.md"),
        vec!["src/retrieval/naive/README.md"]
    );
    assert_eq!(project.status_label("README.md"), "current");
    assert_eq!(
        project.waiting_on("README.md"),
        vec!["src/retrieval/README.md"]
    );
    assert_eq!(project.status_label("src/execution/README.md"), "current");
    assert!(project.waiting_on("src/execution/README.md").is_empty());
    assert_eq!(project.status_label("src/corpus/README.md"), "current");

    // Consumers cannot receive a packet while waiting.
    let (code, refused) = project.json(&["review", "src/retrieval/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["dependencies_pending"]);

    // Acknowledging naive without an export change clears the waiting state.
    project.ack_ok("src/retrieval/naive/README.md");
    assert!(project.waiting_on("src/retrieval/README.md").is_empty());
    assert!(project.waiting_on("README.md").is_empty());
    let plan = project.plan();
    assert!(matches!(get(&plan, &["data", "tasks"]), Json::Array(t) if t.is_empty()));
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn add_delete_and_rename_change_the_input_set() {
    let project = Project::seed();
    project.baseline();
    project.write("src/execution/extra.rs", "pub fn extra() {}\n");
    let status = project.doc_status("src/execution/README.md");
    let Json::Array(causes) = get(&status, &["causes"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&causes[0], &["changes"]) else {
        panic!()
    };
    assert_eq!(get_str(&changes[0], &["change"]), "added");
    assert_eq!(
        get_str(&changes[0], &["identity"]),
        "src/execution/extra.rs"
    );
    project.ack_ok("src/execution/README.md");

    // Rename across an ownership boundary changes both owners.
    std::fs::rename(
        project.root.join("src/execution/extra.rs"),
        project.root.join("src/corpus/extra.rs"),
    )
    .unwrap();
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["input_changed"]
    );
    let (_, status) = project.json(&["status"]);
    let Json::Array(docs) = get(&status, &["data", "documents"]) else {
        panic!()
    };
    let execution = docs
        .iter()
        .find(|d| get_str(d, &["document"]) == "src/execution/README.md")
        .unwrap();
    let Json::Array(causes) = get(execution, &["causes"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&causes[0], &["changes"]) else {
        panic!()
    };
    assert_eq!(get_str(&changes[0], &["change"]), "removed");
    project.ack_ok("src/execution/README.md");
    project.ack_ok("src/corpus/README.md");

    // Deletion of a tracked file.
    project.git(&["add", "src/corpus/extra.rs"]);
    project.remove("src/corpus/extra.rs");
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["input_changed"]
    );
    let (_, explain) = project.json(&["status", "--explain", "src/corpus/extra.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "deleted"
    );
}

#[test]
fn child_readme_boundary_recalculates_ownership() {
    let project = Project::seed();
    project.baseline();
    project.write("src/retrieval/fixtures/README.md", "# Fixtures\n\n<!-- memoria:export id=\"summary\" -->\nFixture files.\n<!-- /memoria:export -->\n");
    // The fixture file moves from retrieval to the new README; retrieval's input set changes.
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.status_label("src/retrieval/fixtures/README.md"),
        "never_reviewed"
    );
    let (_, explain) = project.json(&["status", "--explain", "src/retrieval/fixtures/sample.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "src/retrieval/fixtures/README.md"
    );
    assert_eq!(project.status_label("src/execution/README.md"), "current");
}

#[test]
fn ignored_generated_file_change_needs_no_review() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    project.append("src/retrieval/generated/table.rs", "// regenerated\n");
    assert_eq!(project.json(&["check"]).0, 0);
    assert_eq!(project.status_label("src/retrieval/README.md"), "current");
    assert_eq!(project.state(), before);
    let (_, explain) = project.json(&["status", "--explain", "src/retrieval/generated/table.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "excluded"
    );
    assert!(get_str(&explain, &["data", "explanation", "reason"]).contains("**/generated/**"));
}

#[test]
fn local_include_restores_a_memoria_excluded_fixture() {
    let project = Project::seed();
    project.baseline();
    let (_, explain) = project.json(&["status", "--explain", "src/retrieval/fixtures/sample.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected"
    );
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "src/retrieval/README.md"
    );
    let steps = strings(get(&explain, &["data", "explanation", "steps"]));
    assert_eq!(steps.len(), 2);
    assert!(steps[0].contains("memoria.toml: ignore"));
    assert!(steps[1].contains("src/retrieval/README.memoria.toml: include"));
    project.append("src/retrieval/fixtures/sample.txt", "more\n");
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );

    // A local include cannot restore a Git-excluded path.
    project.write(
        "src/retrieval/README.memoria.toml",
        "include = [\n    \"fixtures/**\",\n    \"../../ignored-output/**\",\n]\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"configuration_invalid".to_string()));
    project.write(
        "src/retrieval/README.memoria.toml",
        "include = [\n    \"fixtures/**\",\n]\n",
    );
    project.write("ignored-output/README.memoria.toml", "");
    let (_, explain) = project.json(&["status", "--explain", "ignored-output/cache.bin"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "git-ignored"
    );
}

#[test]
fn language_filters_are_deferred_and_explicit() {
    let project = Project::seed();
    project.baseline();
    // A formatting-only edit changes the raw fingerprint in this release.
    project.write(
        "src/execution/runner.rs",
        "pub fn run(docs: &[String]) -> usize {\n    docs.len()\n}\n\n",
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    // A nonempty filter configuration fails explicitly instead of silently ignoring it.
    project.write(
        "memoria.toml",
        "version = 1\n\n[fingerprints]\ndefault = \"raw\"\n\n[fingerprints.languages]\npython = \"strip-comments\"\n",
    );
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&status), vec!["configuration_invalid"]);
}

#[test]
fn no_update_result_records_a_meaningful_note() {
    let project = Project::seed();
    project.baseline();
    project.append("src/corpus/types.rs", "// note\n");
    let (packet, token) = project.review_packet("src/corpus/README.md");
    let output = project.ack("src/corpus/README.md", &packet, &token, "no-update", "done");
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("note_invalid"));
    let output = project.ack(
        "src/corpus/README.md",
        &packet,
        &token,
        "no-update",
        "The types module still matches the corpus summary.",
    );
    assert_eq!(output.status.code(), Some(0));
    let state = parse_json(&project.state());
    assert_eq!(
        get_str(&state, &["reviews", "src/corpus/README.md", "result"]),
        "no-update"
    );
    assert_eq!(
        get_str(&state, &["reviews", "src/corpus/README.md", "note"]),
        "The types module still matches the corpus summary."
    );
    assert_eq!(
        get_u64(&state, &["reviews", "src/corpus/README.md", "revision"]),
        2
    );
    assert_eq!(project.status_label("src/corpus/README.md"), "current");
}

#[test]
fn writing_instruction_changes_do_not_stale_documents() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    project.write(
        ".agents/writing.md",
        "# Writing rules\n\nUse Simplified English everywhere.\n",
    );
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("Use short sentences.", "Use very short sentences."),
    );
    assert_eq!(project.json(&["check"]).0, 0);
    assert_eq!(project.state(), before);
    // Future packets carry the new instructions.
    project.append("src/execution/runner.rs", "// edit\n");
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&std::fs::read(packet).unwrap());
    let Json::Array(instructions) = get(&value, &["data", "context", "instructions"]) else {
        panic!()
    };
    let texts: Vec<&str> = instructions.iter().map(|i| get_str(i, &["text"])).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Use very short sentences."))
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Use Simplified English everywhere."))
    );
    // A missing instruction file is a clear error.
    project.remove(".agents/writing.md");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"instruction_file_missing".to_string()));
}

#[test]
fn explicit_invalidation_scopes_and_acknowledgement() {
    let project = Project::seed();
    project.baseline();
    let (code, all) = project.json(&[
        "invalidate",
        "all",
        "--reason",
        "Rewrite every summary in Simplified English.",
    ]);
    assert_eq!(code, 0);
    assert_eq!(get_u64(&all, &["data", "id"]), 1);
    assert_eq!(strings(get(&all, &["data", "targets"])).len(), 6);
    for doc in [
        "README.md",
        "src/corpus/README.md",
        "src/execution/README.md",
    ] {
        assert_eq!(project.cause_codes(doc), vec!["explicit_invalidation"]);
        let status = project.doc_status(doc);
        let Json::Array(causes) = get(&status, &["causes"]) else {
            panic!()
        };
        assert_eq!(
            get_str(&causes[0], &["reason"]),
            "Rewrite every summary in Simplified English."
        );
    }
    let (_, status) = project.json(&["status"]);
    let Json::Array(invalidations) = get(&status, &["data", "invalidations"]) else {
        panic!()
    };
    assert_eq!(
        strings(get(&invalidations[0], &["pending_existing"])).len(),
        6
    );

    // The packet carries the reason and acknowledgement clears only this README.
    let (packet, token) = project.review_packet("src/corpus/README.md");
    let value = parse_json(&std::fs::read(&packet).unwrap());
    let Json::Array(covered) = get(&value, &["data", "covered_invalidations"]) else {
        panic!()
    };
    assert_eq!(get_u64(&covered[0], &["id"]), 1);
    let output = project.ack(
        "src/corpus/README.md",
        &packet,
        &token,
        "updated",
        "Rewrote the corpus summary in Simplified English.",
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(project.status_label("src/corpus/README.md"), "current");
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["explicit_invalidation"]
    );

    // Subtree invalidation captures only retrieval and naive.
    let (code, subtree) = project.json(&[
        "invalidate",
        "subtree:src/retrieval",
        "--reason",
        "Explain retrieval failure modes clearly.",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&subtree, &["data", "targets"])),
        vec!["src/retrieval/README.md", "src/retrieval/naive/README.md"]
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        Vec::<String>::new()
    );
    let (code, _) = project.json(&[
        "invalidate",
        "subtree:src/nothing",
        "--reason",
        "This matches nothing at all.",
    ]);
    assert_eq!(code, 2);
    let (code, doc) = project.json(&[
        "invalidate",
        "doc:src/corpus/README.md",
        "--reason",
        "Add a failure-mode section here.",
    ]);
    assert_eq!(code, 0);
    assert_eq!(get_u64(&doc, &["data", "id"]), 3);
}

#[test]
fn new_invalidation_after_packet_creation_survives_acknowledgement() {
    let project = Project::seed();
    project.baseline();
    project.json(&[
        "invalidate",
        "doc:src/execution/README.md",
        "--reason",
        "Explain the runner failure modes.",
    ]);
    let (packet, token) = project.review_packet("src/execution/README.md");
    project.json(&[
        "invalidate",
        "doc:src/execution/README.md",
        "--reason",
        "Replace outdated terminology in execution.",
    ]);
    let output = project.ack(
        "src/execution/README.md",
        &packet,
        &token,
        "updated",
        "Added the runner failure modes section.",
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let (_, again) = project.json(&["status"]);
    let Json::Array(invalidations) = get(&again, &["data", "invalidations"]) else {
        panic!()
    };
    assert_eq!(invalidations.len(), 1);
    assert_eq!(get_u64(&invalidations[0], &["id"]), 2);
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["explicit_invalidation"]
    );
    let state = parse_json(&project.state());
    assert_eq!(
        numbers(get(
            &state,
            &[
                "reviews",
                "src/execution/README.md",
                "acknowledged_invalidations"
            ]
        )),
        vec![1]
    );
}

#[test]
fn still_pending_flag_reports_newer_invalidations() {
    let project = Project::seed();
    project.baseline();
    project.json(&[
        "invalidate",
        "doc:src/corpus/README.md",
        "--reason",
        "First reason for the corpus.",
    ]);
    let (packet, token) = project.review_packet("src/corpus/README.md");
    project.json(&[
        "invalidate",
        "doc:src/corpus/README.md",
        "--reason",
        "Second reason for the corpus.",
    ]);
    let output = project.run(&[
        "ack",
        "src/corpus/README.md",
        "--packet",
        packet.to_str().unwrap(),
        "--token",
        &token,
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let value = parse_json(&output.stdout);
    assert!(get_bool(&value, &["data", "still_pending"]));
    assert_eq!(
        numbers(get(&value, &["data", "cleared_invalidations"])),
        vec![1]
    );
}

#[test]
fn input_change_after_packet_creation_rejects_acknowledgement() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// v1\n");
    let (packet, token) = project.review_packet("src/execution/README.md");
    let before = project.state();
    project.append("src/execution/runner.rs", "// v2\n");
    let output = project.run(&[
        "ack",
        "src/execution/README.md",
        "--packet",
        packet.to_str().unwrap(),
        "--token",
        &token,
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(3));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    assert_eq!(
        get_str(&changes[0], &["identity"]),
        "src/execution/runner.rs"
    );
    assert_eq!(get_str(&changes[0], &["change"]), "changed");
    assert!(get_str(&changes[0], &["diff"]).contains("+// v2"));
    assert!(get(&changes[0], &["before_hash"]) != get(&changes[0], &["after_hash"]));
    assert_eq!(project.state(), before);

    // README edits also require a fresh packet, even for `updated`.
    project.append("src/execution/runner.rs", "");
    let (packet, token) = project.review_packet("src/execution/README.md");
    project.append("src/execution/README.md", "\nMore prose.\n");
    let output = project.ack(
        "src/execution/README.md",
        &packet,
        &token,
        "updated",
        "Documented the runner change.",
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(project.state(), before);
}

#[test]
fn identical_content_rebase_keeps_reviews_current() {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    let before = project.git(&["rev-parse", "HEAD"]);
    project.git(&[
        "commit",
        "-q",
        "--amend",
        "--allow-empty",
        "-m",
        "rewritten history",
    ]);
    let after = project.git(&["rev-parse", "HEAD"]);
    assert_ne!(before.stdout, after.stdout);
    assert_eq!(project.json(&["check"]).0, 0);
    let (_, status) = project.json(&["status"]);
    assert_eq!(get_u64(&status, &["data", "reviews", "current"]), 6);
}

#[test]
fn export_changes_affect_only_actual_consumers() {
    let project = Project::seed();
    project.baseline();
    let readme = project.read_string("src/retrieval/naive/README.md");
    project.write(
        "src/retrieval/naive/README.md",
        readme.replace(
            "scans every document in order.",
            "scans every document in order, slowly.",
        ),
    );
    assert_eq!(
        project.cause_codes("src/retrieval/naive/README.md"),
        vec!["document_changed"]
    );
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(project.cause_codes("README.md"), Vec::<String>::new());
    assert_eq!(
        project.waiting_on("README.md"),
        vec!["src/retrieval/README.md"]
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        Vec::<String>::new()
    );
    project.ack_ok("src/retrieval/naive/README.md");

    // Retrieval must render before its packet exists, then stays the end of propagation.
    let (code, refused) = project.json(&["review", "src/retrieval/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["imports_outdated"]);
    assert_eq!(project.json(&["render", "src/retrieval/README.md"]).0, 0);
    assert!(
        project
            .read_string("src/retrieval/README.md")
            .contains("in order, slowly.")
    );
    project.ack_ok("src/retrieval/README.md");
    assert_eq!(project.status_label("README.md"), "current");
    assert!(project.waiting_on("README.md").is_empty());
    assert_eq!(project.json(&["check"]).0, 0);

    // Changing retrieval's own export reaches the root, and nothing else.
    let retrieval = project.read_string("src/retrieval/README.md");
    project.write(
        "src/retrieval/README.md",
        retrieval.replace("relevant to a query.", "relevant to a query quickly."),
    );
    project.ack_ok("src/retrieval/README.md");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    assert_eq!(project.status_label("src/execution/README.md"), "current");
    assert_eq!(project.status_label("src/corpus/README.md"), "current");
    assert_eq!(project.json(&["render"]).0, 0);
    project.ack_ok("README.md");
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn prose_changes_outside_exports_keep_consumers_current() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/README.md", "\nUnrelated prose at the end.\n");
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["document_changed"]
    );
    assert_eq!(project.cause_codes("README.md"), Vec::<String>::new());
    assert_eq!(
        project.cause_codes("src/retrieval/naive/README.md"),
        Vec::<String>::new()
    );
    assert_eq!(
        project.waiting_on("src/retrieval/naive/README.md"),
        vec!["src/execution/README.md"]
    );
    let state = parse_json(&project.state());
    let stored = get(
        &state,
        &[
            "reviews",
            "src/retrieval/naive/README.md",
            "input_manifest",
            "imports",
        ],
    );
    project.ack_ok("src/execution/README.md");
    let (_, plan) = project.json(&["review"]);
    assert!(matches!(get(&plan, &["data", "tasks"]), Json::Array(t) if t.is_empty()));
    let state = parse_json(&project.state());
    assert_eq!(
        get(
            &state,
            &[
                "reviews",
                "src/retrieval/naive/README.md",
                "input_manifest",
                "imports"
            ]
        ),
        stored
    );
}

#[test]
fn normal_link_change_creates_no_dependency() {
    let project = Project::seed();
    project.baseline();
    project.append("src/corpus/types.rs", "// corpus change\n");
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["input_changed"]
    );
    assert!(project.waiting_on("README.md").is_empty());
    assert_eq!(project.status_label("README.md"), "current");
    // The hint is non-blocking and suppressible.
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 1);
    assert_eq!(
        diagnostic_codes(&check)
            .iter()
            .filter(|c| *c == "review_pending")
            .count(),
        1
    );
    project.ack_ok("src/corpus/README.md");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 0);
    assert!(diagnostic_codes(&lint).contains(&"missing_import_hint".to_string()));
    project.append("memoria.toml", "[lint]\nmissing_import_hint = false\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 0);
    assert!(!diagnostic_codes(&lint).contains(&"missing_import_hint".to_string()));
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn render_is_idempotent_and_preserves_authored_text() {
    let project = Project::seed();
    assert_eq!(project.run(&["init"]).status.code(), Some(0));
    let authored_before = project.read_string("README.md");
    let (code, dry) = project.json(&["render", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(get_bool(&dry, &["data", "dry_run"]));
    assert_eq!(project.read_string("README.md"), authored_before);
    assert_eq!(project.json(&["render"]).0, 0);
    let rendered = project.read_string("README.md");
    assert!(rendered.contains("Retrieval selects documents that are relevant to a query."));
    // Everything outside the import interiors is byte-identical.
    let strip = |text: &str| -> String {
        let mut out = String::new();
        let mut inside = false;
        for line in text.split_inclusive('\n') {
            if line.starts_with("<!-- memoria:import") {
                inside = true;
                out.push_str(line);
            } else if line.starts_with("<!-- /memoria:import") {
                inside = false;
                out.push_str(line);
            } else if !inside {
                out.push_str(line);
            }
        }
        out
    };
    assert_eq!(strip(&rendered), strip(&authored_before));
    let snapshot = project.tree_snapshot();
    let (code, second) = project.json(&["render"]);
    assert_eq!(code, 0);
    assert_eq!(get_u64(&second, &["data", "documents_changed"]), 0);
    assert_eq!(project.tree_snapshot(), snapshot);
    assert_eq!(project.read_string("README.md"), rendered);
}

#[test]
fn missing_imports_and_cycles_report_exact_references() {
    let project = Project::seed();
    project.baseline();
    project.append("src/corpus/README.md", "\n<!-- memoria:import src=\"../missing/README.md#summary\" -->\n<!-- /memoria:import -->\n<!-- memoria:import src=\"../execution/README.md#nope\" -->\n<!-- /memoria:import -->\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let missing = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "import_missing_document")
        .unwrap();
    assert_eq!(
        get_str(missing, &["details", "target"]),
        "src/missing/README.md"
    );
    assert_eq!(get_u64(missing, &["line"]), 11);
    let export = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "import_missing_export")
        .unwrap();
    assert_eq!(get_str(export, &["details", "export_id"]), "nope");
    assert_eq!(project.json(&["check"]).0, 1);
    assert_eq!(project.json(&["review"]).0, 1);

    // A cycle: execution imports root, root imports retrieval, retrieval imports naive, naive imports execution.
    let project = Project::seed();
    project.baseline();
    project.append(
        "src/execution/README.md",
        "\n<!-- memoria:import src=\"../../README.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let cycle = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "import_cycle")
        .unwrap();
    // The root also imports execution directly, so the shortest closed loop is reported first.
    assert_eq!(
        strings(get(cycle, &["details", "path"])),
        vec!["README.md", "src/execution/README.md", "README.md"]
    );
    let Json::Array(edges) = get(cycle, &["details", "edges"]) else {
        panic!()
    };
    assert_eq!(edges.len(), 2);
    assert_eq!(get_str(&edges[0], &["importer"]), "README.md");
    assert_eq!(get_str(&edges[0], &["provider"]), "src/execution/README.md");
    assert_eq!(get_u64(&edges[0], &["line"]), 23);
    assert_eq!(get_str(&edges[1], &["importer"]), "src/execution/README.md");
    assert_eq!(get_u64(&edges[1], &["line"]), 11);
    assert_eq!(project.json(&["render"]).0, 1);
}

#[test]
fn disconnected_readme_stays_visible_with_a_warning() {
    let project = Project::seed();
    project.baseline();
    let (code, graph) = project.json(&["graph"]);
    assert_eq!(code, 0);
    let Json::Array(nodes) = get(&graph, &["data", "nodes"]) else {
        panic!()
    };
    let node = nodes
        .iter()
        .find(|n| get_str(n, &["document"]) == "src/disconnected/README.md")
        .unwrap();
    assert!(get_bool(node, &["disconnected"]));
    assert_eq!(get_u64(node, &["owned_files"]), 1);
    let Json::Array(edges) = get(&graph, &["data", "edges"]) else {
        panic!()
    };
    let kinds: Vec<(&str, &str, &str)> = edges
        .iter()
        .map(|e| {
            (
                get_str(e, &["kind"]),
                get_str(e, &["from"]),
                get_str(e, &["to"]),
            )
        })
        .collect();
    assert!(kinds.contains(&("owner", "README.md", "src/disconnected/README.md")));
    assert!(kinds.contains(&(
        "import",
        "src/retrieval/naive/README.md",
        "src/execution/README.md"
    )));
    assert!(kinds.contains(&("link", "README.md", "src/corpus/README.md")));
    assert!(kinds.contains(&(
        "owner",
        "src/retrieval/README.md",
        "src/retrieval/naive/README.md"
    )));
    let Json::Array(diagnostics) = get(&graph, &["diagnostics"]) else {
        panic!()
    };
    let warning = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "navigation_disconnected")
        .unwrap();
    assert_eq!(get_str(warning, &["severity"]), "warning");
    assert_eq!(get_u64(warning, &["details", "owned_files"]), 1);
    // Linking to it from the root removes the warning.
    project.append("README.md", "\nSee [disconnected](src/disconnected/).\n");
    let (_, lint) = project.json(&["lint"]);
    assert!(!diagnostic_codes(&lint).contains(&"navigation_disconnected".to_string()));
}

#[test]
fn policy_changes_invalidate_only_inheriting_scopes() {
    let project = Project::seed();
    project.baseline();
    // A root ignore that matches nothing still changes every owner's policy.
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "version = 1\n",
            "version = 1\ninclude = [\n    \"nothing/**\",\n]\n",
        ),
    );
    for doc in [
        "README.md",
        "src/corpus/README.md",
        "src/retrieval/naive/README.md",
    ] {
        assert_eq!(project.cause_codes(doc), vec!["input_changed"]);
    }
    while let Some(next) = project.next_ready() {
        project.ack_ok(&next);
    }
    // A sidecar change affects only its scope.
    project.write(
        "src/retrieval/README.memoria.toml",
        "include = [\n    \"fixtures/**\",\n]\nignore = [\n    \"nothing.txt\",\n]\n",
    );
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/retrieval/naive/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        Vec::<String>::new()
    );
    assert_eq!(project.cause_codes("README.md"), Vec::<String>::new());
    while let Some(next) = project.next_ready() {
        project.ack_ok(&next);
    }
    // A comment-only configuration change means nothing.
    let before = project.state();
    project.append("memoria.toml", "# trailing comment\n");
    assert_eq!(project.json(&["check"]).0, 0);
    // A nested .gitignore rule change is policy for its owner.
    project.write("src/execution/.gitignore", "*.tmp\n");
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        Vec::<String>::new()
    );
    assert_eq!(
        project.state(),
        before,
        "read-only commands never touch state"
    );
}
