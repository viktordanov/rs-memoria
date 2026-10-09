//! `memoria review <DOCUMENT> --save <DIR>` and `memoria ack --packet` with
//! the token derived from the saved artifact (plan §8).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::*;
use memoria_infrastructure::json::Json;

const DOC: &str = "src/corpus/README.md";

fn pending_project() -> Project {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    project.append("src/corpus/types.rs", "// pending change\n");
    project
}

fn save(project: &Project, dir: &Path, extra: &[&str]) -> (i32, Json) {
    let mut args = vec!["review", DOC, "--save", dir.to_str().unwrap()];
    args.extend_from_slice(extra);
    project.json(&args)
}

fn saved_path(receipt: &Json) -> PathBuf {
    PathBuf::from(get_str(receipt, &["data", "path"]))
}

fn ack_saved(project: &Project, packet: &Path, token: Option<&str>) -> (i32, Json) {
    let mut args = vec![
        "ack",
        DOC,
        "--packet",
        packet.to_str().unwrap(),
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
    ];
    if let Some(token) = token {
        args.push("--token");
        args.push(token);
    }
    project.json(&args)
}

#[test]
fn saved_bytes_equal_stdout_and_ack_needs_no_token() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let stdout_bytes = project.run(&["review", DOC, "--format", "json"]).stdout;
    let tree = project.tree_snapshot();
    let (code, receipt) = save(&project, outside.path(), &[]);
    // Review writes nothing to the project.
    assert_eq!(project.tree_snapshot(), tree);
    assert_eq!(code, 0, "{receipt:?}");
    assert_eq!(
        get_str(&receipt, &["data", "kind"]),
        "saved_review_artifact"
    );
    assert_eq!(get_str(&receipt, &["data", "artifact_kind"]), "manifest");
    assert_eq!(get_str(&receipt, &["data", "document"]), DOC);
    assert_eq!(get_str(&receipt, &["data", "document_kind"]), "readme");
    let path = saved_path(&receipt);
    assert!(path.starts_with(outside.path().canonicalize().unwrap()));
    let token = get_str(&receipt, &["data", "token"]).to_string();
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        format!("memoria-manifest-src_corpus_README.md-{}.json", &token[5..])
    );
    // The saved bytes are exactly the stdout bytes for the same snapshot.
    assert_eq!(fs::read(&path).unwrap(), stdout_bytes);
    assert_eq!(
        get_u64(&receipt, &["data", "bytes"]),
        stdout_bytes.len() as u64
    );
    let artifact = parse_json(&stdout_bytes);
    assert_eq!(
        get_str(&receipt, &["data", "artifact_digest"]),
        get_str(&artifact, &["data", "artifact_digest"])
    );
    assert!(get_str(&receipt, &["data", "ack_command"]).contains("--packet"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // A second save of the same snapshot never clobbers: it takes a suffix.
    let (code, again) = save(&project, outside.path(), &[]);
    assert_eq!(code, 0);
    assert!(get_str(&again, &["data", "path"]).ends_with("-2.json"));
    assert_eq!(fs::read(&path).unwrap(), stdout_bytes);
    let (code, acked) = ack_saved(&project, &path, None);
    assert_eq!(code, 0, "{acked:?}");
    assert_eq!(get_str(&acked, &["data", "token_source"]), "artifact");
    assert_eq!(project.status_label(DOC), "current");
}

#[test]
fn the_human_view_names_the_saved_file() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let output = project.run(&["review", DOC, "--save", outside.path().to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.starts_with(&format!("Review {DOC} — README, pending since revision")),
        "{text}"
    );
    assert!(text.contains("\nSaved: "), "{text}");
    assert!(
        text.contains("memoria ack src/corpus/README.md --packet"),
        "{text}"
    );
}

#[test]
fn a_full_export_saves_and_acknowledges_too() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (code, receipt) = save(&project, outside.path(), &["--full"]);
    assert_eq!(code, 0, "{receipt:?}");
    assert_eq!(get_str(&receipt, &["data", "artifact_kind"]), "full");
    let path = saved_path(&receipt);
    assert!(
        path.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("memoria-full-")
    );
    let saved = parse_json(&fs::read(&path).unwrap());
    assert_eq!(get_u64(&saved, &["data", "packet_version"]), 5);
    assert_eq!(
        get_str(&receipt, &["data", "artifact_digest"]),
        get_str(&saved, &["data", "packet_digest"])
    );
    let (code, acked) = ack_saved(&project, &path, None);
    assert_eq!(code, 0, "{acked:?}");
}

#[test]
fn destinations_inside_the_worktree_are_refused() {
    let project = pending_project();
    fs::create_dir_all(project.root.join("ignored-out")).unwrap();
    project.write(".gitignore", "ignored-out/\n");
    fs::create_dir_all(project.root.join("src/sub")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut destinations: Vec<PathBuf> = vec![
        project.root.clone(),
        project.root.join("."),
        project.root.join("src/sub"),
        project.root.join("ignored-out"),
        project.root.join(".git"),
    ];
    #[cfg(unix)]
    {
        let link = outside.path().join("into-project");
        std::os::unix::fs::symlink(&project.root, &link).unwrap();
        destinations.push(link);
    }
    let before = project.tree_snapshot();
    for destination in destinations {
        let (code, value) = save(&project, &destination, &[]);
        assert_eq!(code, 2, "{destination:?}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["save_destination_in_project"],
            "{destination:?}"
        );
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        assert!(get_str(&diagnostics[0], &["message"]).contains("mktemp -d"));
    }
    assert_eq!(project.tree_snapshot(), before, "nothing was written");
    // A relative destination resolves against the invocation directory.
    let output = project.run_in(
        &project.root.join("src"),
        &["review", DOC, "--save", "sub", "--format", "json"],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn missing_file_and_unwritable_destinations() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (code, value) = save(&project, &outside.path().join("absent"), &[]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["save_destination_invalid"]);
    let file = outside.path().join("a-file");
    fs::write(&file, "x").unwrap();
    let (code, value) = save(&project, &file, &[]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["save_destination_invalid"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let readonly = outside.path().join("readonly");
        fs::create_dir(&readonly).unwrap();
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores directory permissions; the case is meaningless there.
        let writable = fs::write(readonly.join("probe"), "x").is_ok();
        if !writable {
            let (code, value) = save(&project, &readonly, &[]);
            assert_eq!(code, 4, "{value:?}");
            assert_eq!(diagnostic_codes(&value), vec!["save_failed"]);
        }
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_final_name_is_never_followed() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (_, receipt) = save(&project, outside.path(), &[]);
    let path = saved_path(&receipt);
    let name = path.file_name().unwrap().to_owned();
    fs::remove_file(&path).unwrap();
    let target = outside.path().join("victim.txt");
    std::os::unix::fs::symlink(&target, outside.path().join(&name)).unwrap();
    let (code, receipt) = save(&project, outside.path(), &[]);
    assert_eq!(code, 0);
    assert!(get_str(&receipt, &["data", "path"]).ends_with("-2.json"));
    assert!(!target.exists(), "the symlink target was never written");
}

#[test]
fn collisions_are_bounded() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (_, receipt) = save(&project, outside.path(), &[]);
    let first = saved_path(&receipt);
    let stem = first
        .to_str()
        .unwrap()
        .trim_end_matches(".json")
        .to_string();
    for n in 2..=99 {
        fs::write(format!("{stem}-{n}.json"), "taken").unwrap();
    }
    let (code, value) = save(&project, outside.path(), &[]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["save_name_exhausted"]);
}

#[test]
fn a_limit_refusal_writes_nothing() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (code, value) = save(&project, outside.path(), &["--max-bytes", "1"]);
    assert_ne!(code, 0, "{value:?}");
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn save_requires_one_document() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (code, value) = project.json(&["review", "--save", outside.path().to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["save_requires_document"]);
}

#[test]
fn ack_refusals_leave_state_unchanged() {
    let project = pending_project();
    let outside = tempfile::tempdir().unwrap();
    let (_, receipt) = save(&project, outside.path(), &[]);
    let path = saved_path(&receipt);
    let token = get_str(&receipt, &["data", "token"]).to_string();
    let before = project.state();

    // A token that does not match the artifact.
    let forged = format!("mrv3.{}", "0".repeat(16));
    let (code, value) = ack_saved(&project, &path, Some(&forged));
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["token_mismatch"]);

    // A tampered artifact fails its integrity check.
    let mut tampered = parse_json(&fs::read(&path).unwrap());
    set_path(
        &mut tampered,
        &["data", "token"],
        Json::String(forged.clone()),
    );
    let tampered_path = outside.path().join("tampered.json");
    fs::write(
        &tampered_path,
        memoria_infrastructure::json::to_pretty(&tampered),
    )
    .unwrap();
    let (code, value) = ack_saved(&project, &tampered_path, None);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["packet_integrity_failed"]);

    // The receipt is not an artifact.
    let receipt_path = outside.path().join("receipt.json");
    fs::write(
        &receipt_path,
        memoria_infrastructure::json::to_pretty(&receipt),
    )
    .unwrap();
    let (code, value) = ack_saved(&project, &receipt_path, None);
    assert_eq!(code, 2, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["packet_schema_invalid"]);

    // A different document.
    let (code, value) = project.json(&[
        "ack",
        "README.md",
        "--packet",
        path.to_str().unwrap(),
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
    ]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["packet_document_mismatch"]);

    // A source that changed after the save.
    project.append("src/corpus/types.rs", "// after the save\n");
    let (code, value) = ack_saved(&project, &path, None);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    assert_eq!(project.state(), before);

    // Guidance that changed after the save is a conflict too.
    let (_, receipt) = save(&project, outside.path(), &[]);
    let fresh = saved_path(&receipt);
    project.write(
        "src/corpus/README.memoria.toml",
        "[documentation]\nguidance = [\"Explain every public type.\"]\n",
    );
    let (code, value) = ack_saved(&project, &fresh, None);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(project.state(), before);

    // An explicit matching token still works, from stdin.
    project.remove("src/corpus/README.memoria.toml");
    let (_, receipt) = save(&project, outside.path(), &[]);
    let bytes = fs::read(saved_path(&receipt)).unwrap();
    let explicit = get_str(&receipt, &["data", "token"]).to_string();
    let output = project.run_stdin(
        &[
            "ack",
            DOC,
            "--packet",
            "-",
            "--token",
            &explicit,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(
        get_str(&parse_json(&output.stdout), &["data", "token_source"]),
        "argument"
    );
    let _ = token;
}

#[test]
fn an_edit_to_any_section_guide_after_saving_refuses_acknowledgement() {
    for guide in [
        // The guide of the suggested section.
        "docs/templates/agent-commands.md",
        // The guide of a section that the review does not suggest.
        "docs/templates/agent-rules.md",
    ] {
        let project = Project::agent_instructions();
        project.append("justfile", "\nlint:\n    cargo clippy\n");
        let (packet, token) = project.review_packet("AGENTS.md");
        let state = project.state();
        project.append(guide, "One more rule.\n");
        let (code, value) = project.ack_json("AGENTS.md", &packet, &token, "no-update", NOTE);
        assert_eq!(code, 3, "{guide}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["guidance_changed"],
            "{guide}"
        );
        assert_eq!(project.state(), state, "{guide}: no state write");
        // A fresh artifact acknowledges.
        let (packet, token) = project.review_packet("AGENTS.md");
        let output = project.ack("AGENTS.md", &packet, &token, "no-update", NOTE);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    }
}
