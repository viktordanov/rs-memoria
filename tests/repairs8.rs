//! Regression tests for the round-8 triage findings (MEM-031 reopened,
//! MEM-039 partial, MEM-040 partial).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::*;
use memoria_infrastructure::json::{self, Json, Limits};

/// Run Git in `dir` with the fixture's isolated environment, so no host
/// global or system configuration reaches this repository.
fn git_in(project: &Project, dir: &std::path::Path, args: &[&str]) -> String {
    let output = project.git_in(dir, args);
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn count_elements(value: &Json) -> u64 {
    match value {
        Json::Array(items) => items.len() as u64 + items.iter().map(count_elements).sum::<u64>(),
        Json::Object(map) => map.values().map(count_elements).sum(),
        _ => 0,
    }
}

// MEM-031 (reopened): a submodule's own clean filter never runs during parent inspection.
#[test]
fn parent_inspection_never_executes_submodule_filters() {
    let project = Project::seed();
    let module = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "m@example.com"],
        vec!["config", "user.name", "M"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git_in(&project, module.path(), &args);
    }
    fs::write(module.path().join("a.txt"), "first\n").unwrap();
    git_in(&project, module.path(), &["add", "-A"]);
    git_in(&project, module.path(), &["commit", "-qm", "seed"]);
    project.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        module.path().to_str().unwrap(),
        "child",
    ]);
    project.commit_all("submodule");
    project.baseline();
    // The submodule configures a clean filter in its own Git directory's info/attributes.
    let helpers = tempfile::tempdir().unwrap();
    let sentinel = helpers.path().join("submodule-helper-called");
    let helper = helpers.path().join("filter.sh");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf called >> \"{}\"\ncat\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    let sub = project.root.join("child");
    git_in(
        &project,
        &sub,
        &["config", "filter.probe.clean", helper.to_str().unwrap()],
    );
    let git_dir =
        std::path::PathBuf::from(git_in(&project, &sub, &["rev-parse", "--absolute-git-dir"]));
    fs::create_dir_all(git_dir.join("info")).unwrap();
    fs::write(git_dir.join("info/attributes"), "*.txt filter=probe\n").unwrap();
    let before = project.tree_snapshot();
    for (i, args) in [
        vec!["status"],
        vec!["lint"],
        vec!["review"],
        vec!["review", "src/execution/README.md"],
        vec!["check"],
        vec!["graph"],
        vec!["render", "--dry-run"],
    ]
    .into_iter()
    .enumerate()
    {
        // A fresh same-size edit forces Git's content comparison path each time.
        fs::write(sub.join("a.txt"), format!("{:05}\n", i)).unwrap();
        let _ = fs::remove_file(&sentinel);
        for format in [vec![], vec!["--format", "json"]] {
            let mut full = args.clone();
            full.extend(format.iter().copied());
            if args == ["review", "src/execution/README.md"] {
                // The document is current in this scenario; make it pending first.
                project.append("src/execution/runner.rs", "// edit\n");
            }
            let output = project.run(&full);
            assert!(
                output.status.code() == Some(0)
                    || (args == ["check"] && output.status.code() == Some(1)),
                "{full:?}: {}",
                stdout(&output)
            );
            assert!(!sentinel.exists(), "{full:?}: the submodule filter ran");
        }
    }
    let mut after = project.tree_snapshot();
    let mut expected = before.clone();
    for map in [&mut after, &mut expected] {
        map.retain(|k, _| !k.starts_with("child/") && k != "src/execution/runner.rs");
    }
    assert_eq!(after, expected, "inspection wrote nothing in the parent");
    // Parent context stays meaningful: the submodule worktree edit is opaque, a staged gitlink change is dirty.
    project.commit_all("runner edit");
    fs::write(sub.join("a.txt"), "first\n").unwrap();
    let (packet, _) = {
        project.append("src/execution/README.md", "\nProse.\n");
        project.commit_all("prose");
        project.review_packet("src/execution/README.md")
    };
    let value = parse_json(&fs::read(&packet).unwrap());
    assert!(
        !get_bool(&value, &["data", "context", "git", "worktree_dirty"]),
        "clean parent with an untouched submodule"
    );
    fs::write(sub.join("a.txt"), "other\n").unwrap();
    let _ = fs::remove_file(&sentinel);
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    assert!(
        !get_bool(&value, &["data", "context", "git", "worktree_dirty"]),
        "submodule worktree content is opaque to parent context"
    );
    assert!(!sentinel.exists());
    git_in(&project, &sub, &["add", "-A"]);
    git_in(&project, &sub, &["config", "user.email", "m@example.com"]);
    git_in(&project, &sub, &["config", "user.name", "M"]);
    git_in(&project, &sub, &["config", "commit.gpgsign", "false"]);
    git_in(&project, &sub, &["commit", "-qm", "advance"]);
    project.git(&["add", "child"]);
    let _ = fs::remove_file(&sentinel);
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    assert!(
        get_bool(&value, &["data", "context", "git", "worktree_dirty"]),
        "a staged gitlink change is a parent change"
    );
    assert!(!sentinel.exists());
}

// MEM-039 (partial): a root-located instruction-only sidecar is context, not policy.
#[test]
fn root_guidance_only_sidecar_is_context_not_policy() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    project.write(
        "README.memoria.toml",
        "[documentation]\nguidance = [\n    \"Use clear short sentences.\",\n]\n",
    );
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "adding a root instruction-only sidecar changes no fingerprint"
    );
    project.write(
        "README.memoria.toml",
        "[documentation]\nguidance = [\n    \"Use even shorter sentences.\",\n]\n",
    );
    assert_eq!(project.json(&["check"]).0, 0);
    // Every descendant inherits the root sidecar's instruction in its packet.
    project.append("src/retrieval/naive/search.rs", "// edit\n");
    let (packet, _) = project.review_packet("src/retrieval/naive/README.md");
    let value = parse_json(&fs::read(packet).unwrap());
    let Json::Array(guidance) = get(&value, &["data", "context", "guidance", "entries"]) else {
        panic!()
    };
    assert!(
        guidance
            .iter()
            .any(|i| get_str(i, &["text"]) == "Use even shorter sentences."
                && get_str(i, &["source"]) == "README.memoria.toml")
    );
    project.ack_ok("src/retrieval/naive/README.md");
    fs::remove_file(project.root.join("README.memoria.toml")).unwrap();
    assert_eq!(project.json(&["check"]).0, 0, "removing it changes nothing");
    assert_ne!(project.state(), before);
    // A root sidecar with a real rule is policy for every owner; memoria.toml edits remain policy too.
    project.write("README.memoria.toml", "ignore = [\n    \"*.tmp\",\n]\n");
    for doc in [
        "README.md",
        "src/corpus/README.md",
        "src/retrieval/naive/README.md",
    ] {
        assert_eq!(project.cause_codes(doc), vec!["input_changed"], "{doc}");
    }
    fs::remove_file(project.root.join("README.memoria.toml")).unwrap();
    assert_eq!(project.json(&["check"]).0, 0);
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "version = 2\n",
            "version = 2\ninclude = [\n    \"nothing/**\",\n]\n",
        ),
    );
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
}

// MEM-040 (partial): human and JSON report one complete record count.
#[test]
fn human_and_json_record_counts_are_the_same_complete_count() {
    fn human_count(output: &std::process::Output) -> u64 {
        let text = stdout(output);
        let line = text
            .lines()
            .find(|l| l.starts_with("Input size"))
            .expect("Input size line");
        let words: Vec<&str> = line.split_whitespace().collect();
        let index = words.iter().position(|w| *w == "in").unwrap();
        words[index + 1].parse().unwrap()
    }
    // Minimal repository with a disconnected child: one navigation diagnostic in the envelope.
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n");
    project.write("child/README.md", "# Child\n");
    project.commit_all("seed");
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    let json_output = project.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(json_output.status.code(), Some(0));
    let envelope = json::parse(&json_output.stdout, Limits::PACKET).unwrap();
    let independent = count_elements(&envelope);
    assert_eq!(
        get_u64(&envelope, &["data", "size", "record_count"]),
        independent
    );
    assert!(
        matches!(get(&envelope, &["diagnostics"]), Json::Array(d) if !d.is_empty()),
        "the navigation warning is present"
    );
    let human = project.run(&["review", "README.md"]);
    assert_eq!(human.status.code(), Some(0));
    assert_eq!(
        human_count(&human),
        independent,
        "human count equals JSON and the independent count"
    );
    // With prior-review context and without diagnostics.
    let project = Project::seed();
    project.baseline();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "version = 2\n",
            "version = 2\nlint.missing_import_hint = false\n",
        ),
    );
    project.append("README.md", "\nSee [disconnected](src/disconnected/).\n");
    project.canonical_loop();
    project.commit_all("baseline");
    project.append("src/execution/runner.rs", "// edit\n");
    let json_output = project.run(&["review", "src/execution/README.md", "--format", "json"]);
    let envelope = json::parse(&json_output.stdout, Limits::PACKET).unwrap();
    assert!(
        matches!(get(&envelope, &["diagnostics"]), Json::Array(d) if d.is_empty()),
        "no diagnostics in this snapshot"
    );
    assert!(!matches!(
        get(&envelope, &["data", "context", "previous_review"]),
        Json::Null
    ));
    let independent = count_elements(&envelope);
    assert_eq!(
        get_u64(&envelope, &["data", "size", "record_count"]),
        independent
    );
    let human = project.run(&["review", "src/execution/README.md"]);
    assert_eq!(human_count(&human), independent);
}
