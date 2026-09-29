//! The 0.7.0 cutover (plan §5): configuration version 3 only, committed
//! 0.6 state read as-is, and an honest review queue. The fixtures under
//! `tests/fixtures/upgrade-0.6` were acknowledged by the real Memoria 0.6.0
//! executable, so their `memoria.lock` bytes are genuine 0.6 output.

mod common;
#[path = "common/counting.rs"]
mod counting;

use common::*;
use memoria_infrastructure::json::Json;

fn bump(project: &Project) {
    let text = project.read_string("memoria.toml");
    assert!(text.contains("version = 2\n"));
    project.write(
        "memoria.toml",
        text.replace("version = 2\n", "version = 3\n"),
    );
}

#[test]
fn version_two_is_refused_with_the_cutover_message() {
    let project = Project::seed_from("upgrade-0.6/linked");
    // Git may refresh its own index stat data; the project files must not
    // change.
    let worktree = |project: &Project| {
        let mut tree = project.tree_snapshot();
        tree.retain(|path, _| !path.starts_with(".git/"));
        tree
    };
    let before = worktree(&project);
    for args in [vec!["status"], vec!["check"], vec!["review"], vec!["lint"]] {
        let (code, value) = project.json(&args);
        assert_eq!(code, 1, "{args:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["configuration_invalid"],
            "{args:?}"
        );
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        let message = get_str(&diagnostics[0], &["message"]);
        assert!(
            message.starts_with(
                "memoria.toml declares version 2. Memoria 0.7 reads configuration version 3:"
            ),
            "{message}"
        );
        assert!(
            message.contains("Change `version = 2` to `version = 3`"),
            "{message}"
        );
        assert!(message.contains("Migration to 0.7.0"), "{message}");
    }
    assert_eq!(worktree(&project), before, "the refusal is read-only");
}

#[test]
fn other_versions_and_sidecars_are_refused_exactly() {
    let project = Project::seed_from("upgrade-0.6/linked");
    for (text, needle) in [
        ("version = 1\n", "this release reads version 3 only"),
        (
            "version = 4\n",
            "unsupported version 4; this release reads version 3 only.",
        ),
    ] {
        project.write("memoria.toml", text);
        let (code, value) = project.json(&["status"]);
        assert_eq!(code, 1);
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        assert!(
            get_str(&diagnostics[0], &["message"]).contains(needle),
            "{text}"
        );
    }
    project.write("memoria.toml", "version = 3\n");
    project.write("child/README.memoria.toml", "version = 2\n");
    let (code, value) = project.json(&["status"]);
    assert_eq!(code, 1, "{value:?}");
    assert!(diagnostic_codes(&value).contains(&"sidecar_invalid".to_string()));
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let sidecar = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "sidecar_invalid")
        .unwrap();
    assert!(
        get_str(sidecar, &["message"]).contains("declares version 2"),
        "{sidecar:?}"
    );
}

#[test]
fn a_fully_linked_0_6_project_stays_current_with_unchanged_state() {
    let project = Project::seed_from("upgrade-0.6/linked");
    let state = project.state();
    bump(&project);
    project.commit_all("bump the configuration version");
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 0, "{check:?}");
    assert!(project.pending().is_empty());
    // The 0.6 records are the baselines, read as-is.
    assert_eq!(project.state(), state, "no command rewrote the lock");
    let (_, summary) = project.json(&["status", "--summary"]);
    assert_eq!(get_u64(&summary, &["data", "handoffs"]), 1);
    assert_eq!(get_u64(&summary, &["data", "overlapping_sources"]), 0);
    // Re-encoding the 0.6 state reproduces it byte for byte.
    let inspected = project.inspect_envelope();
    assert_eq!(get_u64(&inspected, &["data", "format_version"]), 2);
}

#[test]
fn an_unlinked_child_readme_makes_its_parent_pending_honestly() {
    let project = Project::seed_from("upgrade-0.6/unlinked");
    let state = project.state();
    bump(&project);
    project.commit_all("bump the configuration version");
    assert_eq!(project.pending(), vec!["README.md"]);
    assert_eq!(project.status_label("child/README.md"), "current");
    let (_, manifest) = project.json(&["review", "README.md"]);
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    let added: Vec<(&str, &str, &str)> = changes
        .iter()
        .map(|c| {
            (
                get_str(c, &["change"]),
                get_str(c, &["identity"]),
                get_str(c, &["relationship", "kind"]),
            )
        })
        .collect();
    assert_eq!(added, vec![("added", "child/lib.rs", "handoff")]);
    let reasons = project.fallbacks("README.md");
    assert_eq!(
        reasons,
        vec![("handoff_changed".to_string(), "child".to_string())]
    );
    let Json::Array(fallbacks) = get(&manifest, &["data", "review", "fallback_reasons"]) else {
        panic!()
    };
    assert!(
        get_str(&fallbacks[0], &["message"]).contains(
            "0.7 hands a folder to a document only when this document links or imports it"
        )
    );
    // The hint names the fix, and nothing was written.
    let absent = project.lint_diagnostics("handoff_absent");
    assert_eq!(
        get_str(&absent[0], &["details", "target"]),
        "child/README.md"
    );
    assert_eq!(project.state(), state);
    // Adding the link hands the folder off: the root is pending for its own
    // text only, and after review the queue is empty.
    project.append(
        "README.md",
        "\nThe [child library](child/README.md) explains `child/`.\n",
    );
    assert_eq!(project.cause_codes("README.md"), vec!["document_changed"]);
    project.ack_ok("README.md");
    assert!(project.pending().is_empty());
}

#[test]
fn disposable_artifacts_from_0_6_are_refused() {
    let project = Project::seed_from("upgrade-0.6/linked");
    bump(&project);
    project.append("app.rs", "// change\n");
    let (packet, token) = project.review_packet("README.md");
    let mut artifact = parse_json(&std::fs::read(&packet).unwrap());
    set_path(
        &mut artifact,
        &["data", "manifest_version"],
        Json::Number(1),
    );
    let old = project.packets.path().join("old.json");
    std::fs::write(&old, memoria_infrastructure::json::to_pretty(&artifact)).unwrap();
    let (code, value) = project.ack_json("README.md", &old, &token, "no-update", NOTE);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["packet_schema_invalid"]);
}

// R1 case D: a genuine 0.6 upgrade keeps its handoff classification when the
// 0.6 child README was acknowledged after its parent. The fixture's lock was
// written by the real Memoria 0.6.0, with the child reviewed one second
// after the root.
#[test]
fn a_later_child_review_keeps_the_upgrade_honest() {
    let project = Project::seed_from("upgrade-0.6/unlinked-child-later");
    let state = project.state();
    let inspected = project.inspect_state();
    assert!(
        get_str(&inspected, &["reviews", "README.md", "reviewed_at"])
            < get_str(&inspected, &["reviews", "child/README.md", "reviewed_at"]),
        "the fixture reviews the child later"
    );
    bump(&project);
    project.commit_all("bump the configuration version");
    assert_eq!(project.pending(), vec!["README.md"]);
    let (_, manifest) = project.json(&["review", "README.md"]);
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    let files: Vec<(&str, &str, &str)> = changes
        .iter()
        .map(|c| {
            (
                get_str(c, &["change"]),
                get_str(c, &["identity"]),
                get_str(c, &["relationship", "kind"]),
            )
        })
        .collect();
    assert_eq!(files, vec![("added", "child/lib.rs", "handoff")]);
    assert_eq!(
        project.fallbacks("README.md"),
        vec![("handoff_changed".to_string(), "child".to_string())]
    );
    assert_eq!(get_u64(&manifest, &["data", "scope", "files"]), 2);
    assert_eq!(
        get_str(&manifest, &["data", "review", "mode"]),
        "full_baseline"
    );
    assert!(get_bool(
        &manifest,
        &["data", "review", "whole_document_pass"]
    ));
    assert_eq!(
        project.covered_by("child/lib.rs"),
        "README.md, child/README.md"
    );
    assert_eq!(project.state(), state, "the upgrade rewrote nothing");
}

// ---------------------------------------------------------------------------
// Records without coverage evidence (plan §17.3, §17.8 items 11-15). Every
// fixture below was acknowledged by the real Memoria 0.6.0 executable.

/// Seed a 0.6 fixture, move it to configuration version 3, and commit.
fn upgraded(name: &str) -> Project {
    let project = Project::seed_from(&format!("upgrade-0.6/{name}"));
    bump(&project);
    project.commit_all("bump the configuration version");
    project
}

/// `(change, identity, relationship kind, unrecorded reason)` per file change.
fn file_changes(manifest: &Json) -> Vec<(String, String, String, Option<String>)> {
    let Json::Array(changes) = get(manifest, &["data", "changes"]) else {
        panic!()
    };
    changes
        .iter()
        .filter(|c| get_str(c, &["kind"]) == "file")
        .map(|c| {
            (
                get_str(c, &["change"]).to_string(),
                get_str(c, &["identity"]).to_string(),
                get_str(c, &["relationship", "kind"]).to_string(),
                match get(c, &["relationship", "unrecorded_reason"]) {
                    Json::String(reason) => Some(reason.clone()),
                    _ => None,
                },
            )
        })
        .collect()
}

/// Review the root of an unlinked 0.6 fixture and require one
/// `coverage_unrecorded` for `child/lib.rs` with `reason`.
fn assert_coverage_unrecorded(project: &Project, reason: &str) {
    let state = project.state();
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    assert_eq!(project.state(), state, "the review wrote nothing");
    assert_eq!(
        file_changes(&manifest),
        vec![(
            "added".to_string(),
            "child/lib.rs".to_string(),
            "coverage_unrecorded".to_string(),
            Some(reason.to_string()),
        )]
    );
    assert_eq!(
        project.fallbacks("README.md"),
        vec![(
            "coverage_unrecorded".to_string(),
            "child/lib.rs".to_string()
        )]
    );
    let Json::Array(fallbacks) = get(&manifest, &["data", "review", "fallback_reasons"]) else {
        panic!()
    };
    assert_eq!(
        get_str(&fallbacks[0], &["message"]),
        format!(
            "`child/lib.rs` entered this document's scope. This document was last reviewed by a release that did not record handed-off folders, so Memoria cannot tell whether a handoff ended or the source is new (`{reason}`). Review the complete current scope. The next acknowledgement records the coverage."
        )
    );
    assert_eq!(
        get_str(&manifest, &["data", "review", "mode"]),
        "full_baseline"
    );
    assert!(get_bool(
        &manifest,
        &["data", "review", "whole_document_pass"]
    ));
    // The human view names the reason too.
    let human = stdout(&project.run(&["review", "README.md"]));
    assert!(
        human.contains(&format!(
            "child/lib.rs · entered the scope; former coverage not recorded ({reason})"
        )),
        "{human}"
    );
    assert_eq!(project.state(), state);
}

// §17.8 item 11: the root was acknowledged twice by 0.6, so the absent
// baseline of its first review is not the baseline of its latest one.
#[test]
fn a_twice_reviewed_0_6_root_reports_unrecorded_coverage() {
    let project = upgraded("unlinked-child-revision-2");
    assert_eq!(
        get_u64(
            &project.inspect_state(),
            &["reviews", "README.md", "revision"]
        ),
        2
    );
    assert_eq!(project.pending(), vec!["README.md"]);
    assert_coverage_unrecorded(&project, "revision_not_first");
    let counts = counting::review_proof_counts(&project.root, project.home.path(), "README.md");
    assert_eq!(counts, counting::ProofCounts::default(), "no proof can run");
}

// §17.8 item 12: the 0.6 review covered an invalidation, whose reason is no
// longer stored.
#[test]
fn a_0_6_review_that_covered_an_invalidation_reports_unrecorded_coverage() {
    let project = upgraded("unlinked-child-invalidated");
    let inspected = project.inspect_state();
    assert_eq!(
        get_u64(&inspected, &["reviews", "README.md", "revision"]),
        1
    );
    let Json::Array(ids) = get(
        &inspected,
        &["reviews", "README.md", "acknowledged_invalidations"],
    ) else {
        panic!()
    };
    assert_eq!(ids.len(), 1);
    assert_coverage_unrecorded(&project, "acknowledged_invalidations");
}

/// The added sources of the root, one per unlinked descendant.
fn descendant_sources(count: usize) -> Vec<String> {
    (1..=count).map(|i| format!("c{i:02}/lib.rs")).collect()
}

// §17.8 item 13: above twelve candidates the 0.6 layout is not tried, and the
// output says so; at twelve the proof stays within 4096 recomputations.
#[test]
fn the_legacy_proof_names_its_candidate_limit() {
    let project = upgraded("unlinked-descendants-13");
    let state = project.state();
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let changes = file_changes(&manifest);
    assert_eq!(
        changes.iter().map(|c| c.1.clone()).collect::<Vec<_>>(),
        descendant_sources(13)
    );
    assert!(
        changes
            .iter()
            .all(|c| c.2 == "coverage_unrecorded" && c.3.as_deref() == Some("candidate_limit")),
        "{changes:?}"
    );
    let reasons = project.fallbacks("README.md");
    assert_eq!(reasons.len(), 13, "{reasons:?}");
    assert!(
        reasons
            .iter()
            .all(|(code, _)| code == "coverage_unrecorded")
    );
    let counts = counting::review_proof_counts(&project.root, project.home.path(), "README.md");
    eprintln!("proof counts, 13 descendants: {counts:?}");
    assert_eq!(counts.v1_layout, 0, "the 0.6 layout is skipped: {counts:?}");
    assert!(counts.v2_layout() <= 4096, "{counts:?}");
    assert_eq!(project.state(), state);

    let project = upgraded("unlinked-descendants-12");
    let state = project.state();
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let changes = file_changes(&manifest);
    assert_eq!(changes.len(), 12);
    assert!(
        changes.iter().all(|c| c.2 == "handoff" && c.3.is_none()),
        "{changes:?}"
    );
    let mut handoffs: Vec<String> = project
        .fallbacks("README.md")
        .into_iter()
        .map(|(code, identity)| {
            assert_eq!(code, "handoff_changed");
            identity
        })
        .collect();
    handoffs.sort();
    assert_eq!(
        handoffs,
        (1..=12).map(|i| format!("c{i:02}")).collect::<Vec<_>>()
    );
    let counts = counting::review_proof_counts(&project.root, project.home.path(), "README.md");
    eprintln!("proof counts, 12 descendants: {counts:?}");
    assert!(
        counts.v1_layout >= 1 && counts.v1_layout <= 4096,
        "{counts:?}"
    );
    assert!(counts.v2_layout() <= 4096, "{counts:?}");
    assert!(counts.tokens <= 8192, "{counts:?}");
    assert_eq!(project.state(), state);
}

// §17.8 item 15: the first ordinary write re-encodes the whole lock as
// format 3. Only the acknowledged record changes; every other record keeps
// its stored values and is marked unrecorded.
#[test]
fn the_first_write_moves_the_lock_to_format_three() {
    let project = upgraded("unlinked");
    let before_bytes = project.state();
    let before = project.inspect_envelope();
    assert_eq!(get_u64(&before, &["data", "format_version"]), 2);
    project.ack_ok("README.md");
    let after = project.inspect_envelope();
    assert_eq!(get_u64(&after, &["data", "format_version"]), 3);
    assert_eq!(project.state()[4], 3, "the frame byte is 03");
    let reviews = |value: &Json| -> std::collections::BTreeMap<String, Json> {
        let Json::Object(map) = get(value, &["data", "state", "reviews"]) else {
            panic!()
        };
        map.clone()
    };
    let (old, new) = (reviews(&before), reviews(&after));
    assert_eq!(
        old.keys().collect::<Vec<_>>(),
        new.keys().collect::<Vec<_>>()
    );
    for (document, record) in &old {
        assert!(matches!(get(record, &["coverage_evidence"]), Json::Null));
        if document == "README.md" {
            continue;
        }
        assert_eq!(&new[document], record, "{document} is unchanged");
    }
    // The acknowledged root recorded that it hands nothing off: the unlinked
    // child is no handoff.
    assert!(strings(get(&new["README.md"], &["coverage_evidence"])).is_empty());
    assert!(matches!(
        get(&new["README.md"], &["coverage_evidence"]),
        Json::Array(_)
    ));
    assert_ne!(project.state(), before_bytes);
    // Reading the new lock rewrites nothing.
    let bytes = project.state();
    let (code, _) = project.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(project.state(), bytes);
}
