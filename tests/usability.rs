//! Approved usability interfaces, exercised through the actual CLI.
mod common;
use common::*;
use memoria_infrastructure::json::{self, Json, Limits};

fn evidence_for<'a>(result: &'a Json, identity: &str) -> &'a Json {
    let Json::Array(items) = get(result, &["data", "evidence"]) else {
        panic!()
    };
    items
        .iter()
        .find(|item| get_str(item, &["identity"]) == identity)
        .unwrap()
}

#[test]
fn explain_never_reviewed_has_explicit_document_baseline_without_history() {
    let project = Project::empty_repo();
    project.write("README.md", "# Fixture\n\nOwn the selected files.\n");
    project.write("source.txt", "initial\n");
    project.run(&["init", "--apply"]);
    let before = project.tree_snapshot();
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    assert_eq!(get(&result, &["data", "before_manifest"]), &Json::Null);
    assert_eq!(get(&result, &["data", "changes"]), &Json::Array(vec![]));
    let entry = evidence_for(&result, "README.md");
    assert_eq!(get_str(entry, &["reason_code"]), "no_previous_review");
    assert!(!get_bool(entry, &["baseline_verified"]));
    for field in [
        "base_commit",
        "expected_bytes",
        "expected_hash",
        "observed_bytes",
        "observed_hash",
        "text",
    ] {
        assert_eq!(get(entry, &[field]), &Json::Null);
    }
    assert!(stdout(&project.run(&["explain", "README.md"])).contains("no_previous_review"));
    let (_, packet) = project.json(&["review", "README.md"]);
    assert_eq!(
        get(&packet, &["data", "context", "diffs"]),
        &Json::Array(vec![])
    );
    assert_eq!(project.tree_snapshot(), before);
}

#[test]
fn explain_missing_git_baselines_keep_packet_reasons_and_null_observations() {
    for committed_readme in [false, true] {
        let project = Project::empty_repo();
        project.write("README.md", "# Fixture\n\nOwn the selected files.\n");
        if committed_readme {
            project.commit_all("README only");
        }
        // The reviewed source is absent from the stored commit, or there is no commit.
        project.write("source.txt", "reviewed bytes\n");
        project.baseline();
        let expected_code = if committed_readme {
            "blob_unavailable"
        } else {
            "no_base_commit"
        };
        for removed in [false, true] {
            if removed {
                project.remove("source.txt");
            } else {
                project.write("source.txt", "changed bytes\n");
            }
            let before = project.tree_snapshot();
            let (code, result) = project.json(&["explain", "README.md"]);
            assert_eq!(code, 0);
            let entry = evidence_for(&result, "source.txt");
            assert_eq!(get_str(entry, &["reason_code"]), expected_code);
            assert!(!get_bool(entry, &["baseline_verified"]));
            for field in ["observed_bytes", "observed_hash", "text"] {
                assert_eq!(get(entry, &[field]), &Json::Null);
            }
            assert_eq!(get_u64(entry, &["expected_bytes"]), 15);
            assert_eq!(
                get(entry, &["base_commit"]) == &Json::Null,
                !committed_readme
            );
            let human = project.run(&["explain", "README.md"]);
            assert!(stdout(&human).contains(expected_code));
            assert!(!stdout(&human).contains("removed_file"));
            let (_, packet) = project.json(&["review", "README.md"]);
            let Json::Array(diffs) = get(&packet, &["data", "context", "diffs"]) else {
                panic!()
            };
            let diff = diffs
                .iter()
                .find(|d| get_str(d, &["identity"]) == "source.txt")
                .unwrap();
            assert_eq!(get_str(diff, &["status"]), "unavailable");
            assert_eq!(get(diff, &["reason"]), get(entry, &["reason"]));
            assert_eq!(get(diff, &["old_body"]), &Json::Null);
            assert_eq!(get(diff, &["text"]), &Json::Null);
            assert_eq!(project.tree_snapshot(), before);
        }
    }
}

#[test]
fn explain_removed_input_does_not_borrow_another_boundarys_current_bytes() {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n\nOwn the selected files.\n");
    project.write("child/source.txt", "prior owner evidence\n");
    project.commit_all("ownership baseline");
    project.baseline();
    project.write("child/README.md", "# Child\n\nOwn the child files.\n");
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    let Json::Array(evidence) = get(&result, &["data", "evidence"]) else {
        panic!()
    };
    let file = evidence
        .iter()
        .find(|item| get_str(item, &["identity"]) == "child/source.txt")
        .unwrap();
    assert!(get_str(file, &["text"]).contains("-prior owner evidence"));
    assert_eq!(get_str(file, &["reason_code"]), "removed_file");
    assert!(get_str(file, &["reason"]).contains("packet emits no deletion hunk"));
    assert_eq!(
        project.read_string("child/source.txt"),
        "prior owner evidence\n"
    );
}

#[test]
fn existing_summary_error_json_contract_is_unchanged() {
    let project = Project::empty_repo();
    let (code, result) = project.json(&["status", "--summary", "--explain", "README.md"]);
    assert_eq!(code, 2);
    let expected = br#"{"command":"status","data":null,"diagnostics":[{"code":"summary_invalid","column":null,"details":{},"line":null,"message":"--summary emits bounded counts only; it cannot be combined with --explain","path":null,"severity":"error"}],"ok":false,"schema_version":2}"#;
    assert_eq!(result, json::parse(expected, Limits::STATE).unwrap());
}

#[test]
fn explain_import_changes_and_explicit_invalidations() {
    let project = Project::seed();
    project.baseline();
    let file = "src/execution/README.md";
    let body = project.read_string(file);
    project.write(
        file,
        body.replace(
            "<!-- /memoria:export -->",
            "A new exported sentence.\n<!-- /memoria:export -->",
        ),
    );
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    assert!(get_bool(&result, &["data", "waiting"]));
    let text = json::to_compact(&result);
    assert!(text.contains("import_content_not_stored"));
    assert!(stdout(&project.run(&["explain", "README.md"])).contains("import_content_not_stored"));
    assert!(text.contains("src/execution/README.md#summary"));
    project.run(&[
        "invalidate",
        "doc:README.md",
        "--reason",
        "Review the changed public contract.",
    ]);
    let (_, result) = project.json(&["explain", "README.md"]);
    assert!(
        json::to_compact(get(&result, &["data", "active_invalidations"]))
            .contains("Review the changed public contract.")
    );
}

#[test]
fn explain_add_remove_binary_and_line_limits() {
    let project = Project::empty_repo();
    project.write("README.md", "# Fixture\n\nOwn the selected files.\n");
    project.write("removed.txt", "old\n");
    project.write("binary.txt", b"\xff");
    project.write("lines.txt", "line\n".repeat(20_001));
    project.commit_all("evidence baseline");
    project.baseline();
    project.remove("removed.txt");
    project.write("added.txt", "new\n");
    project.write("binary.txt", b"\xfe");
    project.write("lines.txt", "line\n".repeat(20_002));
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    let rendered = json::to_compact(&result);
    for value in [
        "removed.txt",
        "added.txt",
        "binary_content",
        "diff_line_limit",
        "added_file",
    ] {
        assert!(rendered.contains(value), "{value}");
    }
    let Json::Array(evidence) = get(&result, &["data", "evidence"]) else {
        panic!()
    };
    let removed = evidence
        .iter()
        .find(|v| get_str(v, &["identity"]) == "removed.txt")
        .unwrap();
    assert!(get_bool(removed, &["baseline_verified"]));
    assert!(get_str(removed, &["text"]).contains("-old"));
    assert_eq!(get_str(removed, &["reason_code"]), "removed_file");
    assert!(get_str(removed, &["reason"]).contains("packet emits no deletion hunk"));
    let human = stdout(&project.run(&["explain", "README.md"]));
    for code in [
        "added_file",
        "removed_file",
        "binary_content",
        "diff_line_limit",
    ] {
        assert!(human.contains(code));
    }
    assert!(human.contains("packet emits no deletion hunk"));
    assert!(!get_bool(
        evidence_for(&result, "added.txt"),
        &["baseline_verified"]
    ));
    for identity in ["binary.txt", "lines.txt"] {
        assert!(get_bool(
            evidence_for(&result, identity),
            &["baseline_verified"]
        ));
    }
    let (_, packet) = project.json(&["review", "README.md"]);
    let Json::Array(diffs) = get(&packet, &["data", "context", "diffs"]) else {
        panic!()
    };
    for (identity, status) in [
        ("removed.txt", "removed"),
        ("added.txt", "added"),
        ("binary.txt", "binary"),
        ("lines.txt", "too_large"),
    ] {
        let entry = diffs
            .iter()
            .find(|d| get_str(d, &["identity"]) == identity)
            .unwrap();
        assert_eq!(get_str(entry, &["status"]), status);
        assert_eq!(get(entry, &["reason"]), &Json::Null);
        assert_eq!(get(entry, &["text"]), &Json::Null);
    }
}

#[test]
fn explain_json_survives_repository_relocation() {
    let project = Project::empty_repo();
    project.write("README.md", "# Fixture\n\nOwn the selected files.\n");
    project.commit_all("relocation baseline");
    project.baseline();
    project.write("added.txt", "new input\n");
    let first = project.run(&["explain", "README.md", "--format", "json"]);
    let destination = tempfile::tempdir().unwrap();
    for (path, (bytes, _)) in project.tree_snapshot() {
        let target = destination.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    let second = project.run_in(
        destination.path(),
        &["explain", "README.md", "--format", "json"],
    );
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(0), "{}", stdout(&second));
    assert_eq!(stdout(&first), stdout(&second));
}

#[cfg(unix)]
#[test]
fn non_utf8_default_is_rejected_but_explicit_label_wins() {
    use std::os::unix::ffi::OsStringExt as _;
    let project = Project::empty_repo();
    project.baseline();
    let args = [
        "ack",
        "README.md",
        "--packet",
        "/missing",
        "--token",
        "mrv2.0000000000000000",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ];
    let invalid = std::ffi::OsString::from_vec(vec![0xff]);
    let output = project
        .command(&project.root, &args)
        .env("MEMORIA_REVIEWER", &invalid)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("reviewer_invalid"));
    let output = project
        .command(&project.root, &args)
        .args(["--reviewer", "Explicit Reviewer"])
        .env("MEMORIA_REVIEWER", invalid)
        .output()
        .unwrap();
    assert!(stdout(&output).contains("packet_unreadable"));
    assert!(!stdout(&output).contains("reviewer_invalid"));
}

#[test]
fn state_diff_strict_snapshot_errors_and_frozen_records() {
    let project = Project::empty_repo();
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/state-v2");
    let old = fixtures.join("empty.lock");
    for name in ["tiny.lock", "current.lock", "mixed.lock"] {
        let next = fixtures.join(name);
        let output = project.run_in(
            project.packets.path(),
            &[
                "state",
                "diff",
                old.to_str().unwrap(),
                next.to_str().unwrap(),
                "--format",
                "json",
            ],
        );
        assert!(output.status.success(), "{}", stdout(&output));
        let value = json::parse(&output.stdout, Limits::STATE).unwrap();
        assert!(!get_bool(&value, &["data", "logical_equal"]));
        let reverse = project.run_in(
            project.packets.path(),
            &[
                "state",
                "diff",
                next.to_str().unwrap(),
                old.to_str().unwrap(),
                "--format",
                "json",
            ],
        );
        assert!(reverse.status.success());
    }
    let original = std::fs::read(&old).unwrap();
    for bytes in [
        b"{\"version\":1}".to_vec(),
        {
            let mut b = original.clone();
            b[4] = 99;
            b
        },
        {
            let mut b = original.clone();
            *b.last_mut().unwrap() ^= 1;
            b
        },
    ] {
        let invalid = project.packets.path().join("invalid.lock");
        std::fs::write(&invalid, bytes).unwrap();
        let output = project.run_in(
            project.packets.path(),
            &[
                "state",
                "diff",
                old.to_str().unwrap(),
                "invalid.lock",
                "--format",
                "json",
            ],
        );
        assert_eq!(output.status.code(), Some(4));
        let value = json::parse(&output.stdout, Limits::STATE).unwrap();
        assert_eq!(get(&value, &["data"]), &Json::Null);
        assert!(stdout(&output).contains("invalid.lock"));
    }
}

#[test]
fn human_plan_carries_existing_guidance_and_render_order() {
    let project = Project::seed();
    project.run(&["init", "--apply"]);
    let (_, plan) = project.json(&["review"]);
    let human = project.run(&["review"]);
    assert!(stdout(&human).contains(get_str(&plan, &["data", "guidance_first"])));
    let next = get_str(&plan, &["data", "next_ready"]);
    assert!(stdout(&human).contains(&format!("Guidance: memoria guidance {next}")));
    let kind = get_str(&plan, &["data", "next_action", "kind"]);
    assert!(stdout(&human).contains(&format!("Next: memoria {kind} {next}")));
    project.baseline();
    let human = project.run(&["review"]);
    assert!(stdout(&human).contains("No README needs review."));
    assert!(!stdout(&human).contains("Guidance:"));
}

#[test]
fn completions_work_without_project_and_json_contains_exact_script() {
    let dir = tempfile::tempdir().unwrap();
    for shell in ["bash", "zsh", "fish"] {
        let run = |args: &[&str]| {
            std::process::Command::new(memoria_bin())
                .current_dir(dir.path())
                .args(args)
                .output()
                .unwrap()
        };
        let human = run(&["completions", shell]);
        assert!(human.status.success());
        assert!(human.stderr.is_empty());
        let result = run(&["completions", shell, "--format", "json"]);
        let value = json::parse(&result.stdout, Limits::STATE).unwrap();
        assert_eq!(
            get_str(&value, &["data", "script"]).as_bytes(),
            human.stdout
        );
        for word in ["explain", "diff", "agent", "format", "root"] {
            assert!(stdout(&human).contains(word), "{shell}: {word}");
        }
        if shell == "bash" {
            let script = dir.path().join("completion.bash");
            std::fs::write(&script, &human.stdout).unwrap();
            assert!(
                std::process::Command::new("bash")
                    .arg("-n")
                    .arg(script)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
}

#[test]
fn reviewer_default_override_and_validation_precede_packet_reads() {
    let project = Project::empty_repo();
    project.baseline();
    let lock = project.state();
    let args = [
        "ack",
        "README.md",
        "--packet",
        "/missing/packet",
        "--token",
        "mrv2.0000000000000000",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ];
    for env in [None, Some(""), Some("   ")] {
        let mut command = project.command(&project.root, &args);
        command.env_remove("MEMORIA_REVIEWER");
        if let Some(env) = env {
            command.env("MEMORIA_REVIEWER", env);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(stdout(&output).contains("reviewer_required"));
    }
    for label in ["", "bad\tlabel", "bad\nlabel"] {
        let output = project
            .command(&project.root, &args)
            .args(["--reviewer", label])
            .env("MEMORIA_REVIEWER", "default")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(stdout(&output).contains("reviewer_invalid"));
    }
    assert_eq!(project.state(), lock);
    for (explicit, expected) in [
        (None, "Default Reviewer"),
        (Some(" Explicit Reviewer "), "Explicit Reviewer"),
    ] {
        project.append("README.md", "\nChanged review input.\n");
        let (packet, token) = project.review_packet("README.md");
        let mut command = project.command(
            &project.root,
            &[
                "ack",
                "README.md",
                "--packet",
                packet.to_str().unwrap(),
                "--token",
                &token,
                "--result",
                "no-update",
                "--note",
                NOTE,
            ],
        );
        command.env("MEMORIA_REVIEWER", " Default Reviewer ");
        if let Some(label) = explicit {
            command.args(["--reviewer", label]);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains(expected));
        assert!(project.state_text().contains(expected));
    }
}

#[test]
fn explain_is_deterministic_read_only_and_matches_packet_hunks() {
    let project = Project::empty_repo();
    project.write("README.md", "# Example\n\nThis project owns a file.\n");
    project.write("source.txt", "before\n");
    project.commit_all("baseline source");
    project.baseline();
    assert_eq!(project.json(&["explain", "README.md"]).0, 0);
    project.write("source.txt", "after\n");
    let before = project.tree_snapshot();
    let first = project.run(&["explain", "README.md", "--format", "json"]);
    let second = project.run(&["explain", "README.md", "--format", "json"]);
    assert!(first.status.success(), "{}", stdout(&first));
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(project.tree_snapshot(), before);
    let explain = json::parse(&first.stdout, Limits::STATE).unwrap();
    let Json::Array(evidence) = get(&explain, &["data", "evidence"]) else {
        panic!()
    };
    let file = evidence
        .iter()
        .find(|item| get_str(item, &["identity"]) == "source.txt")
        .unwrap();
    assert!(get_bool(file, &["baseline_verified"]));
    let (_, packet) = project.json(&["review", "README.md"]);
    let Json::Array(diffs) = get(&packet, &["data", "context", "diffs"]) else {
        panic!()
    };
    let diff = diffs
        .iter()
        .find(|item| get_str(item, &["identity"]) == "source.txt")
        .unwrap();
    assert_eq!(get(file, &["text"]), get(diff, &["text"]));
    project.ack_ok("README.md"); // Save uncommitted bytes in the disposable fixture.
    project.write("source.txt", "third\n");
    let (_, explain) = project.json(&["explain", "README.md"]);
    assert!(json::to_compact(&explain).contains("reviewed_bytes_mismatch"));
    assert_eq!(project.json(&["explain", "missing/README.md"]).0, 1);
    assert_eq!(project.json(&["explain", "../README.md"]).0, 2);
}

#[test]
fn explain_waiting_never_reviewed_guidance_and_policy() {
    let project = Project::seed();
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    assert_eq!(get(&result, &["data", "before_manifest"]), &Json::Null);
    assert!(get_bool(&result, &["data", "waiting"]));
    project.baseline();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "Use short sentences.",
            "Review the public interface carefully.",
        ),
    );
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0, "{}", json::to_pretty(&result));
    assert_eq!(get_str(&result, &["data", "status"]), "current");
    assert!(get_bool(&result, &["data", "guidance", "changed"]));
    project.write(".gitignore", "*.ignored\n");
    let (code, result) = project.json(&["explain", "README.md"]);
    assert_eq!(code, 0);
    assert!(get_bool(&result, &["data", "policy", "changed"]));
}

#[test]
fn state_comparison_tracks_identities_and_does_not_write() {
    let project = Project::empty_repo();
    project.baseline();
    let before_path = project.packets.path().join("before.lock");
    std::fs::write(&before_path, project.state()).unwrap();
    project.write("new-file.txt", "new input\n");
    let (packet, token) = project.review_packet("README.md");
    let note = "The new input matches the documented boundary.\nUnicode evidence: 漢字.";
    assert!(
        project
            .ack("README.md", &packet, &token, "no-update", note)
            .status
            .success()
    );
    let after_path = project.packets.path().join("after.lock");
    std::fs::write(&after_path, project.state()).unwrap();
    let before = project.tree_snapshot();
    let output = project.run_in(
        project.packets.path(),
        &[
            "state",
            "diff",
            "before.lock",
            "after.lock",
            "--root",
            "/nonexistent",
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stdout(&output));
    let value = json::parse(&output.stdout, Limits::STATE).unwrap();
    assert!(!get_bool(&value, &["data", "logical_equal"]));
    let human = project.run_in(
        project.packets.path(),
        &["state", "diff", "before.lock", "after.lock"],
    );
    assert!(stdout(&human).contains(NOTE));
    assert!(stdout(&human).contains(note));
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    assert!(changes.iter().any(|change| strings(get(change, &["path"]))
        == [
            "reviews",
            "README.md",
            "input_manifest",
            "files",
            "new-file.txt"
        ]));
    let same = project.run_in(
        project.packets.path(),
        &[
            "state",
            "diff",
            "before.lock",
            "before.lock",
            "--format",
            "json",
        ],
    );
    let same = json::parse(&same.stdout, Limits::STATE).unwrap();
    assert!(get_bool(&same, &["data", "byte_equal"]));
    assert_eq!(project.tree_snapshot(), before);
    let missing = project.run_in(
        project.packets.path(),
        &[
            "state",
            "diff",
            "before.lock",
            "missing.lock",
            "--format",
            "json",
        ],
    );
    assert_eq!(missing.status.code(), Some(4));
    assert!(stdout(&missing).contains("missing.lock"));
}

#[test]
fn state_diff_distinguishes_frame_changes_from_logical_changes() {
    use memoria_infrastructure::lock_codec;
    let dir = tempfile::tempdir().unwrap();
    let compressed = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/state-v2/current.lock"),
    )
    .unwrap();
    let decoded = lock_codec::decode(&compressed).unwrap();
    let payload = lock_codec::encode_payload(&decoded.state).unwrap();
    let mut raw = lock_codec::MAGIC.to_vec();
    raw.extend([lock_codec::FORMAT_VERSION, lock_codec::CODEC_RAW]);
    let mut length = payload.len() as u64;
    loop {
        let byte = (length & 0x7f) as u8;
        length >>= 7;
        raw.push(if length == 0 { byte } else { byte | 0x80 });
        if length == 0 {
            break;
        }
    }
    raw.extend(payload);
    raw.extend(memoria_infrastructure::hash::xxh3_128(&raw));
    std::fs::write(dir.path().join("raw.lock"), &raw).unwrap();
    std::fs::write(dir.path().join("compressed.lock"), &compressed).unwrap();
    let output = std::process::Command::new(memoria_bin())
        .current_dir(dir.path())
        .args([
            "state",
            "diff",
            "raw.lock",
            "compressed.lock",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stdout(&output));
    let result = json::parse(&output.stdout, Limits::STATE).unwrap();
    assert!(get_bool(&result, &["data", "logical_equal"]));
    assert!(!get_bool(&result, &["data", "byte_equal"]));
    assert_eq!(std::fs::read(dir.path().join("raw.lock")).unwrap(), raw);
    assert_eq!(
        std::fs::read(dir.path().join("compressed.lock")).unwrap(),
        compressed
    );
}
