//! Regression tests for the round-2 triage findings (MEM-005, MEM-008,
//! MEM-014, MEM-019, MEM-020, MEM-021, MEM-022).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use common::*;
use memoria_infrastructure::json::Json;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn count_objects(root: &Path) -> usize {
    let mut count = 0;
    for entry in fs::read_dir(root.join(".git/objects")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            count += fs::read_dir(entry.path()).unwrap().count();
        }
    }
    count
}

// MEM-005
#[test]
fn context_dependent_reference_syntax_is_rejected_in_both_directions() {
    // Provider export uses an unresolved reference that a consumer could define.
    let project = Project::seed();
    project.baseline();
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee [manual][guide].\n<!-- /memoria:export -->\n");
    project.append("README.md", "\n[guide]: ../wrong.md\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"export_invalid".to_string()));
    let before = project.read("README.md");
    assert_eq!(project.json(&["render"]).0, 1);
    assert_eq!(project.read("README.md"), before);
    // Provider export carries a definition that would redefine consumer text.
    let project = Project::seed();
    project.baseline();
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\n[guide]: ../wrong.md\n<!-- /memoria:export -->\n");
    project.append("README.md", "\nSee [manual][guide].\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"export_invalid".to_string()));
    assert_eq!(project.json(&["render"]).0, 1);
    // Image variants.
    for body in ["![diagram][img]", "![img]", "[img]: ./i.png"] {
        project.write("src/execution/README.md", format!("# Execution\n\n<!-- memoria:export id=\"summary\" -->\n{body}\n<!-- /memoria:export -->\n"));
        let (code, lint) = project.json(&["lint"]);
        assert_eq!(code, 1, "{body}");
        assert!(
            diagnostic_codes(&lint).contains(&"export_invalid".to_string()),
            "{body}"
        );
    }
    // Literal code and escaped brackets stay allowed and render unchanged.
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nThe runner \\[executes\\] `[docs][ref]` items.\n\n```\n[guide]: ../x.md\n```\n<!-- /memoria:export -->\n");
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);
    assert_eq!(project.json(&["render"]).0, 0);
    assert!(
        project
            .read_string("README.md")
            .contains("The runner \\[executes\\] `[docs][ref]` items.")
    );
}

// MEM-008
#[test]
fn ignore_files_in_directories_without_eligible_files_still_change_policy() {
    let project = Project::seed();
    project.write(".gitignore", "ignored-output/**\n**/.gitignore\n");
    project.write("empty/.gitignore", "*.log\n");
    project.write("empty/cache.log", "ignored bytes\n");
    project.baseline();
    let (_, explain) = project.json(&["status", "--explain", "empty/cache.log"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "git-ignored"
    );
    project.write("empty/.gitignore", "cache.log\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    assert_eq!(project.json(&["check"]).0, 1);
    project.ack_ok("README.md");
    assert_eq!(project.json(&["check"]).0, 0);
    // An ignore file inside an ignored directory is never used by Git and never read.
    project.write(".gitignore", "ignored-output/**\n**/.gitignore\nbuild/\n");
    project.canonical_loop();
    project.write("build/.gitignore", "one\n");
    project.write("build/artifact.bin", "x\n");
    assert_eq!(project.json(&["check"]).0, 0);
    project.write("build/.gitignore", "two\n");
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "policy inside an ignored directory is not applicable"
    );
    let (_, status) = project.json(&["status"]);
    assert_eq!(get_u64(&status, &["data", "reviews", "current"]), 6);
    // Nested repositories are not entered for policy either.
    fs::create_dir_all(project.root.join("vendor/lib")).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(project.root.join("vendor/lib"))
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    project.write("vendor/lib/.gitignore", "inner\n");
    assert_eq!(project.json(&["check"]).0, 0);
}

// MEM-014
#[test]
fn help_and_version_delivery_failures_exit_four() {
    let project = Project::seed();
    for args in [vec!["--help"], vec!["--version"], vec!["review", "--help"]] {
        let ok = project.run(&args);
        assert_eq!(ok.status.code(), Some(0), "{args:?}");
        assert!(!stdout(&ok).is_empty());
        let full = fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let output = project
            .command(&project.root, &args)
            .stdout(Stdio::from(full))
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{args:?}");
        assert!(
            stderr(&output).contains("io_error"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

// MEM-019
#[test]
fn missing_historical_blobs_never_invoke_a_remote_helper() {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    // Review once so the record carries the base commit that holds runner.rs.
    project.append("src/execution/runner.rs", "// first\n");
    project.ack_ok("src/execution/README.md");
    let head = String::from_utf8(project.git(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    let _ = head;
    let oid = String::from_utf8(
        project
            .git(&["rev-parse", "HEAD:src/execution/runner.rs"])
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    project.append("src/execution/runner.rs", "// second\n");
    // Configure a promisor remote served by a local helper that records any invocation.
    let helpers = tempfile::tempdir().unwrap();
    let sentinel = helpers.path().join("remote-helper-called");
    let helper = helpers.path().join("git-remote-reviewtest");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf called > \"{}\"\nexit 1\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    project.git(&["config", "remote.origin.url", "reviewtest::unused"]);
    project.git(&["config", "remote.origin.promisor", "true"]);
    project.git(&["config", "remote.origin.partialclonefilter", "blob:none"]);
    project.git(&["config", "extensions.partialClone", "origin"]);
    let blob = project
        .root
        .join(".git/objects")
        .join(&oid[..2])
        .join(&oid[2..]);
    assert!(blob.exists(), "loose blob {oid} present");
    fs::remove_file(&blob).unwrap();
    let objects_before = count_objects(&project.root);
    let path = format!(
        "{}:{}",
        helpers.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = project
        .command(
            &project.root,
            &["review", "src/execution/README.md", "--format", "json"],
        )
        .env("PATH", &path)
        .env("GIT_ALLOW_PROTOCOL", "reviewtest")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        !sentinel.exists(),
        "the remote helper must never run during a read-only review"
    );
    assert_eq!(
        count_objects(&project.root),
        objects_before,
        "no objects were written"
    );
    let value = parse_json(&output.stdout);
    let Json::Array(diffs) = get(&value, &["data", "context", "diffs"]) else {
        panic!()
    };
    let runner = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/runner.rs")
        .unwrap();
    assert_eq!(get_str(runner, &["status"]), "unavailable");
    assert!(get_str(runner, &["reason"]).contains("not available"));
    // Every other read-only command also stays local.
    for args in [vec!["status"], vec!["check"], vec!["review"]] {
        let output = project
            .command(&project.root, &args)
            .env("PATH", &path)
            .env("GIT_ALLOW_PROTOCOL", "reviewtest")
            .output()
            .unwrap();
        assert!(output.status.code().is_some(), "{args:?}");
    }
    assert!(!sentinel.exists());
}

// MEM-020
#[test]
fn copied_installations_cannot_move_another_installations_backup() {
    let project = Project::seed();
    project.baseline();
    let parent = project.root.join("custom-skills");
    fs::create_dir_all(parent.join("memoria")).unwrap();
    fs::write(
        parent.join("memoria/user.txt"),
        "original unmanaged content\n",
    )
    .unwrap();
    let (code, _) = project.json(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--path",
        parent.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert!(parent.join("memoria.backup/user.txt").exists());
    let elsewhere = tempfile::tempdir().unwrap();
    let copy = elsewhere.path().join("skills");
    copy_dir(&parent, &copy);
    let (code, report) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        copy.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{report:?}");
    assert!(
        parent.join("memoria.backup/user.txt").exists(),
        "the original backup survives"
    );
    assert!(
        parent.join("memoria/SKILL.md").exists(),
        "the original installation survives"
    );
    assert!(
        !copy.join("memoria.backup").exists(),
        "the copy restored its own sibling backup"
    );
    assert_eq!(
        fs::read_to_string(copy.join("memoria/user.txt")).unwrap(),
        "original unmanaged content\n"
    );
    // A record pointing at an unrelated directory is rejected without writes.
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("user.txt"), "keep me").unwrap();
    let record_path = parent.join("memoria/.memoria-install.json");
    let edited = fs::read_to_string(&record_path).unwrap().replace(
        "\"backup\": \"memoria.backup\"",
        &format!("\"backup\": \"{}\"", outside.path().display()),
    );
    assert_ne!(edited, fs::read_to_string(&record_path).unwrap());
    fs::write(&record_path, edited).unwrap();
    let (code, conflict) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        parent.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);
    assert_eq!(diagnostic_codes(&conflict), vec!["skill_conflict"]);
    assert!(outside.path().join("user.txt").exists());
    assert!(parent.join("memoria/SKILL.md").exists());
    assert!(!parent.join("memoria/user.txt").exists());
    // Normal restoration with the correct sibling name.
    let restored = fs::read_to_string(&record_path).unwrap().replace(
        &format!("\"backup\": \"{}\"", outside.path().display()),
        "\"backup\": \"memoria.backup\"",
    );
    fs::write(&record_path, restored).unwrap();
    let (code, _) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        parent.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(parent.join("memoria/user.txt")).unwrap(),
        "original unmanaged content\n"
    );
}

// MEM-021
#[test]
fn interrupted_transactions_recover_through_the_cli() {
    let project = Project::seed();
    project.baseline();
    let parent = project.root.join("custom-skills");
    let parent_str = parent.to_str().unwrap().to_string();
    let txn = |phase: &str| {
        format!(
            "{{\"schema_version\":1,\"phase\":\"{phase}\",\"destination\":\"{}\",\"staging\":\"{}\",\"removing\":\"{}\",\"backup\":null}}",
            parent.join("memoria").display(),
            parent.join("memoria.staging").display(),
            parent.join("memoria.removing").display()
        )
    };
    // Interrupted removal: the package sits in memoria.removing with its transaction.
    assert_eq!(
        project
            .json(&[
                "agent",
                "install",
                "--target",
                "codex",
                "--path",
                &parent_str
            ])
            .0,
        0
    );
    fs::rename(parent.join("memoria"), parent.join("memoria.removing")).unwrap();
    fs::write(parent.join("memoria.install-txn.json"), txn("removing")).unwrap();
    let snapshot = project.tree_snapshot();
    let (code, dry) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        &parent_str,
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{dry:?}");
    assert!(get_bool(&dry, &["data", "plan", "recovery_needed"]));
    assert_eq!(project.tree_snapshot(), snapshot, "dry run writes nothing");
    let (code, report) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        &parent_str,
    ]);
    assert_eq!(code, 0, "{report:?}");
    assert!(get_bool(&report, &["data", "applied"]));
    assert!(!parent.join("memoria.removing").exists());
    assert!(!parent.join("memoria.install-txn.json").exists());
    assert!(!parent.join("memoria").exists());
    let (code, missing) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        &parent_str,
    ]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&missing), vec!["skill_not_installed"]);
    // Interrupted installation: a complete staged package with its transaction, destination absent.
    assert_eq!(
        project
            .json(&[
                "agent",
                "install",
                "--target",
                "codex",
                "--path",
                &parent_str
            ])
            .0,
        0
    );
    fs::rename(parent.join("memoria"), parent.join("memoria.staging")).unwrap();
    fs::write(parent.join("memoria.install-txn.json"), txn("staged")).unwrap();
    let (code, dry) = project.json(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--path",
        &parent_str,
        "--dry-run",
    ]);
    assert_eq!(code, 0);
    assert!(get_bool(&dry, &["data", "plan", "recovery_needed"]));
    assert!(parent.join("memoria.staging").exists());
    let (code, _) = project.json(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--path",
        &parent_str,
    ]);
    assert_eq!(code, 0);
    assert!(parent.join("memoria/SKILL.md").exists());
    assert!(!parent.join("memoria.staging").exists());
    assert!(!parent.join("memoria.install-txn.json").exists());
    // The same interrupted installation can also be resolved by uninstall: recovery finishes it, then removes it.
    fs::rename(parent.join("memoria"), parent.join("memoria.staging")).unwrap();
    fs::write(parent.join("memoria.install-txn.json"), txn("staged")).unwrap();
    let (code, dry) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        &parent_str,
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{dry:?}");
    assert!(get_bool(&dry, &["data", "plan", "recovery_needed"]));
    let (code, _) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "codex",
        "--path",
        &parent_str,
    ]);
    assert_eq!(code, 0);
    assert!(!parent.join("memoria").exists());
    assert!(!parent.join("memoria.staging").exists());
    assert!(!parent.join("memoria.install-txn.json").exists());
    // Unknown staged content stays a conflict through the CLI.
    fs::create_dir_all(parent.join("memoria.staging")).unwrap();
    fs::write(parent.join("memoria.staging/user.txt"), "keep").unwrap();
    fs::write(parent.join("memoria.install-txn.json"), txn("staged")).unwrap();
    let (code, conflict) = project.json(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--path",
        &parent_str,
    ]);
    assert_eq!(code, 3);
    assert_eq!(diagnostic_codes(&conflict), vec!["skill_conflict"]);
    assert!(parent.join("memoria.staging/user.txt").exists());
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "installer artifacts never become review inputs"
    );
}

// MEM-022
#[test]
fn init_rejects_an_invalid_existing_readme_before_writing() {
    for (name, readme) in [
        (
            "unclosed",
            b"<!-- memoria:export id=\"s\" -->\nnever closed\n".to_vec(),
        ),
        ("utf8", vec![0xff, 0xfe, b'#', b' ', b'x']),
    ] {
        let project = Project::empty_repo();
        project.write("README.md", &readme);
        project.write("source.txt", "original\n");
        project.commit_all("seed");
        let before = project.tree_snapshot();
        let (code, init) = project.json(&["init"]);
        assert_eq!(code, 1, "{name}: {init:?}");
        let codes = diagnostic_codes(&init);
        assert!(
            codes
                .iter()
                .all(|c| c == "marker_unclosed" || c == "markdown_invalid"),
            "{name}: {codes:?}"
        );
        assert!(!project.exists("memoria.toml"), "{name}");
        assert!(!project.exists(".memoria"), "{name}");
        assert_eq!(project.tree_snapshot(), before, "{name}: nothing written");
    }
    // A malformed import reference is rejected too; a valid README initializes normally.
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\n<!-- memoria:import src=\"nohash/README.md\" -->\n<!-- /memoria:import -->\n",
    );
    project.commit_all("seed");
    let (code, init) = project.json(&["init"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&init), vec!["import_invalid"]);
    assert!(!project.exists("memoria.toml"));
    project.write(
        "README.md",
        "# Root\n\n<!-- memoria:export id=\"summary\" -->\nFine.\n<!-- /memoria:export -->\n",
    );
    let (code, init) = project.json(&["init"]);
    assert_eq!(code, 0, "{init:?}");
    assert_eq!(
        strings(get(&init, &["data", "existing"])),
        vec!["README.md"]
    );
    assert!(project.exists("memoria.toml"));
    assert_eq!(project.json(&["lint"]).0, 0);
}
