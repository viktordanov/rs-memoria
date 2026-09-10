//! Project documentation guidance: visible, advisory, and bound into the
//! reviewed context without deciding byte-based freshness.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

/// The effective guidance digest of one document, from the read-only command.
fn digest(project: &Project, document: &str) -> String {
    let (code, value) = project.json(&["guidance", document]);
    assert_eq!(code, 0, "guidance {document}: {value:?}");
    get_str(&value, &["data", "digest"]).to_string()
}

fn entries(value: &Json) -> Vec<(String, String, String)> {
    let Json::Array(items) = get(value, &["data", "entries"]) else {
        panic!("entries is not an array")
    };
    items
        .iter()
        .map(|entry| {
            (
                get_str(entry, &["kind"]).to_string(),
                get_str(entry, &["source"]).to_string(),
                get_str(entry, &["text"]).to_string(),
            )
        })
        .collect()
}

#[test]
fn init_preview_teaches_strategy_and_writes_nothing() {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n\nAuthored first.\n");
    project.commit_all("seed");
    let before = project.tree_snapshot();
    let index_before = project.git(&["ls-files", "--stage"]).stdout;
    for format in ["json", "human"] {
        let output = project.run(&["init", "--format", format]);
        assert_eq!(output.status.code(), Some(0), "{format}");
        let text = stdout(&output);
        assert!(
            text.contains("You choose") || text.contains("you choose"),
            "{format}: the author chooses the documentation model"
        );
        assert!(text.contains("memoria.toml"), "{format}");
        assert!(text.contains("memoria.lock"), "{format}");
        assert!(
            text.contains("architecture modules")
                && text.contains("business concepts")
                && text.contains("operational workflows"),
            "{format}: three strategies, none chosen"
        );
        assert!(text.contains("memoria review"), "{format}: next command");
    }
    let (_, preview) = project.json(&["init"]);
    assert!(!get_bool(&preview, &["data", "applied"]));
    assert!(get_bool(&preview, &["data", "root_readme_present"]));
    assert_eq!(project.tree_snapshot(), before, "the preview wrote nothing");
    assert_eq!(project.git(&["ls-files", "--stage"]).stdout, index_before);
}

#[test]
fn init_apply_requires_authored_root_and_preserves_existing_files() {
    let project = Project::empty_repo();
    project.write("notes.md", "# Notes\n");
    project.commit_all("seed");
    let before = project.tree_snapshot();
    let (code, refused) = project.json(&["init", "--apply"]);
    assert_eq!(code, 1, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["root_readme_missing"]);
    assert_eq!(project.tree_snapshot(), before, "apply wrote nothing");
    // With an authored root README, apply creates only the two files.
    project.write("README.md", "# Root\n\nAuthored by the project.\n");
    let (code, applied) = project.json(&["init", "--apply"]);
    assert_eq!(code, 0, "{applied:?}");
    assert_eq!(
        strings(get(&applied, &["data", "created"])),
        vec!["memoria.toml", "memoria.lock"]
    );
    assert_eq!(
        project.read_string("README.md"),
        "# Root\n\nAuthored by the project.\n",
        "apply never writes README prose"
    );
    assert!(!project.exists("README.memoria.toml"), "no sidecar");
    assert!(!project.exists("docs"), "no hierarchy");
    // The generated configuration is generic, with empty guidance.
    let config = project.read_string("memoria.toml");
    assert!(config.contains("version = 2"), "{config}");
    assert!(config.contains("guidance = []"), "{config}");
    assert!(config.contains("guidance_files = []"), "{config}");
    assert!(
        !config.contains("simple-english") && !config.contains("i-have-adhd"),
        "the template imposes no project's writing skills"
    );
    // An existing valid installation produces no changes.
    let after = project.tree_snapshot();
    let (code, again) = project.json(&["init", "--apply"]);
    assert_eq!(code, 0, "{again:?}");
    assert!(strings(get(&again, &["data", "created"])).is_empty());
    assert_eq!(project.tree_snapshot(), after);
}

#[test]
fn legacy_guidance_keys_have_actionable_errors() {
    let project = Project::seed();
    project.baseline();
    for (label, config) in [
        (
            "old version",
            "version = 1\n[documentation]\nguidance = []\n",
        ),
        (
            "instructions",
            "version = 2\n[documentation]\ninstructions = [\"Old key.\"]\n",
        ),
        (
            "instruction_files",
            "version = 2\n[documentation]\ninstruction_files = [\"x.md\"]\n",
        ),
        (
            "mixed",
            "version = 2\n[documentation]\nguidance = [\"New.\"]\ninstructions = [\"Old.\"]\n",
        ),
    ] {
        project.write("memoria.toml", config);
        let (code, value) = project.json(&["status"]);
        assert_eq!(code, 1, "{label}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["configuration_invalid"],
            "{label}"
        );
        let message = memoria_infrastructure::json::to_compact(&value);
        assert!(
            message.contains("documentation.guidance") || message.contains("version 2"),
            "{label}: the error names the replacement: {message}"
        );
        assert!(
            message.contains("docs/releases/0.2.0.md"),
            "{label}: the error names the cutover: {message}"
        );
    }
}

#[test]
fn effective_guidance_preserves_scope_order_and_exact_text() {
    let project = Project::seed();
    project.write(
        "memoria.toml",
        "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n\n[documentation]\nguidance = [\"Root one.\", \"Root two: keep '#' literal.\"]\nguidance_files = [\".agents/writing.md\", \"docs/extra.md\"]\n",
    );
    project.write("docs/extra.md", "Root file text with caf\u{e9}.\n");
    project.write(
        "src/retrieval/README.memoria.toml",
        "include = [\"fixtures/**\"]\n[documentation]\nguidance = [\"Local one.\"]\nguidance_files = [\"local.md\"]\n",
    );
    project.write("src/retrieval/local.md", "Local file text.\n");
    project.baseline();
    let (code, value) = project.json(&["guidance", "src/retrieval/README.md"]);
    assert_eq!(code, 0, "{value:?}");
    let observed = entries(&value);
    // Root scope first, inline before file, authored order preserved.
    assert_eq!(
        observed,
        vec![
            ("inline".into(), "memoria.toml".into(), "Root one.".into()),
            (
                "inline".into(),
                "memoria.toml".into(),
                "Root two: keep '#' literal.".into()
            ),
            (
                "file".into(),
                ".agents/writing.md".into(),
                "# Writing rules\n\nExplain each part before its details. Prefer plain words.\n"
                    .into()
            ),
            (
                "file".into(),
                "docs/extra.md".into(),
                "Root file text with caf\u{e9}.\n".into()
            ),
            (
                "inline".into(),
                "src/retrieval/README.memoria.toml".into(),
                "Local one.".into()
            ),
            (
                "file".into(),
                "src/retrieval/local.md".into(),
                "Local file text.\n".into()
            ),
        ]
    );
    // A boundary outside the sidecar scope sees only the root entries.
    let (_, root) = project.json(&["guidance", "src/corpus/README.md"]);
    assert_eq!(entries(&root).len(), 4);
    // The scope list names every contributing scope and its command.
    let Json::Array(scopes) = get(&value, &["data", "scopes"]) else {
        panic!()
    };
    assert_eq!(scopes.len(), 2);
    assert!(
        scopes
            .iter()
            .any(|s| get_str(s, &["inspect_command"]) == "memoria guidance README.md")
    );
}

#[test]
fn guidance_is_visible_for_current_and_pending_documents() {
    let project = Project::seed();
    project.baseline();
    // The document is current; `guidance` still works and changes nothing.
    let state = project.state();
    let current = digest(&project, "src/corpus/README.md");
    assert_eq!(current.len(), 16);
    assert_eq!(project.state(), state, "guidance is read-only");
    // A focused review of a current document is still refused.
    let output = project.run(&["review", "src/corpus/README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["review_not_pending"]
    );
    // Status publishes presence and the per-document summary.
    let (_, status) = project.json(&["status"]);
    assert_eq!(
        get(&status, &["data", "guidance", "documents_with_guidance"]),
        &Json::Number(6)
    );
    let doc = project.doc_status("src/corpus/README.md");
    assert!(get_bool(&doc, &["guidance", "present"]));
    assert_eq!(get_str(&doc, &["guidance", "current_digest"]), current);
    // The review plan names the guidance command for each due boundary.
    project.append("src/corpus/types.rs", "// pending\n");
    let plan = project.plan();
    assert!(
        get_str(&plan, &["data", "guidance_first"]).contains("memoria guidance"),
        "the plan tells the reviewer to read guidance first"
    );
    let Json::Array(tasks) = get(&plan, &["data", "tasks"]) else {
        panic!()
    };
    assert!(
        tasks.iter().any(|t| {
            get_str(t, &["guidance_command"]) == "memoria guidance src/corpus/README.md"
        })
    );
    // Canonical JSON and the explicit full human view carry the same guidance.
    let (packet, _) = project.review_packet("src/corpus/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    assert_eq!(
        get_str(&value, &["data", "context", "guidance", "digest"]),
        current
    );
    let brief = project.run(&["review", "src/corpus/README.md"]);
    assert!(stdout(&brief).contains("Guidance:"));
    let human = project.run(&["review", "src/corpus/README.md", "--full"]);
    assert!(
        stdout(&human).contains("Project documentation guidance"),
        "the human packet shows guidance before the owned evidence"
    );
    assert!(stdout(&human).contains("Use short sentences."));
}

#[test]
fn guidance_edits_are_advisory_until_explicit_invalidation() {
    let project = Project::seed();
    project.baseline();
    let state = project.state();
    let before = digest(&project, "README.md");
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("\"Use short sentences.\",", "\"Use shorter sentences.\","),
    );
    let after = digest(&project, "README.md");
    assert_ne!(before, after, "the digest tracks the wording");
    // No policy change, no input change, no pending review.
    assert_eq!(
        project.cause_codes("README.md"),
        Vec::<String>::new(),
        "changed guidance never makes a document stale"
    );
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 0, "check still passes: {check:?}");
    assert_eq!(
        get(&check, &["data", "guidance", "changed_documents"]),
        &Json::Number(6),
        "the advisory count is visible"
    );
    // The advisory travels as a hint beside the existing warnings and hints,
    // and `check` still exits 0.
    assert!(
        diagnostic_codes(&check).contains(&"guidance_changed".to_string()),
        "{:?}",
        diagnostic_codes(&check)
    );
    let Json::Array(diagnostics) = get(&check, &["diagnostics"]) else {
        panic!()
    };
    let advisory = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "guidance_changed")
        .expect("the advisory diagnostic");
    assert_eq!(get_str(advisory, &["severity"]), "hint");
    assert!(
        diagnostics
            .iter()
            .all(|d| get_str(d, &["severity"]) != "error"),
        "changed guidance never becomes an error"
    );
    let (_, status) = project.json(&["status"]);
    assert_eq!(
        get(&status, &["data", "guidance", "changed_documents"]),
        &Json::Number(6)
    );
    let doc = project.doc_status("README.md");
    assert!(get_bool(&doc, &["guidance", "changed_since_review"]));
    assert_eq!(project.state(), state, "no state was written");
}

#[test]
fn explicit_guidance_invalidation_records_the_reviewed_digest() {
    let project = Project::seed();
    project.baseline();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "\"Use short sentences.\",",
            "\"Explain the workflow first.\",",
        ),
    );
    let current = digest(&project, "src/execution/README.md");
    let (code, _) = project.json(&[
        "invalidate",
        "subtree:src/execution",
        "--reason",
        "The workflow explanation has new requirements",
    ]);
    assert_eq!(code, 0);
    project.ack_ok("src/execution/README.md");
    let state = project.inspect_state();
    assert_eq!(
        get_str(
            &state,
            &["reviews", "src/execution/README.md", "guidance_digest"]
        ),
        current,
        "the review stores the guidance its reviewer saw"
    );
    let doc = project.doc_status("src/execution/README.md");
    assert!(!get_bool(&doc, &["guidance", "changed_since_review"]));
    // The covered invalidation is cleared for that document.
    let (_, status) = project.json(&["status"]);
    let Json::Array(open) = get(&status, &["data", "invalidations"]) else {
        panic!()
    };
    assert!(open.iter().all(|i| {
        !strings(get(i, &["pending_existing"])).contains(&"src/execution/README.md".to_string())
    }));
}

#[test]
fn guidance_change_during_ack_is_a_context_conflict() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let (packet, token) = project.review_packet("src/execution/README.md");
    let state = project.state();
    // Guidance changes after the packet was created.
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "\"Use short sentences.\",",
            "\"Use very short sentences.\",",
        ),
    );
    let (code, value) = project.ack_json(
        "src/execution/README.md",
        &packet,
        &token,
        "no-update",
        NOTE,
    );
    assert_eq!(
        code,
        3,
        "{}",
        memoria_infrastructure::json::to_compact(&value)
    );
    assert_eq!(diagnostic_codes(&value), vec!["guidance_changed"]);
    assert_eq!(project.state(), state, "no review state changed");
    // The document is still pending for its byte change, not for guidance.
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    // A fresh packet acknowledges.
    let (packet, token) = project.review_packet("src/execution/README.md");
    let output = project.ack(
        "src/execution/README.md",
        &packet,
        &token,
        "no-update",
        NOTE,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn guidance_limits_and_unsafe_paths_fail_without_omission() {
    let project = Project::seed();
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("rules.md"), "outside\n").unwrap();
    for (label, entry, code) in [
        ("missing", "\"no-such-file.md\"", "guidance_file_missing"),
        ("escape", "\"../outside.md\"", "guidance_file_invalid"),
        ("state", "\"memoria.lock\"", "guidance_file_invalid"),
        ("configuration", "\"memoria.toml\"", "guidance_file_invalid"),
        ("readme", "\"README.md\"", "guidance_file_invalid"),
        ("git", "\".git/config\"", "guidance_file_invalid"),
    ] {
        project.write(
            "memoria.toml",
            format!(
                "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n[documentation]\nguidance_files = [{entry}]\n"
            ),
        );
        let (exit, lint) = project.json(&["lint"]);
        assert_eq!(exit, 1, "{label}: {lint:?}");
        assert!(
            diagnostic_codes(&lint).contains(&code.to_string()),
            "{label}: {:?}",
            diagnostic_codes(&lint)
        );
    }
    // A symlinked guidance file is rejected, not followed.
    project.write(
        "memoria.toml",
        "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n[documentation]\nguidance_files = [\"linked.md\"]\n",
    );
    std::os::unix::fs::symlink(
        outside.path().join("rules.md"),
        project.root.join("linked.md"),
    )
    .unwrap();
    let (exit, lint) = project.json(&["lint"]);
    assert_eq!(exit, 1);
    assert!(diagnostic_codes(&lint).contains(&"guidance_file_invalid".to_string()));
    // Oversized guidance produces an explicit limit error, never silence.
    fs::remove_file(project.root.join("linked.md")).unwrap();
    project.write("huge.md", "x".repeat(33 * 1024 * 1024));
    project.write(
        "memoria.toml",
        "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\", \"huge.md\"]\n[documentation]\nguidance_files = [\"huge.md\"]\n",
    );
    project.append("src/execution/runner.rs", "// pending\n");
    let output = project.run(&["review", "src/execution/README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_too_large"],
        "the packet budget counts every guidance byte"
    );
    assert!(
        !stdout(&output).contains("xxxxxxxxxx"),
        "the refusal never carries the guidance text"
    );
}

#[test]
fn guidance_role_changes_preserve_real_selection_diffs() {
    let project = Project::seed();
    project.write("src/corpus/notes.md", "Notes about the corpus.\n");
    project.baseline();
    let before = digest(&project, "src/corpus/README.md");
    // Designating a previously selected file as guidance is a real manifest
    // change for its owner.
    project.write(
        "memoria.toml",
        "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n[documentation]\nguidance = [\"Use short sentences.\"]\nguidance_files = [\".agents/writing.md\", \"src/corpus/notes.md\"]\n",
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["input_changed"],
        "the file left the owner's selected inputs"
    );
    assert_ne!(before, digest(&project, "src/corpus/README.md"));
    let (packet, _) = project.review_packet("src/corpus/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(changes) = get(&value, &["data", "context", "changes"]) else {
        panic!()
    };
    assert!(
        changes
            .iter()
            .any(|c| get_str(c, &["identity"]) == "src/corpus/notes.md"
                && get_str(c, &["change"]) == "removed"),
        "{changes:?}"
    );
    // Editing an already-designated guidance file is advisory only.
    project.ack_ok("src/corpus/README.md");
    let state = project.state();
    project.write("src/corpus/notes.md", "Different notes about the corpus.\n");
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        Vec::<String>::new(),
        "guidance text edits never stale a document"
    );
    assert_eq!(project.json(&["check"]).0, 0);
    assert_eq!(project.state(), state);
}

#[test]
fn guidance_never_hides_a_missing_or_invalid_guidance_file() {
    // A guidance file that contributes no entry must never look like
    // "this boundary has no guidance". The dedicated command reports the
    // same error that `status` reports, for a present document too.
    for (label, entry, code) in [
        ("missing", "\"missing.md\"", "guidance_file_missing"),
        ("escape", "\"../outside.md\"", "guidance_file_invalid"),
        ("readme", "\"README.md\"", "guidance_file_invalid"),
    ] {
        let project = Project::seed();
        project.baseline();
        project.write(
            "memoria.toml",
            format!(
                "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n[documentation]\nguidance = [\"Root rule.\"]\nguidance_files = [{entry}]\n"
            ),
        );
        // `status` already reports it.
        let (status_code, status) = project.json(&["status"]);
        assert_eq!(status_code, 1, "{label}: {status:?}");
        assert!(
            diagnostic_codes(&status).contains(&code.to_string()),
            "{label}: {:?}",
            diagnostic_codes(&status)
        );
        // The dedicated command must agree, with and without a path.
        for args in [vec!["guidance"], vec!["guidance", "README.md"]] {
            let (exit, value) = project.json(&args);
            assert_eq!(exit, 1, "{label} {args:?}: {value:?}");
            assert!(
                diagnostic_codes(&value).contains(&code.to_string()),
                "{label} {args:?}: {:?}",
                diagnostic_codes(&value)
            );
            assert!(
                !get_bool(&value, &["ok"]),
                "{label} {args:?}: a hidden error must not report success"
            );
        }
    }
    // A symlinked guidance file is refused rather than followed.
    let project = Project::seed();
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("rules.md"), "outside\n").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("rules.md"),
        project.root.join("linked.md"),
    )
    .unwrap();
    project.write(
        "memoria.toml",
        "version = 2\nignore = [\"**/generated/**\", \"**/fixtures/**\"]\n[documentation]\nguidance_files = [\"linked.md\"]\n",
    );
    let (exit, value) = project.json(&["guidance", "README.md"]);
    assert_eq!(exit, 1, "{value:?}");
    assert!(diagnostic_codes(&value).contains(&"guidance_file_invalid".to_string()));
    // A valid project still reports guidance with exit 0.
    let clean = Project::seed();
    clean.baseline();
    let (exit, value) = clean.json(&["guidance", "README.md"]);
    assert_eq!(exit, 0, "{value:?}");
    assert!(get_bool(&value, &["ok"]));
    assert!(!strings(get(&value, &["data", "sources"])).is_empty());
}
