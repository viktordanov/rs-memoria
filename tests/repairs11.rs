//! Regression tests for the round-11 triage findings (MEM-035 reopened,
//! MEM-044).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use common::*;
use memoria_infrastructure::json::Json;

const BOM: &[u8] = b"\xef\xbb\xbf";

fn policy_hash(packet: &Path) -> String {
    get_str(
        &parse_json(&fs::read(packet).unwrap()),
        &["data", "manifest", "policy_hash"],
    )
    .to_string()
}

fn with_bom(text: &str) -> Vec<u8> {
    let mut bytes = BOM.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

// MEM-035 (reopened)
#[test]
fn bom_prefixed_ignore_comments_have_no_policy_meaning() {
    let project = Project::seed();
    let external = tempfile::tempdir().unwrap();
    let global = external.path().join("global-ignore");
    fs::write(&global, with_bom("# global comment\n")).unwrap();
    project.git(&["config", "core.excludesFile", global.to_str().unwrap()]);
    project.baseline();
    let before = project.state();
    let rules = project.read_string(".gitignore");
    // Add, edit, and remove a BOM-prefixed first-line comment in every
    // ignore source: never review work.
    for (label, content) in [
        ("add", with_bom(&format!("# first comment\n{rules}"))),
        ("edit", with_bom(&format!("# second comment\n{rules}"))),
        (
            "remove bom",
            format!("# second comment\n{rules}").into_bytes(),
        ),
        ("remove comment", rules.clone().into_bytes()),
    ] {
        project.write(".gitignore", &content);
        assert_eq!(project.json(&["check"]).0, 0, ".gitignore {label}");
    }
    for (label, content) in [
        ("add", with_bom("# repository comment\n")),
        ("edit", with_bom("# repository comment edited\n")),
        ("remove", b"# repository comment edited\n".to_vec()),
    ] {
        fs::write(project.root.join(".git/info/exclude"), &content).unwrap();
        assert_eq!(project.json(&["check"]).0, 0, "repository excludes {label}");
    }
    for (label, content) in [
        ("edit", with_bom("# global comment edited\n")),
        ("remove", b"# global comment edited\n".to_vec()),
    ] {
        fs::write(&global, &content).unwrap();
        assert_eq!(project.json(&["check"]).0, 0, "global excludes {label}");
    }
    assert_eq!(project.state(), before, "no state was written");
    // Git agrees that only the rule lines matter.
    project.write(".gitignore", with_bom(&format!("# c\n{rules}")));
    let ignored = project.git(&[
        "check-ignore",
        "--no-index",
        "ignored-output/x.bin",
        "app.rs",
    ]);
    assert_eq!(stdout(&ignored).trim(), "ignored-output/x.bin");
    // A real rule behind a BOM is a policy change with the same identity as
    // the plain spelling, and a BOM on a later line is pattern content.
    project.write(".gitignore", &rules);
    assert_eq!(project.json(&["check"]).0, 0);
    project.write(".gitignore", with_bom(&format!("*.log\n{rules}")));
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    let (bom_packet, _) = project.review_packet("src/execution/README.md");
    project.write(".gitignore", format!("*.log\n{rules}"));
    let (plain_packet, _) = project.review_packet("src/execution/README.md");
    assert_eq!(policy_hash(&bom_packet), policy_hash(&plain_packet));
    project.write(".gitignore", &rules);
    assert_eq!(project.json(&["check"]).0, 0);
    let mut embedded = format!("# c\n{rules}").into_bytes();
    embedded.extend_from_slice(BOM);
    embedded.extend_from_slice(b"# git treats this as a pattern\n");
    project.write(".gitignore", &embedded);
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"],
        "an embedded BOM line is a rule, exactly as in Git"
    );
    assert_eq!(project.state(), before);
}

fn removed_document_changes(value: &Json) -> Vec<(String, String, String)> {
    let Json::Array(diagnostics) = get(value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!("{}", memoria_infrastructure::json::to_compact(value))
    };
    changes
        .iter()
        .map(|c| {
            (
                get_str(c, &["kind"]).to_string(),
                get_str(c, &["change"]).to_string(),
                get_str(c, &["identity"]).to_string(),
            )
        })
        .collect()
}

// MEM-044
#[test]
fn deleting_the_packet_document_is_a_snapshot_conflict() {
    let project = Project::seed();
    project.baseline();
    let document = "src/disconnected/README.md";
    assert_eq!(
        project
            .json(&[
                "invalidate",
                &format!("doc:{document}"),
                "--reason",
                "Review current explanations for accuracy."
            ])
            .0,
        0
    );
    let (packet, token) = project.review_packet(document);
    let before = project.state();
    project.remove(document);
    // JSON: the conflict class with the removed document and its inputs.
    let output = project.run(&[
        "ack",
        document,
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
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert_eq!(get_str(&diagnostics[0], &["path"]), document);
    assert!(get_str(&diagnostics[0], &["message"]).contains("removed document README"));
    assert_eq!(
        removed_document_changes(&value),
        vec![
            ("document".into(), "removed".into(), "README".into()),
            (
                "file".into(),
                "removed".into(),
                "src/disconnected/item.rs".into()
            ),
        ]
    );
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    assert!(get_u64(&changes[0], &["before_bytes"]) > 0);
    assert!(matches!(get(&changes[0], &["after_bytes"]), Json::Null));
    assert!(matches!(get(&changes[0], &["after_hash"]), Json::Null));
    assert!(
        get_str(&changes[0], &["diff"]).contains("-# Disconnected"),
        "{}",
        get_str(&changes[0], &["diff"])
    );
    assert_eq!(project.state(), before, "state and invalidations preserved");
    // Human output keeps the same class and evidence.
    let output = project.ack(document, &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(3));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("snapshot_changed"), "{text}");
    assert!(text.contains("removed document README"), "{text}");
    assert!(text.contains("src/disconnected/item.rs"), "{text}");
    assert_eq!(project.state(), before);
    // The document is still absent from status, and the project stays valid.
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0);
    let Json::Array(documents) = get(&status, &["data", "documents"]) else {
        panic!()
    };
    assert!(
        documents
            .iter()
            .all(|d| get_str(d, &["document"]) != document)
    );
    // Malformed requests and invalid projects keep their own classes.
    let output = project.ack("missing/README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    project.remove("README.md");
    let output = project.ack(document, &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(project.state(), before);
}

// MEM-044: disappearance during the final revalidation.
#[test]
fn document_deleted_during_final_revalidation_is_a_snapshot_conflict() {
    let project = Project::seed();
    project.baseline();
    let document = "src/disconnected/README.md";
    project.append("src/disconnected/item.rs", "// pending\n");
    let (packet, token) = project.review_packet(document);
    let before = project.state();
    let shim_dir = tempfile::tempdir().unwrap();
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    let target = project.root.join(document);
    let sentinel = shim_dir.path().join("triggered");
    // The first snapshot has already discovered and read the document when
    // it asks Git for worktree state; the document disappears then, so only
    // the final revalidation can see it.
    fs::write(
        shim_dir.path().join("git"),
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" status \"*) if [ ! -e \"{s}\" ]; then touch \"{s}\"; rm -f \"{t}\"; fi ;; esac\nexec {git} \"$@\"\n",
            s = sentinel.display(),
            t = target.display(),
            git = real.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(
        shim_dir.path().join("git"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        shim_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(memoria_bin())
        .current_dir(&project.root)
        .env("PATH", &path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "ack",
            document,
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
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(sentinel.exists(), "the shim removed the document");
    assert!(!target.exists());
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    assert_eq!(
        removed_document_changes(&value)[0],
        ("document".into(), "removed".into(), "README".into())
    );
    assert_eq!(project.state(), before, "nothing was written");
}
