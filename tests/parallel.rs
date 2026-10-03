//! Independent reviews in one checkout and across linked worktrees.
//!
//! Every case runs real Memoria processes. Reviewers capture their artifacts
//! before anyone acknowledges, as parallel reviewers do. The cases check that
//! unrelated work never forces a fresh capture, and that a real dependency
//! change still refuses an older artifact.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::*;
use memoria_infrastructure::json::Json;

const NOTE: &str = "The prose matches the current sources in this folder.";

/// A root README that links `count` folders, each with one tracked README
/// and one source file.
fn disjoint(count: usize) -> (Project, Vec<String>) {
    let project = Project::empty_repo();
    let documents: Vec<String> = (1..=count).map(|i| format!("d{i:02}/README.md")).collect();
    let links: Vec<String> = documents.iter().map(|d| format!("- [{d}]({d})")).collect();
    project.write(
        "README.md",
        format!("# Root\n\nThe folders:\n\n{}\n", links.join("\n")),
    );
    for (i, document) in documents.iter().enumerate() {
        let dir = document.trim_end_matches("/README.md");
        project.write(document, format!("# Folder {i}\n\nIt owns its source.\n"));
        project.write(&format!("{dir}/lib.rs"), format!("fn f{i}() {{}}\n"));
    }
    project.commit_all("fixture");
    project.baseline();
    project.commit_all("baseline");
    (project, documents)
}

/// Root links `a/` and `b/`. `a` imports `b#summary`. `c/` is not handed
/// off, so the root and `c` both cover `c/sub/x.rs`.
fn shared() -> Project {
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\nSee [A](a/README.md) and [B](b/README.md).\n",
    );
    project.write(
        "a/README.md",
        "# A\n\n<!-- memoria:import src=\"../b/README.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    project.write("a/lib.rs", "fn a() {}\n");
    project.write(
        "b/README.md",
        "# B\n\n<!-- memoria:export id=\"summary\" -->\nB summary one.\n<!-- /memoria:export -->\n",
    );
    project.write("b/lib.rs", "fn b() {}\n");
    project.write("c/README.md", "# C\n\nC covers its folder too.\n");
    project.write("c/sub/x.rs", "fn x() {}\n");
    project.commit_all("fixture");
    project.baseline();
    project.commit_all("baseline");
    project
}

/// Save one document's artifact outside the worktree and return its path.
fn capture(project: &Project, cwd: &Path, document: &str) -> PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("artifact-")
        .tempdir_in(project.packets.path())
        .unwrap()
        .keep();
    let output = project
        .command(
            cwd,
            &[
                "review",
                document,
                "--save",
                dir.to_str().unwrap(),
                "--format",
                "json",
            ],
        )
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "capture {document}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value = parse_json(&output.stdout);
    PathBuf::from(get_str(&value, &["data", "path"]))
}

fn ack_args<'a>(document: &'a str, packet: &'a Path, reviewer: &'a str) -> Vec<&'a str> {
    vec![
        "ack",
        document,
        "--packet",
        packet.to_str().unwrap(),
        "--reviewer",
        reviewer,
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ]
}

fn ack_in(project: &Project, cwd: &Path, document: &str, packet: &Path) -> (i32, Vec<String>) {
    let output = project
        .command(cwd, &ack_args(document, packet, "fixture"))
        .output()
        .unwrap();
    outcome(&output)
}

fn outcome(output: &Output) -> (i32, Vec<String>) {
    let value = parse_json(&output.stdout);
    let errors = match get(&value, &["diagnostics"]) {
        Json::Array(items) => items
            .iter()
            .filter(|d| get_str(d, &["severity"]) == "error")
            .map(|d| get_str(d, &["code"]).to_string())
            .collect(),
        _ => Vec::new(),
    };
    (output.status.code().unwrap(), errors)
}

fn reviewer_of(project: &Project, document: &str) -> String {
    let state = project.inspect_state();
    get_str(&state, &["reviews", document, "reviewer"]).to_string()
}

fn status_of(project: &Project, document: &str) -> String {
    get_str(&project.doc_status(document), &["status"]).to_string()
}

/// Capture every document first, then acknowledge them all at once.
fn simultaneous(count: usize) {
    let (project, documents) = disjoint(count);
    project.run(&[
        "invalidate",
        "all",
        "--reason",
        "Parallel reviewers check every folder.",
    ]);
    let packets: Vec<PathBuf> = documents
        .iter()
        .map(|document| capture(&project, &project.root, document))
        .collect();
    let reviewers: Vec<String> = (1..=count).map(|i| format!("worker-{i}")).collect();
    let children: Vec<_> = documents
        .iter()
        .zip(&packets)
        .zip(&reviewers)
        .map(|((document, packet), reviewer)| {
            project
                .command(&project.root, &ack_args(document, packet, reviewer))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for (child, document) in children.into_iter().zip(&documents) {
        let output = child.wait_with_output().unwrap();
        assert_eq!(outcome(&output), (0, vec![]), "{document}");
    }
    // No acknowledgement was lost: every record names its own reviewer.
    for (document, reviewer) in documents.iter().zip(&reviewers) {
        assert_eq!(&reviewer_of(&project, document), reviewer);
        assert_eq!(status_of(&project, document), "current");
    }
    // Only the root is still pending for the invalidation.
    assert_eq!(status_of(&project, "README.md"), "pending");
}

#[test]
fn two_reviewers_capture_first_and_acknowledge_at_once() {
    simultaneous(2);
}

#[test]
fn nine_reviewers_capture_first_and_acknowledge_at_once() {
    simultaneous(9);
}

#[test]
fn an_unchanged_shared_source_allows_independent_reviews() {
    let project = shared();
    project.run(&[
        "invalidate",
        "all",
        "--reason",
        "Shared source independence check.",
    ]);
    let root = capture(&project, &project.root, "README.md");
    let c = capture(&project, &project.root, "c/README.md");
    assert_eq!(
        ack_in(&project, &project.root, "c/README.md", &c),
        (0, vec![])
    );
    assert_eq!(
        ack_in(&project, &project.root, "README.md", &root),
        (0, vec![])
    );
}

#[test]
fn a_changed_shared_source_refuses_every_artifact_that_covers_it() {
    let project = shared();
    project.append("c/sub/x.rs", "// first change\n");
    let root = capture(&project, &project.root, "README.md");
    let c = capture(&project, &project.root, "c/README.md");
    project.append("c/sub/x.rs", "// second change\n");
    let refused = (3, vec!["snapshot_changed".to_string()]);
    assert_eq!(ack_in(&project, &project.root, "README.md", &root), refused);
    assert_eq!(ack_in(&project, &project.root, "c/README.md", &c), refused);
    let root = capture(&project, &project.root, "README.md");
    assert_eq!(
        ack_in(&project, &project.root, "README.md", &root),
        (0, vec![])
    );
}

#[test]
fn a_consumer_waits_for_its_provider_and_the_rendered_import() {
    let project = shared();
    let text = project.read_string("b/README.md").replace("one", "two");
    project.write("b/README.md", text);
    let (code, early) = project.json(&["review", "a/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&early), vec!["dependencies_pending"]);
    let b = capture(&project, &project.root, "b/README.md");
    assert_eq!(
        ack_in(&project, &project.root, "b/README.md", &b),
        (0, vec![])
    );
    let (code, outdated) = project.json(&["review", "a/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&outdated), vec!["imports_outdated"]);
    assert_eq!(
        project.run(&["render", "a/README.md"]).status.code(),
        Some(0)
    );
    let a = capture(&project, &project.root, "a/README.md");
    assert_eq!(
        ack_in(&project, &project.root, "a/README.md", &a),
        (0, vec![])
    );
    let (code, _) = project.json(&["check"]);
    assert_eq!(code, 0);
}

#[test]
fn two_reviewers_of_one_document_conflict_explicitly() {
    let project = shared();
    project.run(&[
        "invalidate",
        "doc:b/README.md",
        "--reason",
        "Two reviewers pick the same document.",
    ]);
    let first = capture(&project, &project.root, "b/README.md");
    let second = capture(&project, &project.root, "b/README.md");
    assert_eq!(
        ack_in(&project, &project.root, "b/README.md", &first),
        (0, vec![])
    );
    assert_eq!(
        ack_in(&project, &project.root, "b/README.md", &second),
        (3, vec!["revision_conflict".to_string()])
    );
    let state = project.inspect_state();
    assert_eq!(get_u64(&state, &["reviews", "b/README.md", "revision"]), 2);
}

#[test]
fn a_guidance_change_after_capture_refuses_the_older_artifact() {
    let project = shared();
    project.run(&[
        "invalidate",
        "all",
        "--reason",
        "Guidance changes while reviewers work.",
    ]);
    let c = capture(&project, &project.root, "c/README.md");
    let config = project
        .read_string("memoria.toml")
        .replace("guidance = []", "guidance = [\"Write for operators.\"]");
    project.write("memoria.toml", config);
    assert_eq!(
        ack_in(&project, &project.root, "c/README.md", &c),
        (3, vec!["guidance_changed".to_string()])
    );
}

#[test]
fn a_busy_lock_is_awaited_and_a_zero_wait_refuses_at_once() {
    let project = shared();
    project.append("c/sub/x.rs", "// pending\n");
    let c = capture(&project, &project.root, "c/README.md");
    let lock = project.write_lock();
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let guard = memoria_infrastructure::fs::lock_file(&lock).unwrap_or_else(|_| panic!("lock"));
    let refused = project
        .command(&project.root, &ack_args("c/README.md", &c, "fixture"))
        .env("MEMORIA_LOCK_WAIT_MS", "0")
        .output()
        .unwrap();
    assert_eq!(outcome(&refused), (3, vec!["state_busy".to_string()]));
    let child = project
        .command(&project.root, &ack_args("c/README.md", &c, "fixture"))
        .env("MEMORIA_LOCK_WAIT_MS", "20000")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(400));
    drop(guard);
    let output = child.wait_with_output().unwrap();
    assert_eq!(outcome(&output), (0, vec![]));
}

#[test]
fn an_invalid_lock_wait_is_a_usage_error_for_mutations_only() {
    let project = shared();
    let output = project
        .command(
            &project.root,
            &[
                "invalidate",
                "all",
                "--reason",
                "Invalid wait setting check.",
                "--format",
                "json",
            ],
        )
        .env("MEMORIA_LOCK_WAIT_MS", "soon")
        .output()
        .unwrap();
    assert_eq!(outcome(&output), (2, vec!["lock_wait_invalid".to_string()]));
    let read_only = project
        .command(&project.root, &["check"])
        .env("MEMORIA_LOCK_WAIT_MS", "soon")
        .output()
        .unwrap();
    assert_eq!(read_only.status.code(), Some(0));
}

/// A linked worktree with its own branch, created from the current commit.
fn worktree(project: &Project, name: &str) -> (tempfile::TempDir, PathBuf) {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().canonicalize().unwrap().join(name);
    let output = project.git(&["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    (parent, path)
}

#[test]
fn a_worktree_artifact_is_acknowledged_in_main_after_identical_integration() {
    let project = shared();
    let (_keep, wt) = worktree(&project, "worker-a");
    std::fs::write(wt.join("a/lib.rs"), "fn a() {}\nfn a2() {}\n").unwrap();
    project.git_in(&wt, &["add", "-A"]);
    project.git_in(&wt, &["commit", "-qm", "worker a"]);
    let packet = capture(&project, &wt, "a/README.md");
    let merged = project.git(&["merge", "-q", "--ff-only", "worker-a"]);
    assert!(merged.status.success(), "{merged:?}");
    // The worktree never acknowledged, so main's state is the only state.
    assert_eq!(
        ack_in(&project, &project.root, "a/README.md", &packet),
        (0, vec![])
    );
    assert_eq!(status_of(&project, "a/README.md"), "current");
}

#[test]
fn a_worktree_artifact_is_refused_when_the_integration_differs() {
    let project = shared();
    let (_keep, wt) = worktree(&project, "worker-a");
    std::fs::write(wt.join("a/lib.rs"), "fn a() {}\nfn a2() {}\n").unwrap();
    project.git_in(&wt, &["add", "-A"]);
    project.git_in(&wt, &["commit", "-qm", "worker a"]);
    let packet = capture(&project, &wt, "a/README.md");
    // Another change to a's scope lands in main first.
    project.write("a/other.rs", "fn other() {}\n");
    project.commit_all("other worker");
    let merged = project.git(&["merge", "-q", "--no-edit", "worker-a"]);
    assert!(merged.status.success(), "{merged:?}");
    assert_eq!(
        ack_in(&project, &project.root, "a/README.md", &packet),
        (3, vec!["snapshot_changed".to_string()])
    );
    let fresh = capture(&project, &project.root, "a/README.md");
    assert_eq!(
        ack_in(&project, &project.root, "a/README.md", &fresh),
        (0, vec![])
    );
}
