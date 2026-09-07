//! Regression tests for the round-6 triage findings (MEM-036, MEM-037).

mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use memoria_infrastructure::json::Json;

/// Run a command with a completion deadline; a hang is a failure, not a wait.
fn run_with_deadline(project: &Project, args: &[&str], seconds: u64) -> std::process::Output {
    let mut child = project
        .command(&project.root, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("memoria {args:?} did not finish within {seconds}s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dirty_flag(project: &Project, document: &str) -> bool {
    let (packet, _) = project.review_packet(document);
    let value = parse_json(&fs::read(packet).unwrap());
    get_bool(&value, &["data", "context", "git", "worktree_dirty"])
}

// MEM-036
#[test]
fn raw_dirtiness_fallback_never_follows_links_or_opens_special_files() {
    let project = Project::seed();
    let outside = tempfile::tempdir().unwrap();
    let fifo = outside.path().join("external-fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    std::os::unix::fs::symlink(&fifo, project.root.join("link")).unwrap();
    project.write(
        "memoria.yml",
        project
            .read_string("memoria.yml")
            .replace("ignore:\n", "ignore:\n  - \"link\"\n"),
    );
    project.commit_all("excluded link to an external fifo");
    // Enable the raw fallback: info/attributes declares a filter that --attr-source cannot override.
    project.write(".git/info/attributes", "unused filter=anything\n");
    let output = run_with_deadline(&project, &["status", "--format", "json"], 30);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(
        get_u64(&value, &["data", "exclusions", "memoria-rule"]),
        2,
        "the link is excluded and harmless"
    );
    project.baseline();
    // Unchanged link text: the committed tree is clean, compared by link text only.
    project.commit_all("baseline");
    project.append("src/execution/README.md", "\nProse.\n");
    project.commit_all("prose");
    assert!(
        !dirty_flag(&project, "src/execution/README.md"),
        "link text unchanged means clean"
    );
    project.ack_ok("src/execution/README.md");
    // Retargeting the link (different text) is dirty, still without opening the target.
    fs::remove_file(project.root.join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("other-fifo"), project.root.join("link"))
        .unwrap();
    project.append("src/execution/README.md", "\nMore.\n");
    let output = run_with_deadline(
        &project,
        &["review", "src/execution/README.md", "--format", "json"],
        30,
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(get_bool(
        &parse_json(&output.stdout),
        &["data", "context", "git", "worktree_dirty"]
    ));

    // A tracked regular file replaced by an excluded FIFO: type change, no open, dirty.
    let project = Project::seed();
    project.write("pipe.txt", "regular\n");
    project.write(
        "memoria.yml",
        project
            .read_string("memoria.yml")
            .replace("ignore:\n", "ignore:\n  - \"pipe.txt\"\n"),
    );
    project.commit_all("regular file");
    project.write(".git/info/attributes", "unused filter=anything\n");
    project.baseline();
    fs::remove_file(project.root.join("pipe.txt")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(project.root.join("pipe.txt"))
            .status()
            .unwrap()
            .success()
    );
    let output = run_with_deadline(&project, &["status", "--format", "json"], 30);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    project.append("src/execution/README.md", "\nProse.\n");
    let output = run_with_deadline(
        &project,
        &["review", "src/execution/README.md", "--format", "json"],
        30,
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(
        get_bool(
            &parse_json(&output.stdout),
            &["data", "context", "git", "worktree_dirty"]
        ),
        "type change is dirty"
    );

    // A symlink ancestor pointing outside (with a FIFO behind it) is never followed.
    let project = Project::seed();
    project.write("lib/util.rs", "pub fn util() {}\n");
    project.write(
        "memoria.yml",
        project
            .read_string("memoria.yml")
            .replace("ignore:\n", "ignore:\n  - \"lib\"\n  - \"lib/**\"\n"),
    );
    project.commit_all("lib");
    project.write(".git/info/attributes", "unused filter=anything\n");
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(outside.path().join("util.rs"))
            .status()
            .unwrap()
            .success()
    );
    fs::remove_dir_all(project.root.join("lib")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root.join("lib")).unwrap();
    let output = run_with_deadline(&project, &["status", "--format", "json"], 30);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "excluded link and tree stay harmless"
    );

    // Submodule contents are opaque for the context as well.
    let project = Project::seed();
    let module = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "m@example.com"],
        vec!["config", "user.name", "M"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(module.path())
                .args(&args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fs::write(module.path().join("lib.rs"), "pub fn m() {}\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "m"]] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(module.path())
                .args(&args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let added = Command::new("git")
        .arg("-C")
        .arg(&project.root)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            module.path().to_str().unwrap(),
            "vendor/module",
        ])
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    project.commit_all("submodule");
    project.write(".git/info/attributes", "unused filter=anything\n");
    project.baseline();
    fs::write(
        project.root.join("vendor/module/lib.rs"),
        "pub fn changed() {}\n",
    )
    .unwrap();
    let output = run_with_deadline(&project, &["status", "--format", "json"], 30);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(project.json(&["check"]).0, 0);
}

// MEM-037
#[test]
fn percent_encoded_local_links_provide_navigation() {
    let project = Project::seed();
    let child = |dir: &str| {
        format!(
            "# Child\n\n<!-- memoria:export id=\"summary\" -->\nChild {dir}.\n<!-- /memoria:export -->\n"
        )
    };
    for dir in ["a b", "café", "x#y", "100%", "a+b"] {
        project.write(&format!("{dir}/README.md"), child(dir));
        project.write(&format!("{dir}/item.rs",), "x\n");
    }
    project.append(
        "README.md",
        "\nSee [space](a%20b/README.md), [utf8](caf%C3%A9/README.md), [hash](x%23y/README.md#top), [percent](100%25/README.md), [plus](a+b/), [bad](%zz/README.md), [escape](%2e%2e/README.md).\n",
    );
    project.commit_all("children");
    project.baseline();
    let (code, graph) = project.json(&["graph"]);
    assert_eq!(code, 0);
    let Json::Array(edges) = get(&graph, &["data", "edges"]) else {
        panic!()
    };
    let links: Vec<&str> = edges
        .iter()
        .filter(|e| get_str(e, &["kind"]) == "link" && get_str(e, &["from"]) == "README.md")
        .map(|e| get_str(e, &["to"]))
        .collect();
    for target in [
        "a b/README.md",
        "café/README.md",
        "x#y/README.md",
        "100%/README.md",
        "a+b/README.md",
    ] {
        assert!(links.contains(&target), "{target} missing from {links:?}");
    }
    let Json::Array(nodes) = get(&graph, &["data", "nodes"]) else {
        panic!()
    };
    for node in nodes.iter().filter(|n| {
        [
            "a b/README.md",
            "café/README.md",
            "x#y/README.md",
            "100%/README.md",
            "a+b/README.md",
        ]
        .contains(&get_str(n, &["document"]))
    }) {
        assert!(
            !get_bool(node, &["disconnected"]),
            "{}",
            get_str(node, &["document"])
        );
    }
    let (_, lint) = project.json(&["lint"]);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let disconnected: Vec<&str> = diagnostics
        .iter()
        .filter(|d| get_str(d, &["code"]) == "navigation_disconnected")
        .map(|d| get_str(d, &["path"]))
        .collect();
    assert_eq!(
        disconnected,
        vec!["src/disconnected/README.md"],
        "only the fixture's disconnected README warns"
    );
    // Normal links stay navigation-only: hints, no dependencies, no authored-byte changes.
    assert!(
        diagnostics
            .iter()
            .any(|d| get_str(d, &["code"]) == "missing_import_hint")
    );
    assert!(project.waiting_on("README.md").is_empty());
    let before = project.read("README.md");
    assert_eq!(project.json(&["render"]).0, 0);
    assert_eq!(project.read("README.md"), before);
    assert_eq!(project.json(&["check"]).0, 0);
}
