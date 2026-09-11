//! `memoria integrations github`: the managed consumer workflow lifecycle.
//!
//! The CLI creates and maintains the workflow file. The setup Action installs
//! the executable on the runner. These fixtures cover preview and apply,
//! ownership and conflicts, path safety, durability, and the documentation
//! policy for the generated files.

mod common;

use std::fs;

use common::{Project, diagnostic_codes, get, get_bool, get_str, stderr, stdout, strings};
use memoria_infrastructure::json::Json;

const WORKFLOW: &str = ".github/workflows/memoria.yml";
const RECORD: &str = ".github/memoria-workflows/memoria.yml.json";

fn plan_str<'a>(value: &'a Json, key: &str) -> &'a str {
    get_str(value, &["data", "plan", key])
}

/// A seeded project that meets the workflow prerequisites.
///
/// The workflow runs `memoria check` on a runner, so install and upgrade
/// require an authored root README, a valid `memoria.toml`, and a readable
/// `memoria.lock`. A pending review is permitted: review follows the change.
fn initialized() -> Project {
    let project = Project::seed();
    let output = project.run(&["init", "--apply"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    project
}

fn install(project: &Project) -> Json {
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0, "install failed: {value:?}");
    value
}

// ---------------------------------------------------------------------------
// Preview and lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_preview_writes_nothing_and_names_its_apply_command() {
    let project = Project::seed();
    let before = project.tree_snapshot();
    for arguments in [
        vec!["integrations", "github", "install"],
        vec!["integrations", "github", "install", "--dry-run"],
        vec!["integrations", "github", "uninstall"],
    ] {
        let (code, value) = project.json(&arguments);
        assert_eq!(code, 0, "{arguments:?}: {value:?}");
        assert!(!get_bool(&value, &["data", "applied"]));
        assert_eq!(
            project.tree_snapshot(),
            before,
            "{arguments:?} wrote a file"
        );
    }
    let (_, value) = project.json(&["integrations", "github", "install"]);
    assert_eq!(
        get_str(&value, &["data", "apply_command"]),
        "memoria integrations github install --apply"
    );
    // A preview creates no directory, lock, or record either.
    assert!(!project.exists(".github"));
    assert!(!project.root.join(".git/memoria/github").exists());
}

#[test]
fn a_preview_states_that_publication_was_not_verified() {
    let project = Project::seed();
    let (code, value) = project.json(&["integrations", "github", "install"]);
    assert_eq!(code, 0);
    assert!(
        diagnostic_codes(&value).contains(&"github_publication_unverified".to_string()),
        "{:?}",
        diagnostic_codes(&value)
    );
}

#[test]
fn apply_and_dry_run_together_are_a_usage_error() {
    let project = Project::seed();
    let (code, value) =
        project.json(&["integrations", "github", "install", "--apply", "--dry-run"]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["apply_conflict"]);
}

#[test]
fn status_is_read_only_and_takes_no_change_options() {
    let project = Project::seed();
    for arguments in [
        vec!["integrations", "github", "status", "--apply"],
        vec!["integrations", "github", "status", "--dry-run"],
        vec!["integrations", "github", "status", "--version", "0.5.0"],
    ] {
        let output = project.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn the_lifecycle_installs_upgrades_and_uninstalls() {
    let project = initialized();

    let installed = install(&project);
    assert!(get_bool(&installed, &["data", "applied"]));
    assert_eq!(plan_str(&installed, "state"), "current");
    assert!(project.exists(WORKFLOW));
    assert!(project.exists(RECORD));
    let workflow = project.read_string(WORKFLOW);
    assert!(workflow.contains("uses: viktordanov/rs-memoria@v0.5.0"));
    assert!(workflow.contains("runs-on: ubuntu-24.04"));
    assert!(workflow.contains("version: '0.5.0'"));

    // A repeated install is a successful no-op that writes nothing.
    let snapshot = project.tree_snapshot();
    let (code, again) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0);
    assert!(get_bool(&again, &["data", "plan", "no_change"]));
    assert!(!get_bool(&again, &["data", "applied"]));
    assert_eq!(project.tree_snapshot(), snapshot);

    // Status reports the installed and the desired values.
    let (code, status) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(plan_str(&status, "state"), "current");
    assert_eq!(plan_str(&status, "installed_version"), "0.5.0");
    assert_eq!(plan_str(&status, "installed_runner"), "ubuntu-24.04");

    // An upgrade to another runner rewrites both files.
    let (code, upgraded) = project.json(&[
        "integrations",
        "github",
        "upgrade",
        "--runner",
        "ubuntu-24.04-arm",
        "--apply",
    ]);
    assert_eq!(code, 0, "{upgraded:?}");
    assert!(get_bool(&upgraded, &["data", "applied"]));
    assert_eq!(plan_str(&upgraded, "installed_runner"), "ubuntu-24.04-arm");
    assert!(
        project
            .read_string(WORKFLOW)
            .contains("runs-on: ubuntu-24.04-arm")
    );

    // Uninstall removes exactly the two owned files and leaves the shared
    // directories in place.
    let (code, removed) = project.json(&["integrations", "github", "uninstall", "--apply"]);
    assert_eq!(code, 0, "{removed:?}");
    assert_eq!(
        strings(get(&removed, &["data", "plan", "removals"])),
        vec![WORKFLOW, RECORD]
    );
    assert!(!project.exists(WORKFLOW));
    assert!(!project.exists(RECORD));
    assert!(project.root.join(".github/workflows").is_dir());

    // Uninstall again is a successful no-op.
    let (code, empty) = project.json(&["integrations", "github", "uninstall", "--apply"]);
    assert_eq!(code, 0);
    assert!(get_bool(&empty, &["data", "plan", "no_change"]));
}

#[test]
fn upgrade_without_an_installation_is_a_validation_failure() {
    let project = initialized();
    let (code, value) = project.json(&["integrations", "github", "upgrade", "--apply"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["github_not_installed"]);
}

#[test]
fn install_over_a_managed_workflow_with_other_parameters_requires_upgrade() {
    let project = initialized();
    install(&project);
    let (code, value) = project.json(&[
        "integrations",
        "github",
        "install",
        "--runner",
        "ubuntu-latest",
        "--apply",
    ]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["github_upgrade_required"]);
    assert!(
        project
            .read_string(WORKFLOW)
            .contains("runs-on: ubuntu-24.04")
    );
}

#[test]
fn a_downgrade_is_refused_and_a_commit_pin_is_preserved() {
    let project = initialized();
    let sha = "a".repeat(40);
    let (code, value) = project.json(&[
        "integrations",
        "github",
        "install",
        "--version",
        "1.2.3",
        "--action-ref",
        &sha,
        "--apply",
    ]);
    assert_eq!(code, 0, "{value:?}");
    assert!(
        project
            .read_string(WORKFLOW)
            .contains(&format!("rs-memoria@{sha}"))
    );

    // The running executable is older, so the default upgrade target is a
    // downgrade and must be refused.
    let (code, refused) = project.json(&["integrations", "github", "upgrade", "--apply"]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&refused), vec!["github_downgrade_refused"]);

    // An explicit newer version keeps the recorded commit pin and says so.
    let (code, upgraded) = project.json(&[
        "integrations",
        "github",
        "upgrade",
        "--version",
        "1.2.4",
        "--apply",
    ]);
    assert_eq!(code, 0, "{upgraded:?}");
    let workflow = project.read_string(WORKFLOW);
    assert!(
        workflow.contains(&format!("rs-memoria@{sha}")),
        "{workflow}"
    );
    assert!(workflow.contains("version: '1.2.4'"));
}

#[test]
fn a_release_tag_reference_advances_with_the_version() {
    let project = initialized();
    let (code, _) = project.json(&[
        "integrations",
        "github",
        "install",
        "--version",
        "1.0.0",
        "--apply",
    ]);
    assert_eq!(code, 0);
    assert!(project.read_string(WORKFLOW).contains("rs-memoria@v1.0.0"));
    let (code, _) = project.json(&[
        "integrations",
        "github",
        "upgrade",
        "--version",
        "1.1.0",
        "--apply",
    ]);
    assert_eq!(code, 0);
    assert!(project.read_string(WORKFLOW).contains("rs-memoria@v1.1.0"));
}

// ---------------------------------------------------------------------------
// Initialization prerequisites
// ---------------------------------------------------------------------------

#[test]
fn a_preview_reports_every_missing_prerequisite_and_writes_nothing() {
    let project = Project::empty_repo();
    let before = project.tree_snapshot();
    let (code, value) = project.json(&["integrations", "github", "install"]);
    assert_eq!(code, 0, "a preview works before initialization: {value:?}");
    let codes = diagnostic_codes(&value);
    assert_eq!(
        codes
            .iter()
            .filter(|code| *code == "github_prerequisite_missing")
            .count(),
        3,
        "{codes:?}"
    );
    let reported: Vec<String> = match get(&value, &["data", "missing_prerequisites"]) {
        Json::Array(items) => items
            .iter()
            .map(|item| get_str(item, &["path"]).to_string())
            .collect(),
        other => panic!("expected a list, got {other:?}"),
    };
    assert_eq!(reported, vec!["README.md", "memoria.toml", "memoria.lock"]);
    assert_eq!(project.tree_snapshot(), before);
}

#[test]
fn an_apply_refuses_before_any_write_when_a_prerequisite_is_missing() {
    for operation in ["install", "upgrade"] {
        let project = Project::empty_repo();
        let before = project.tree_snapshot();
        let (code, value) = project.json(&["integrations", "github", operation, "--apply"]);
        assert_eq!(code, 1, "{operation}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["github_prerequisites_missing"],
            "{operation}"
        );
        assert!(!project.exists(WORKFLOW), "{operation}");
        assert!(!project.exists(RECORD), "{operation}");
        assert_eq!(project.tree_snapshot(), before, "{operation} wrote a file");
        // Nothing was created under Git metadata either: the refusal happens
        // before the store takes a lock or writes an intent.
        assert!(
            !project.root.join(".git/memoria/github").exists(),
            "{operation}"
        );
    }
}

#[test]
fn each_prerequisite_is_reported_separately() {
    // Missing README only.
    let project = initialized();
    project.remove("README.md");
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 1, "{value:?}");
    assert_eq!(
        diagnostic_codes(&value),
        vec!["github_prerequisites_missing"]
    );
    let Json::Array(items) = get(&value, &["diagnostics"]) else {
        panic!("diagnostics is not an array")
    };
    assert!(
        get_str(&items[0], &["message"]).contains("README.md"),
        "{value:?}"
    );
    assert!(!project.exists(WORKFLOW));

    // Invalid configuration.
    let project = initialized();
    project.write("memoria.toml", "version = \"not a number\"\n");
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 1, "{value:?}");
    assert_eq!(
        diagnostic_codes(&value),
        vec!["github_prerequisites_missing"]
    );
    assert!(!project.exists(WORKFLOW));

    // Corrupt committed state.
    let project = initialized();
    project.write("memoria.lock", "not a lock file");
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 1, "{value:?}");
    assert!(!project.exists(WORKFLOW));
}

#[test]
fn a_pending_review_does_not_block_the_workflow() {
    // Review follows the workflow change, so `check` need not pass first.
    let project = initialized();
    let (pending, _) = project.json(&["check"]);
    assert_eq!(pending, 1, "the fixture starts with review pending");
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(project.exists(WORKFLOW));
}

#[test]
fn status_and_uninstall_ignore_configuration_validity() {
    // An uninitialized project must still see and remove a managed workflow.
    let project = initialized();
    install(&project);
    project.write("memoria.toml", "version = \"not a number\"\n");
    project.remove("README.md");

    let (code, status) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(plan_str(&status, "state"), "current");
    assert!(
        !diagnostic_codes(&status).contains(&"github_prerequisite_missing".to_string()),
        "status reports no prerequisite: {:?}",
        diagnostic_codes(&status)
    );

    let (code, removed) = project.json(&["integrations", "github", "uninstall", "--apply"]);
    assert_eq!(code, 0, "{removed:?}");
    assert!(!project.exists(WORKFLOW));

    // A status on a project that was never initialized also works.
    let empty = Project::empty_repo();
    let (code, value) = empty.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0, "{value:?}");
    assert_eq!(plan_str(&value, "state"), "absent");
    let (code, _) = empty.json(&["integrations", "github", "uninstall", "--apply"]);
    assert_eq!(code, 0);
}

// ---------------------------------------------------------------------------
// Interrupted transactions
// ---------------------------------------------------------------------------

fn intent_path(project: &Project) -> std::path::PathBuf {
    project
        .root
        .join(".git/memoria/github/memoria.yml.intent.json")
}

#[test]
fn a_completed_intent_clears_on_an_apply_that_writes_nothing() {
    let project = initialized();
    install(&project);
    let workflow = project.read_string(WORKFLOW);
    let record = project.read_string(RECORD);
    let intent = intent_path(&project);
    fs::write(
        &intent,
        format!(
            "{{\n  \"schema_version\": 1,\n  \"operation\": \"install\",\n  \"workflow_path\": {:?},\n  \"record_path\": {:?},\n  \"expected_workflow\": null,\n  \"intended_workflow\": {:?},\n  \"expected_record\": null,\n  \"intended_record\": {:?}\n}}\n",
            WORKFLOW, RECORD, workflow, record
        ),
    )
    .unwrap();

    // The desired files already match, so this apply writes nothing. It must
    // still settle the interrupted transaction.
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(
        !get_bool(&value, &["data", "applied"]),
        "nothing was written"
    );
    assert!(!get_bool(&value, &["data", "plan", "recovery_needed"]));
    assert!(!intent.exists(), "the completed intent is cleared");
    assert_eq!(project.read_string(WORKFLOW), workflow, "no replacement");
    assert_eq!(project.read_string(RECORD), record);
}

#[test]
fn an_unusable_intent_is_preserved_and_never_cleared() {
    for (label, body) in [
        ("invalid json", "{ not json at all".to_string()),
        (
            "another workflow",
            format!(
                "{{\"schema_version\": 1, \"operation\": \"install\", \"workflow_path\": \".github/workflows/other.yml\", \"record_path\": {:?}, \"expected_workflow\": null, \"intended_workflow\": null, \"expected_record\": null, \"intended_record\": null}}",
                RECORD
            ),
        ),
        (
            "unknown operation",
            format!(
                "{{\"schema_version\": 1, \"operation\": \"rewrite\", \"workflow_path\": {:?}, \"record_path\": {:?}, \"expected_workflow\": null, \"intended_workflow\": null, \"expected_record\": null, \"intended_record\": null}}",
                WORKFLOW, RECORD
            ),
        ),
    ] {
        let project = initialized();
        let intent = intent_path(&project);
        fs::create_dir_all(intent.parent().unwrap()).unwrap();
        fs::write(&intent, &body).unwrap();

        let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
        assert_eq!(code, 3, "{label}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["github_recovery_needed"],
            "{label}"
        );
        assert_eq!(
            fs::read_to_string(&intent).unwrap(),
            body,
            "{label}: the record is preserved byte for byte"
        );
        assert!(!project.exists(WORKFLOW), "{label}: nothing was installed");
    }
}

#[test]
fn a_preview_and_a_status_never_clear_an_intent() {
    let project = initialized();
    let intent = intent_path(&project);
    fs::create_dir_all(intent.parent().unwrap()).unwrap();
    fs::write(&intent, "{ not json at all").unwrap();
    for arguments in [
        vec!["integrations", "github", "install"],
        vec!["integrations", "github", "status"],
        vec!["integrations", "github", "install", "--dry-run"],
    ] {
        let (code, value) = project.json(&arguments);
        assert_eq!(code, 0, "{arguments:?}: {value:?}");
        assert!(intent.exists(), "{arguments:?} removed the intent");
    }
}

// ---------------------------------------------------------------------------
// Ownership and conflicts
// ---------------------------------------------------------------------------

#[test]
fn an_identical_unmanaged_file_is_never_adopted() {
    let project = initialized();
    let (_, preview) = project.json(&["integrations", "github", "install"]);
    let rendered = plan_str(&preview, "workflow").to_string();
    project.write(WORKFLOW, &rendered);

    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["github_unmanaged"]);
    assert_eq!(project.read_string(WORKFLOW), rendered);
    assert!(!project.exists(RECORD));

    let (code, status) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0, "a readable status exits 0 even for a conflict");
    assert_eq!(plan_str(&status, "state"), "unmanaged");
}

#[test]
fn an_edited_workflow_is_preserved_by_every_mutation() {
    let project = initialized();
    install(&project);
    let edited = format!("{}# a local edit\n", project.read_string(WORKFLOW));
    project.write(WORKFLOW, &edited);

    let (code, status) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(plan_str(&status, "state"), "modified");

    for operation in ["install", "upgrade", "uninstall"] {
        let (code, value) = project.json(&["integrations", "github", operation, "--apply"]);
        assert_eq!(code, 3, "{operation}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["github_modified"],
            "{operation}"
        );
        assert_eq!(project.read_string(WORKFLOW), edited, "{operation}");
    }
}

#[test]
fn an_edited_or_corrupt_record_never_authorizes_a_repair() {
    let project = initialized();
    install(&project);
    let workflow = project.read_string(WORKFLOW);
    let valid = project.read_string(RECORD);

    // An unusable record is an ownership conflict.
    for record in [
        "{}\n".to_string(),
        "not json at all\n".to_string(),
        valid.replace(
            ".github/workflows/memoria.yml",
            ".github/workflows/other.yml",
        ),
        "{\n  \"schema_version\": 99\n}\n".to_string(),
    ] {
        project.write(RECORD, &record);
        let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
        assert_eq!(code, 3, "{record}: {value:?}");
        assert_eq!(diagnostic_codes(&value), vec!["github_ownership_conflict"]);
        assert_eq!(project.read_string(WORKFLOW), workflow);
        assert_eq!(project.read_string(RECORD), record);
    }

    // A record edited into a different but self-consistent claim does not
    // repair anything either: the file no longer matches what it records.
    let claimed = valid.replace("ubuntu-24.04", "ubuntu-latest");
    project.write(RECORD, &claimed);
    let (code, value) = project.json(&["integrations", "github", "upgrade", "--apply"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["github_modified"]);
    assert_eq!(project.read_string(WORKFLOW), workflow);
    assert_eq!(project.read_string(RECORD), claimed);
}

#[test]
fn an_oversized_record_is_refused_without_reading_it_all() {
    let project = initialized();
    install(&project);
    project.write(RECORD, "x".repeat(200 * 1024));
    let (code, value) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(plan_str(&value, "state"), "conflict");
    let (code, refused) = project.json(&["integrations", "github", "upgrade", "--apply"]);
    assert_eq!(code, 3);
    assert_eq!(
        diagnostic_codes(&refused),
        vec!["github_ownership_conflict"]
    );
}

#[test]
fn a_record_without_its_workflow_is_an_inconsistent_pair() {
    let project = initialized();
    install(&project);
    project.remove(WORKFLOW);
    let (code, value) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(plan_str(&value, "state"), "conflict");
    let (code, refused) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 3);
    assert_eq!(
        diagnostic_codes(&refused),
        vec!["github_ownership_conflict"]
    );
    assert!(project.exists(RECORD), "the record is preserved");
}

#[test]
fn unrelated_workflows_are_reported_and_never_touched() {
    let project = initialized();
    let other = ".github/workflows/ci.yml";
    project.write(other, "name: CI\non: [push]\njobs: {}\n");
    let before = project.read_string(other);

    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0, "{value:?}");
    assert_eq!(
        strings(get(&value, &["data", "plan", "siblings"])),
        vec!["ci.yml"]
    );
    assert_eq!(project.read_string(other), before);

    let (_, preview) = project.json(&["integrations", "github", "status"]);
    assert!(
        diagnostic_codes(&preview).contains(&"github_sibling_workflows".to_string()),
        "{:?}",
        diagnostic_codes(&preview)
    );
}

// ---------------------------------------------------------------------------
// Paths and filesystem safety
// ---------------------------------------------------------------------------

#[test]
fn a_custom_path_must_be_one_direct_workflow_child() {
    let project = Project::seed();
    for path in [
        "/etc/cron.d/memoria.yml",
        "../escape.yml",
        ".github/workflows/../../escape.yml",
        ".github/workflows/nested/deep.yml",
        ".github/memoria.yml",
        ".github/workflows/memoria.txt",
        ".github/workflows/",
    ] {
        let (code, value) = project.json(&["integrations", "github", "status", "--path", path]);
        assert_eq!(code, 2, "{path}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["workflow_path_invalid"],
            "{path}"
        );
    }
}

#[test]
fn a_custom_path_installs_beside_its_own_record() {
    let project = initialized();
    let path = ".github/workflows/docs.yaml";
    let (code, value) = project.json(&[
        "integrations",
        "github",
        "install",
        "--path",
        path,
        "--apply",
    ]);
    assert_eq!(code, 0, "{value:?}");
    assert!(project.exists(path));
    assert!(project.exists(".github/memoria-workflows/docs.yaml.json"));
    // The default destination stays absent and is managed independently.
    let (_, other) = project.json(&["integrations", "github", "status"]);
    assert_eq!(plan_str(&other, "state"), "absent");
}

#[test]
fn a_symlinked_destination_or_ancestor_is_refused() {
    let project = initialized();
    fs::create_dir_all(project.root.join(".github/workflows")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", project.root.join(WORKFLOW)).unwrap();
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["github_path_unsafe"]);

    let project = initialized();
    let outside = project.home.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(project.root.join(".github")).unwrap();
    std::os::unix::fs::symlink(&outside, project.root.join(".github/workflows")).unwrap();
    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 4, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["io_error"]);
    assert!(
        fs::read_dir(&outside).unwrap().next().is_none(),
        "nothing was written outside the project"
    );
}

#[test]
fn another_memoria_writer_holding_the_lock_is_a_conflict() {
    let project = initialized();
    let lock = project.root.join(".git/memoria/github/memoria.yml.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();

    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["github_busy"]);
    assert!(!project.exists(WORKFLOW));
    drop(file);

    let (code, value) = project.json(&["integrations", "github", "install", "--apply"]);
    assert_eq!(code, 0, "{value:?}");
}

#[test]
fn a_linked_worktree_manages_its_own_workflow() {
    let project = initialized();
    project.baseline();
    // The linked worktree checks out HEAD, so the prerequisites have to be
    // committed before it exists.
    project.commit_all("baseline");
    let linked = project.home.path().join("linked");
    project.git(&[
        "worktree",
        "add",
        "--detach",
        linked.to_str().unwrap(),
        "HEAD",
    ]);
    let output = project.run_in(
        &linked,
        &[
            "integrations",
            "github",
            "install",
            "--apply",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(linked.join(WORKFLOW).is_file());
    assert!(!project.exists(WORKFLOW), "the main checkout is untouched");
    // The private coordination file belongs to the linked worktree's own
    // Git metadata directory, not to the main checkout's.
    assert!(!project.root.join(".git/memoria/github").exists());
}

// ---------------------------------------------------------------------------
// Documentation policy
// ---------------------------------------------------------------------------

#[test]
fn the_workflow_and_its_record_are_ordinary_documentation_inputs() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();

    install(&project);
    // The lock is untouched: creating a workflow is not a review.
    assert_eq!(project.state(), before);
    for path in [WORKFLOW, RECORD] {
        let (code, explain) = project.json(&["status", "--explain", path]);
        assert_eq!(code, 0, "{path}: {explain:?}");
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "selected",
            "{path} must stay an ordinary input"
        );
    }
    // New inputs make the owning README pending. Review follows the change.
    let (code, _) = project.json(&["check"]);
    assert_eq!(code, 1, "a new workflow makes documentation pending");

    project.canonical_loop();
    let (code, value) = project.json(&["check"]);
    assert_eq!(code, 0, "after review the project checks clean: {value:?}");
}

#[test]
fn the_private_coordination_files_stay_outside_documentation_inputs() {
    let project = Project::seed();
    project.baseline();
    install(&project);
    let private = project.root.join(".git/memoria/github");
    assert!(private.is_dir());
    // Git metadata is never an input, so its coordination files never reach
    // the documentation inventory.
    let (code, value) = project.json(&[
        "status",
        "--explain",
        ".git/memoria/github/memoria.yml.lock",
    ]);
    assert_eq!(code, 0, "{value:?}");
    assert_ne!(
        get_str(&value, &["data", "explanation", "outcome"]),
        "selected"
    );
}

#[test]
fn nothing_is_acknowledged_automatically() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    install(&project);
    let (code, _) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(project.state(), before, "no command acknowledged anything");
}

// ---------------------------------------------------------------------------
// Progress and human output
// ---------------------------------------------------------------------------

#[test]
fn an_apply_reports_its_writes_before_it_performs_them() {
    let project = initialized();
    let output = project.run(&["integrations", "github", "install", "--apply"]);
    assert_eq!(output.status.code(), Some(0));
    let notes = stderr(&output);
    assert!(notes.contains("integrations github install"), "{notes}");
    assert!(notes.contains(WORKFLOW), "{notes}");
}

#[test]
fn the_human_preview_shows_the_exact_proposed_bytes() {
    let project = Project::seed();
    let text = stdout(&project.run(&["integrations", "github", "install"]));
    assert!(
        text.contains("proposed .github/workflows/memoria.yml"),
        "{text}"
    );
    assert!(text.contains("name: Memoria documentation"), "{text}");
    assert!(
        text.contains("Run `memoria integrations github install --apply`"),
        "{text}"
    );
}
