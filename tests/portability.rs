//! Portable freshness: equal selected inputs and equal repository policy
//! produce equal freshness across hosts.
//!
//! Actual file eligibility still follows the host Git behavior. Repository
//! policy does not: it is built from committed `.gitignore` bytes alone.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

/// Point a project's Git at a host excludes file it owns.
fn host_excludes(project: &Project, contents: &str) -> std::path::PathBuf {
    let path = project.root.join(".git/host-excludes");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    project.git(&["config", "core.excludesFile", path.to_str().unwrap()]);
    path
}

fn policy_hash(project: &Project, document: &str) -> String {
    let state = project.inspect_state();
    get_str(
        &state,
        &["reviews", document, "input_manifest", "policy_hash"],
    )
    .to_string()
}

#[test]
fn irrelevant_host_rules_preserve_review_state() {
    let project = Project::seed();
    project.baseline();
    let state = project.state();
    let hash = policy_hash(&project, "README.md");
    let selected = get_u64(&project.json(&["status"]).1, &["data", "selected_files"]);
    // Every host ignore source, varied in turn. None matches a project input.
    let excludes = host_excludes(&project, "never-matched-a/**\n");
    let xdg = project.home.path().join(".config/git");
    fs::create_dir_all(&xdg).unwrap();
    for (label, action) in [
        ("explicit global", 0),
        ("xdg fallback", 1),
        ("info exclude", 2),
    ] {
        match action {
            0 => fs::write(&excludes, "never-matched-b/**\n").unwrap(),
            1 => {
                project.git(&["config", "--unset", "core.excludesFile"]);
                fs::write(xdg.join("ignore"), "never-matched-c/**\n").unwrap();
            }
            _ => {
                fs::create_dir_all(project.root.join(".git/info")).unwrap();
                fs::write(
                    project.root.join(".git/info/exclude"),
                    "never-matched-d/**\n",
                )
                .unwrap();
            }
        }
        let (code, status) = project.json(&["status"]);
        assert_eq!(code, 0, "{label}: {status:?}");
        assert_eq!(
            get(&status, &["data", "reviews", "pending"]),
            &Json::Number(0),
            "{label}: no document became pending"
        );
        assert_eq!(
            get_u64(&status, &["data", "selected_files"]),
            selected,
            "{label}: the selected set is unchanged"
        );
        assert_eq!(project.json(&["check"]).0, 0, "{label}");
        assert_eq!(project.state(), state, "{label}: state bytes unchanged");
        assert_eq!(
            policy_hash(&project, "README.md"),
            hash,
            "{label}: policy hash unchanged"
        );
    }
}

#[test]
fn matching_host_rules_change_only_actual_owners() {
    for source in ["global", "info"] {
        let project = Project::seed();
        project.baseline();
        // An untracked source under one boundary only.
        project.write("src/corpus/extra.rs", "fn extra() {}\n");
        assert_eq!(
            project.cause_codes("src/corpus/README.md"),
            vec!["input_changed"],
            "{source}: the new owner gained the file"
        );
        assert_eq!(
            project.cause_codes("src/execution/README.md"),
            Vec::<String>::new(),
            "{source}: an unrelated owner is untouched"
        );
        project.ack_ok("src/corpus/README.md");
        assert_eq!(project.json(&["check"]).0, 0, "{source}");
        // Hiding it again is a loss for exactly that owner.
        match source {
            "global" => {
                host_excludes(&project, "src/corpus/extra.rs\n");
            }
            _ => {
                fs::create_dir_all(project.root.join(".git/info")).unwrap();
                fs::write(
                    project.root.join(".git/info/exclude"),
                    "src/corpus/extra.rs\n",
                )
                .unwrap();
            }
        }
        assert_eq!(
            project.cause_codes("src/corpus/README.md"),
            vec!["input_changed"],
            "{source}: the owner lost the file"
        );
        assert_eq!(
            project.cause_codes("src/execution/README.md"),
            Vec::<String>::new(),
            "{source}"
        );
        let (_, explain) = project.json(&["status", "--explain", "src/corpus/extra.rs"]);
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "git-ignored",
            "{source}: reporting still names the actual Git source"
        );
    }
}

#[test]
fn tracked_files_remain_inputs_under_host_excludes() {
    let project = Project::seed();
    project.baseline();
    // `app.rs` is tracked. A matching host rule cannot remove it.
    host_excludes(&project, "app.rs\n");
    assert_eq!(project.json(&["check"]).0, 0, "tracked file stays selected");
    let before = policy_hash(&project, "README.md");
    project.append("app.rs", "// changed\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    let (packet, _) = project.review_packet("README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(files) = get(&value, &["data", "manifest", "files"]) else {
        panic!()
    };
    assert!(
        files.iter().any(|f| get_str(f, &["path"]) == "app.rs"),
        "the tracked file is still a manifest entry"
    );
    assert_eq!(
        get_str(&value, &["data", "manifest", "policy_hash"]),
        before,
        "the host rule changed no policy"
    );
}

#[test]
fn repository_rules_remain_deterministic_inputs() {
    let project = Project::seed();
    project.baseline();
    // A repository rule that selects nothing new still changes policy for
    // every inheriting boundary.
    project.append(".gitignore", "never-present-here/**\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    // Every boundary inherits the root rule change, so the plan starts at a
    // leaf that waits on nothing. Compare that document with its own record.
    let ready = project.next_ready().expect("a ready document");
    let recorded = policy_hash(&project, &ready);
    let (packet, _) = project.review_packet(&ready);
    assert_ne!(
        get_str(
            &parse_json(&fs::read(&packet).unwrap()),
            &["data", "manifest", "policy_hash"]
        ),
        recorded,
        "{ready}: the repository rule changed the policy"
    );
    // A nested repository rule reaches only its own scope.
    let scoped = Project::seed();
    scoped.baseline();
    let scoped_leaf = policy_hash(&scoped, "src/retrieval/naive/README.md");
    scoped.write("src/corpus/.gitignore", "local-only/**\n");
    assert_eq!(
        scoped.cause_codes("src/corpus/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        scoped.cause_codes("src/retrieval/naive/README.md"),
        Vec::<String>::new(),
        "an unrelated boundary keeps its policy"
    );
    assert_eq!(
        policy_hash(&scoped, "src/retrieval/naive/README.md"),
        scoped_leaf
    );
}

#[test]
fn host_directory_rules_do_not_hide_repository_policy() {
    let project = Project::seed();
    // A directory whose only eligible file is its own `.gitignore`, so a
    // host rule that hides it removes no selected input.
    project.write("rules-only/.gitignore", "*.generated\n");
    project.baseline();
    let before = policy_hash(&project, "README.md");
    // Hide the whole directory through a host rule only. Git stops
    // traversing it, but the repository policy inventory does not.
    host_excludes(&project, "rules-only/\n");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(
        get(&status, &["data", "reviews", "pending"]),
        &Json::Number(0),
        "a host directory rule must not change repository policy"
    );
    assert_eq!(policy_hash(&project, "README.md"), before);
    // Editing the hidden directory's rules is still a real policy change.
    project.write("rules-only/.gitignore", "*.other\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
}

#[test]
fn repository_ignored_directories_keep_nested_rules_inactive() {
    let project = Project::seed();
    project.write(".gitignore", "ignored-output/**\nhidden/\n");
    project.write("hidden/.gitignore", "*.log\n");
    project.write("hidden/tracked.rs", "fn tracked() {}\n");
    project.git(&["add", "-f", "hidden/tracked.rs", "hidden/.gitignore"]);
    project.commit_all("tracked inside an ignored directory");
    project.baseline();
    let before = policy_hash(&project, "README.md");
    // The nested rules are inactive: Git never traverses the directory.
    project.write("hidden/.gitignore", "*.other\n");
    assert_eq!(
        project.cause_codes("README.md"),
        Vec::<String>::new(),
        "an inactive nested rule file changes no policy"
    );
    assert_eq!(policy_hash(&project, "README.md"), before);
    // Its tracked content is still a selected input.
    project.append("hidden/tracked.rs", "// changed\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
}

#[test]
fn host_paths_and_settings_never_enter_canonical_policy() {
    let external = tempfile::tempdir().unwrap();
    let mut hashes = std::collections::BTreeSet::new();
    for (label, configure) in [
        ("unset", 0usize),
        ("absolute with spaces", 1),
        ("explicitly empty", 2),
        ("ignore case on", 3),
    ] {
        let project = Project::seed();
        match configure {
            1 => {
                let path = external.path().join("host ignore file ");
                fs::write(&path, "unrelated-name/**\n").unwrap();
                project.git(&["config", "core.excludesFile", path.to_str().unwrap()]);
            }
            2 => {
                project.git(&["config", "core.excludesFile", ""]);
            }
            3 => {
                project.git(&["config", "core.ignoreCase", "true"]);
            }
            _ => {}
        }
        project.baseline();
        hashes.insert(policy_hash(&project, "README.md"));
        assert_eq!(project.json(&["check"]).0, 0, "{label}");
    }
    assert_eq!(
        hashes.len(),
        1,
        "equal repository inputs must produce one policy: {hashes:?}"
    );
}

#[test]
fn same_size_edits_renames_and_deletions_remain_visible() {
    let project = Project::seed();
    project.baseline();
    let path = project.root.join("src/execution/runner.rs");
    let original = fs::read(&path).unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    // A same-size edit with a preserved modification time.
    let mut edited = original.clone();
    let index = edited.iter().position(|b| *b == b'f').unwrap();
    edited[index] = b'F';
    assert_eq!(edited.len(), original.len());
    fs::write(&path, &edited).unwrap();
    filetime_set(&path, mtime);
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"],
        "a same-size, same-mtime edit is still a byte change"
    );
    project.ack_ok("src/execution/README.md");
    // A rename inside one boundary is one removal and one addition.
    fs::rename(&path, project.root.join("src/execution/renamed.rs")).unwrap();
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(changes) = get(&value, &["data", "context", "changes"]) else {
        panic!()
    };
    let kinds: Vec<String> = changes
        .iter()
        .map(|c| format!("{} {}", get_str(c, &["change"]), get_str(c, &["identity"])))
        .collect();
    assert!(
        kinds.contains(&"removed src/execution/runner.rs".to_string()),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"added src/execution/renamed.rs".to_string()),
        "{kinds:?}"
    );
}

/// Restore a file's modification time so a test can prove that freshness
/// comes from bytes, not timestamps.
fn filetime_set(path: &std::path::Path, when: std::time::SystemTime) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    let duration = when
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let times = rustix_times(duration);
    let _ = rustix::fs::futimens(&file, &times);
}

fn rustix_times(duration: std::time::Duration) -> rustix::fs::Timestamps {
    let spec = rustix::fs::Timespec {
        tv_sec: duration.as_secs() as i64,
        tv_nsec: duration.subsec_nanos() as i64,
    };
    rustix::fs::Timestamps {
        last_access: spec,
        last_modification: spec,
    }
}

#[test]
fn clean_clone_is_current_with_distinct_host_rule_profiles() {
    // One acknowledged, fully committed project, copied into two independent
    // clones with different harmless host settings. Both stay current.
    let origin = Project::seed();
    origin.baseline();
    origin.commit_all("acknowledged state");
    let committed_state = origin.state();
    for (label, rules) in [
        ("profile one", "clone-only-a/**\n"),
        ("profile two", "clone-only-b/**\nanother/**\n"),
    ] {
        let clone_dir = tempfile::tempdir().unwrap();
        let clone_root = clone_dir.path().join("clone");
        let clone_home = tempfile::tempdir().unwrap();
        let mut clone = std::process::Command::new("git");
        clone
            .args(["clone", "-q"])
            .arg(&origin.root)
            .arg(&clone_root);
        isolate_git(&mut clone, clone_home.path());
        let output = clone.output().unwrap();
        assert!(
            output.status.success(),
            "{label}: clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let excludes = clone_root.join(".git/host-excludes");
        fs::write(&excludes, rules).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new(memoria_bin())
                .current_dir(&clone_root)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", clone_home.path())
                .env("XDG_CONFIG_HOME", clone_home.path().join(".config"))
                .output()
                .unwrap()
        };
        git_isolated(
            &clone_root,
            clone_home.path(),
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        );
        // The committed state travels with the configuration.
        assert_eq!(
            fs::read(clone_root.join("memoria.lock")).unwrap(),
            committed_state,
            "{label}: the clone carries the same state bytes"
        );
        let output = run(&["check", "--format", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        // No acknowledgement happened, and the state was not rewritten.
        assert_eq!(
            fs::read(clone_root.join("memoria.lock")).unwrap(),
            committed_state,
            "{label}: check rewrote nothing"
        );
    }
}
