//! Regression tests for the round-1 triage findings (MEM-001 … MEM-018).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use common::*;
use memoria_infrastructure::json::Json;

fn ack_json(project: &Project, document: &str, packet: &Path, token: &str) -> std::process::Output {
    project.run(&[
        "ack",
        document,
        "--packet",
        packet.to_str().unwrap(),
        "--token",
        token,
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ])
}

/// A `git` shim on PATH that runs `mutation` (a shell snippet) whenever the
/// arguments contain `trigger`, then executes the real git.
fn git_shim(dir: &Path, trigger: &str, mutation: &str) -> std::path::PathBuf {
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    let real = real.trim();
    let shim = dir.join("git");
    fs::write(
        &shim,
        format!("#!/bin/sh\ncase \" $* \" in *\" {trigger} \"*) {mutation} ;; esac\nexec {real} \"$@\"\n"),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    dir.to_path_buf()
}

// MEM-001
#[test]
fn symlink_ancestors_cannot_read_or_write_outside_the_project() {
    let project = Project::seed();
    project.write("lib/util.rs", "pub fn util() {}\n");
    project.commit_all("lib");
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("util.rs"), "OUTSIDE CONTENT\n").unwrap();
    fs::remove_dir_all(project.root.join("lib")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root.join("lib")).unwrap();
    // The tracked descendant is selected, so the symlink ancestor is an error.
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let unsupported: Vec<&str> = diagnostics
        .iter()
        .filter(|d| get_str(d, &["code"]) == "path_unsupported")
        .map(|d| get_str(d, &["path"]))
        .collect();
    assert_eq!(
        unsupported,
        vec!["lib", "lib/util.rs"],
        "the symlink and the tracked descendant behind it are both rejected"
    );
    let output = project.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!stdout(&output).contains("OUTSIDE CONTENT"));
    // Excluding the symlink and its tree keeps them harmless.
    project.write(
        "memoria.yml",
        project
            .read_string("memoria.yml")
            .replace("ignore:\n", "ignore:\n  - \"lib\"\n  - \"lib/**\"\n"),
    );
    assert_eq!(project.json(&["lint"]).0, 0);
    assert_eq!(
        fs::read_to_string(outside.path().join("util.rs")).unwrap(),
        "OUTSIDE CONTENT\n"
    );

    // Instruction files and READMEs behind a symlink ancestor are rejected, and render writes nothing outside.
    let project = Project::seed();
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("writing.md"), "# Outside rules\n").unwrap();
    fs::remove_dir_all(project.root.join(".agents")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root.join(".agents")).unwrap();
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"instruction_file_invalid".to_string()));
    let project = Project::seed();
    project.baseline();
    let outside = tempfile::tempdir().unwrap();
    for name in ["README.md", "runner.rs"] {
        fs::copy(
            project.root.join("src/execution").join(name),
            outside.path().join(name),
        )
        .unwrap();
    }
    fs::remove_dir_all(project.root.join("src/execution")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root.join("src/execution")).unwrap();
    let before = fs::read(outside.path().join("README.md")).unwrap();
    project.write("src/corpus/README.md", "# Corpus\n\n<!-- memoria:export id=\"summary\" -->\nChanged corpus.\n<!-- /memoria:export -->\n");
    let (code, render) = project.json(&["render"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&render).contains(&"path_unsupported".to_string()));
    assert_eq!(fs::read(outside.path().join("README.md")).unwrap(), before);

    // A symlinked state directory blocks every mutation and leaves the target untouched.
    let project = Project::seed();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root.join(".memoria")).unwrap();
    let (code, init) = project.json(&["init"]);
    assert_eq!(code, 4);
    assert!(
        diagnostic_codes(&init)
            .iter()
            .all(|c| c == "io_error" || c == "state_unreadable"),
        "{init:?}"
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    let (code, _) = project.json(&[
        "invalidate",
        "all",
        "--reason",
        "Should never touch the symlinked target.",
    ]);
    assert_eq!(code, 4);
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

// MEM-003
#[test]
fn read_only_commands_never_run_configured_git_hooks() {
    let project = Project::seed();
    project.baseline();
    let hook = project.root.join(".git/fsmonitor-test");
    let sentinel = project.root.join(".git/fsmonitor-called");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf called >> \"{}\"\nprintf 'token\\0'\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    project.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    for args in [
        vec!["status"],
        vec!["lint"],
        vec!["review"],
        vec!["check"],
        vec!["graph"],
    ] {
        let mut json_args = args.clone();
        json_args.extend(["--format", "json"]);
        let output = project.run(&json_args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stdout(&output)
        );
    }
    assert!(
        !sentinel.exists(),
        "the configured fsmonitor hook must never execute"
    );
}

// MEM-004
#[test]
fn final_validation_rejects_a_consumer_whose_provider_became_pending() {
    let project = Project::seed();
    project.baseline();
    project.append("README.md", "\nMore root prose.\n");
    let (packet, token) = project.review_packet("README.md");
    let before = project.state();
    let shim_dir = tempfile::tempdir().unwrap();
    let provider_source = project.root.join("src/execution/runner.rs");
    let path_dir = git_shim(
        shim_dir.path(),
        "status",
        &format!(
            "printf '// changed during ack\\n' >> \"{}\"",
            provider_source.display()
        ),
    );
    let path = format!(
        "{}:{}",
        path_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(memoria_bin())
        .current_dir(&project.root)
        .env("PATH", &path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "ack",
            "README.md",
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
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["dependencies_pending"]
    );
    assert_eq!(project.state(), before, "state must not change");
    assert_eq!(project.status_label("src/execution/README.md"), "pending");
    assert_eq!(project.status_label("README.md"), "pending");
    assert_eq!(
        project.waiting_on("README.md"),
        vec!["src/execution/README.md", "src/retrieval/README.md"],
        "retrieval waits transitively through naive"
    );
}

// MEM-005
#[test]
fn reference_links_defined_outside_an_export_are_rejected_before_rendering() {
    let project = Project::seed();
    project.baseline();
    project.write(
        "src/execution/README.md",
        "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee [manual][guide] and ![diagram][img].\n<!-- /memoria:export -->\n\n[guide]: ../../docs/manual.md\n[img]: ./diagram.png\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let invalid: Vec<&Json> = diagnostics
        .iter()
        .filter(|d| get_str(d, &["code"]) == "export_invalid")
        .collect();
    assert!(invalid.len() >= 2, "{lint:?}");
    let before = project.read("README.md");
    let (code, _) = project.json(&["render"]);
    assert_eq!(code, 1);
    assert_eq!(
        project.read("README.md"),
        before,
        "nothing is copied while the export is invalid"
    );
    // Collapsed and shortcut references are rejected as well.
    for body in ["[guide][]", "[guide]"] {
        project.write(
            "src/execution/README.md",
            format!("# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee {body}.\n<!-- /memoria:export -->\n\n[guide]: https://example.com/manual\n"),
        );
        let (code, lint) = project.json(&["lint"]);
        assert_eq!(code, 1, "{body}");
        assert!(
            diagnostic_codes(&lint).contains(&"export_invalid".to_string()),
            "{body}"
        );
    }
}

// MEM-006
#[test]
fn nested_repositories_stay_opaque_even_when_the_parent_tracks_their_files() {
    let project = Project::seed();
    project.write("child/README.md", "# Child\n");
    project.write("child/a.rs", "one\n");
    project.commit_all("child");
    project.baseline();
    let nested = Command::new("git")
        .arg("-C")
        .arg(project.root.join("child"))
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(nested.status.success());
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&status, &["data", "boundaries"])),
        vec!["child"]
    );
    assert_eq!(
        get_u64(&status, &["data", "readmes"]),
        6,
        "the child README is no longer a boundary of this project"
    );
    let (_, explain) = project.json(&["status", "--explain", "child/a.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "boundary"
    );
    // child/a.rs belonged to the child README, so no remaining owner changed; the dormant child review stays in state.
    assert_eq!(project.cause_codes("README.md"), Vec::<String>::new());
    assert_eq!(project.json(&["check"]).0, 0);
    assert!(
        project
            .read_string(".memoria/state.json")
            .contains("child/README.md")
    );

    // An initialized submodule is a boundary too.
    let project = Project::seed();
    project.baseline();
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
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&status, &["data", "boundaries"])),
        vec!["vendor/module"]
    );
    let (_, explain) = project.json(&["status", "--explain", "vendor/module/lib.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "boundary"
    );
}

// MEM-007
#[test]
fn deleted_tracked_readmes_and_sidecars_recalculate_ownership() {
    let project = Project::seed();
    project.baseline();
    // Unstaged deletion of a tracked README moves its files to the ancestor.
    project.remove("src/retrieval/naive/README.md");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 1, "the retrieval README imports the deleted document");
    assert!(diagnostic_codes(&status).contains(&"import_missing_document".to_string()));
    let (_, explain) = project.json(&["status", "--explain", "src/retrieval/naive/search.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "src/retrieval/README.md"
    );
    // Staging the deletion changes nothing.
    project.git(&["rm", "-q", "--cached", "src/retrieval/naive/README.md"]);
    let (code, _) = project.json(&["status"]);
    assert_eq!(code, 1);
    // A deleted README without importers simply recalculates.
    let project = Project::seed();
    project.baseline();
    project.remove("src/corpus/README.md");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(get_u64(&status, &["data", "readmes"]), 5);
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    let (_, explain) = project.json(&["status", "--explain", "src/corpus/types.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "README.md"
    );
    // Moving a README before staging.
    fs::rename(
        project.root.join("src/disconnected/README.md"),
        project.root.join("src/disconnected/moved.md"),
    )
    .unwrap();
    let (code, _) = project.json(&["status"]);
    assert_eq!(code, 0);
    let (_, explain) = project.json(&["status", "--explain", "src/disconnected/item.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "README.md"
    );
    // A deleted sidecar drops its rules; the fixture becomes excluded again.
    let project = Project::seed();
    project.baseline();
    project.remove("src/retrieval/README.memoria.yml");
    let (code, _) = project.json(&["status"]);
    assert_eq!(code, 0);
    let (_, explain) = project.json(&["status", "--explain", "src/retrieval/fixtures/sample.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "excluded"
    );
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );
    // Real orphan sidecars are still errors.
    project.write("src/orphan/README.memoria.yml", "ignore: []\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"sidecar_orphan".to_string()));
}

// MEM-008
#[test]
fn ignored_gitignore_files_still_change_policy() {
    let project = Project::seed();
    project.write("src/execution/.gitignore", ".gitignore\n");
    project.baseline();
    let (_, explain) = project.json(&["status", "--explain", "src/execution/.gitignore"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "git-ignored"
    );
    project.write("src/execution/.gitignore", ".gitignore\nfuture-output/\n");
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        Vec::<String>::new()
    );
    assert_eq!(project.json(&["check"]).0, 1);
    // A self-ignored root .gitignore affects every owner.
    let project = Project::seed();
    project.write(".gitignore", "ignored-output/**\n.gitignore\n");
    project.baseline();
    project.write(
        ".gitignore",
        "ignored-output/**\n.gitignore\nfuture-output/\n",
    );
    for doc in ["README.md", "src/corpus/README.md"] {
        assert_eq!(project.cause_codes(doc), vec!["input_changed"], "{doc}");
    }
}

// MEM-009
#[test]
fn relative_global_excludes_resolve_from_the_worktree_root() {
    let project = Project::seed();
    project.write(".git/local-excludes", "future-one/\n");
    project.git(&["config", "core.excludesFile", ".git/local-excludes"]);
    let elsewhere = tempfile::tempdir().unwrap();
    let root = project.root.to_str().unwrap().to_string();
    // Baseline through --root from an unrelated directory.
    let output = project.run_in(elsewhere.path(), &["--root", &root, "init"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        project
            .run_in(elsewhere.path(), &["--root", &root, "render"])
            .status
            .code(),
        Some(0)
    );
    for _ in 0..8 {
        let output = project.run_in(
            elsewhere.path(),
            &["--root", &root, "review", "--format", "json"],
        );
        let plan = parse_json(&output.stdout);
        let Json::String(next) = get(&plan, &["data", "next_ready"]) else {
            break;
        };
        let output = project.run_in(
            elsewhere.path(),
            &["--root", &root, "review", next, "--format", "json"],
        );
        let packet = elsewhere.path().join("packet.json");
        fs::write(&packet, &output.stdout).unwrap();
        let token = get_str(&parse_json(&output.stdout), &["data", "token"]).to_string();
        let output = project.run_in(
            elsewhere.path(),
            &[
                "--root",
                &root,
                "ack",
                next,
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
            ],
        );
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    }
    let subdir = project.root.join("src/corpus");
    for cwd in [elsewhere.path(), project.root.as_path(), subdir.as_path()] {
        assert_eq!(
            project
                .run_in(cwd, &["--root", &root, "check"])
                .status
                .code(),
            Some(0),
            "{}",
            cwd.display()
        );
    }
    assert_eq!(project.run_in(&subdir, &["check"]).status.code(), Some(0));
    project.write(".git/local-excludes", "future-two/\n");
    for cwd in [elsewhere.path(), project.root.as_path(), subdir.as_path()] {
        let output = project.run_in(cwd, &["--root", &root, "check", "--format", "json"]);
        assert_eq!(output.status.code(), Some(1), "{}", cwd.display());
        assert!(
            diagnostic_codes(&parse_json(&output.stdout)).contains(&"review_pending".to_string())
        );
    }
    // Policy identities never embed the absolute exclude path: an identical project elsewhere hashes the same policy.
    let (packet_a, _) = project.review_packet("src/corpus/README.md");
    let twin = Project::seed();
    twin.write(".git/local-excludes", "future-two/\n");
    twin.git(&["config", "core.excludesFile", ".git/local-excludes"]);
    assert_eq!(twin.run(&["init"]).status.code(), Some(0));
    assert_eq!(twin.run(&["render"]).status.code(), Some(0));
    let (packet_b, _) = twin.review_packet("src/corpus/README.md");
    let hash_of = |packet: &Path| {
        get_str(
            &parse_json(&fs::read(packet).unwrap()),
            &["data", "manifest", "policy_hash"],
        )
        .to_string()
    };
    assert_eq!(hash_of(&packet_a), hash_of(&packet_b));
}

// MEM-010
#[test]
fn custom_in_project_skill_packages_are_guidance_not_sources() {
    let project = Project::seed();
    project.baseline();
    let parent = project.root.join("custom-skills");
    let (code, _) = project.json(&[
        "agent",
        "install",
        "--target",
        "codex",
        "--path",
        parent.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    assert!(parent.join("memoria/SKILL.md").exists());
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 0, "{check:?}");
    for path in [
        "custom-skills/memoria/SKILL.md",
        "custom-skills/memoria/.memoria-install.json",
        "custom-skills/memoria.install.lock",
    ] {
        let (_, explain) = project.json(&["status", "--explain", path]);
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "excluded",
            "{path}"
        );
        assert!(
            get_str(&explain, &["data", "explanation", "reason"]).contains("skill-package"),
            "{path}"
        );
    }
    // Unrelated files under the same parent remain ordinary sources.
    project.write("custom-skills/notes.md", "my notes\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    let (_, explain) = project.json(&["status", "--explain", "custom-skills/notes.md"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected"
    );
    // Review state and installation state stay separate.
    assert!(
        !project
            .read_string(".memoria/state.json")
            .contains("custom-skills")
    );
}

// MEM-011
#[test]
fn impossible_counters_are_rejected_before_any_write() {
    let project = Project::seed();
    project.baseline();
    let good = project.read_string(".memoria/state.json");
    let cases = [
        (
            "zero",
            good.replace("\"next_invalidation_id\": 1", "\"next_invalidation_id\": 0"),
        ),
        (
            "exhausted",
            good.replace(
                "\"next_invalidation_id\": 1",
                "\"next_invalidation_id\": 18446744073709551615",
            ),
        ),
        (
            "revision",
            good.replace(
                "\"revision\": 6,\n  \"schema_version\"",
                "\"revision\": 0,\n  \"schema_version\"",
            ),
        ),
    ];
    for (name, bytes) in cases {
        assert_ne!(bytes, good, "{name}: fixture must change the state");
        project.write(".memoria/state.json", &bytes);
        let (code, status) = project.json(&["status"]);
        assert_eq!(code, 4, "{name}: {status:?}");
        assert_eq!(diagnostic_codes(&status), vec!["state_corrupt"], "{name}");
        let (code, _) = project.json(&[
            "invalidate",
            "all",
            "--reason",
            "Should be refused before writing.",
        ]);
        assert_eq!(code, 4, "{name}");
        assert_eq!(
            project.read_string(".memoria/state.json"),
            bytes,
            "{name}: bytes untouched"
        );
    }
    project.write(".memoria/state.json", &good);
    assert_eq!(project.json(&["check"]).0, 0);
}

// MEM-012
#[test]
fn changed_imports_are_reported_with_exact_differences_and_text() {
    let project = Project::seed();
    project.baseline();
    project.append("README.md", "\nRoot prose edit.\n");
    let (packet, token) = project.review_packet("README.md");
    let before = project.state();
    let execution = project.read_string("src/execution/README.md");
    project.write(
        "src/execution/README.md",
        execution.replace("one at a time.", "one at a time, carefully."),
    );
    // Before the provider is acknowledged: the import difference wins over readiness.
    let output = ack_json(&project, "README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    let import = changes
        .iter()
        .find(|c| get_str(c, &["kind"]) == "import")
        .expect("import change listed");
    assert_eq!(
        get_str(import, &["identity"]),
        "src/execution/README.md#summary"
    );
    assert_eq!(get_str(import, &["change"]), "changed");
    assert!(get_u64(import, &["before_bytes"]) < get_u64(import, &["after_bytes"]));
    assert_ne!(
        get_str(import, &["before_hash"]),
        get_str(import, &["after_hash"])
    );
    assert!(
        get_str(import, &["diff"]).contains("+")
            && get_str(import, &["diff"]).contains("carefully"),
        "{}",
        get_str(import, &["diff"])
    );
    assert_eq!(project.state(), before);
    // After the provider is acknowledged the same packet is still rejected with the same facts.
    project.ack_ok("src/execution/README.md");
    let output = ack_json(&project, "README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    let import = changes
        .iter()
        .find(|c| get_str(c, &["kind"]) == "import")
        .unwrap();
    assert!(get_str(import, &["diff"]).contains("carefully"));
    // Human output carries the same facts (MEM-013).
    let output = project.run(&[
        "ack",
        "README.md",
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
    ]);
    assert_eq!(output.status.code(), Some(3));
    let err = stderr(&output);
    assert!(err.contains("src/execution/README.md#summary"), "{err}");
    assert!(
        err.contains("before_hash=") && err.contains("after_bytes="),
        "{err}"
    );
    assert!(err.contains("+") && err.contains("carefully"), "{err}");
}

// MEM-013
#[test]
fn human_cycle_diagnostics_list_every_edge_location() {
    let project = Project::seed();
    project.baseline();
    project.append(
        "src/execution/README.md",
        "\n<!-- memoria:import src=\"../../README.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    let output = project.run(&["lint"]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("import_cycle"), "{err}");
    assert!(
        err.contains(
            "export_id=summary importer=README.md line=23 provider=src/execution/README.md"
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "export_id=summary importer=src/execution/README.md line=11 provider=README.md"
        ),
        "{err}"
    );
    assert!(err.contains("edges:") && err.contains("path:"), "{err}");
}

// MEM-014
#[test]
fn stdout_failures_are_io_errors() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    for args in [
        vec!["status", "--format", "json"],
        vec!["review", "src/execution/README.md", "--format", "json"],
        vec!["status"],
    ] {
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
    // A closed pipe is the same I/O failure.
    let mut child = project
        .command(
            &project.root,
            &["review", "src/execution/README.md", "--format", "json"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(4));
    // A usage error with an unwritable stdout is also an I/O failure.
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = project
        .command(&project.root, &["ack", "README.md", "--format", "json"])
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
}

// MEM-015
#[test]
fn usage_errors_honor_requested_json_output() {
    let project = Project::seed();
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("ack", vec!["ack", "README.md", "--format", "json"]),
        (
            "invalidate",
            vec![
                "invalidate",
                "all",
                "--reason",
                "one reason here",
                "--reason",
                "two reasons here",
                "--format",
                "json",
            ],
        ),
        ("unknown", vec!["bogus", "--format=json"]),
        (
            "agent",
            vec!["--format", "json", "agent", "install", "--target", "gemini"],
        ),
        (
            "review",
            vec!["review", "--max-bytes", "many", "--format", "json"],
        ),
    ];
    for (command, args) in cases {
        let output = project.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let value = parse_json(&output.stdout);
        assert_eq!(get_u64(&value, &["schema_version"]), 1);
        assert_eq!(get_str(&value, &["command"]), command, "{args:?}");
        assert!(!get_bool(&value, &["ok"]));
        assert_eq!(diagnostic_codes(&value), vec!["usage_error"], "{args:?}");
        assert!(matches!(get(&value, &["data"]), Json::Null));
    }
    // Human mode keeps clap's text; help and version keep exit 0.
    let output = project.run(&["ack", "README.md"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("--packet"));
    assert_eq!(
        project.run(&["--help", "--format", "json"]).status.code(),
        Some(0)
    );
    assert_eq!(
        project
            .run(&["--version", "--format", "json"])
            .status
            .code(),
        Some(0)
    );
}

// MEM-016
#[test]
fn canonical_next_action_reaches_render_required_tasks() {
    let project = Project::seed();
    project.baseline();
    let naive = project.read_string("src/retrieval/naive/README.md");
    project.write(
        "src/retrieval/naive/README.md",
        naive.replace("in order.", "in order, slowly."),
    );
    let steps = project.canonical_loop();
    assert_eq!(
        steps,
        vec![
            "ack src/retrieval/naive/README.md",
            "render src/retrieval/README.md",
            "ack src/retrieval/README.md"
        ]
    );
    assert_eq!(project.json(&["check"]).0, 0);
    // The plan exposes the render step explicitly while still refusing a packet.
    let retrieval = project.read_string("src/retrieval/README.md");
    project.write(
        "src/retrieval/README.md",
        retrieval.replace("relevant to a query.", "relevant to a query quickly."),
    );
    project.ack_ok("src/retrieval/README.md");
    let plan = project.plan();
    assert_eq!(get_str(&plan, &["data", "next_ready"]), "README.md");
    assert_eq!(get_str(&plan, &["data", "next_action", "kind"]), "render");
    assert_eq!(
        get_str(&plan, &["data", "next_action", "document"]),
        "README.md"
    );
    let (code, refused) = project.json(&["review", "README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["imports_outdated"]);
    let output = project.run(&["review"]);
    assert!(stdout(&output).contains("Next: memoria render README.md"));
    assert_eq!(
        project.canonical_loop(),
        vec!["render README.md", "ack README.md"]
    );
    assert_eq!(project.json(&["check"]).0, 0);
}

// MEM-018: unreadable and special selected files, packet read failures, stale policy and file sets.
#[test]
fn unreadable_and_special_files_are_reported() {
    let project = Project::seed();
    project.baseline();
    let secret = project.root.join("src/corpus/types.rs");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    let (code, status) = project.json(&["status"]);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    if code != 0 {
        assert_eq!(code, 4);
        assert_eq!(diagnostic_codes(&status), vec!["io_error"]);
    }
    // Git never lists special files, so a FIFO is not eligible and never read.
    let fifo = project.root.join("src/corpus/pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);
    let (_, explain) = project.json(&["status", "--explain", "src/corpus/pipe"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "not-eligible"
    );
    fs::remove_file(&fifo).unwrap();
    // A special file reached through Git eligibility (a symlink to it) is rejected as unsupported.
    let (code, lint) = {
        let fifo = project.root.join("pipe");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        std::os::unix::fs::symlink("pipe", project.root.join("src/corpus/pipe-link")).unwrap();
        let result = project.json(&["lint"]);
        fs::remove_file(project.root.join("src/corpus/pipe-link")).unwrap();
        fs::remove_file(&fifo).unwrap();
        result
    };
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"path_unsupported".to_string()));
}

#[test]
fn packet_read_failures_are_transport_errors() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let (packet, token) = project.review_packet("src/execution/README.md");
    fs::set_permissions(&packet, fs::Permissions::from_mode(0o000)).unwrap();
    let output = ack_json(&project, "src/execution/README.md", &packet, &token);
    fs::set_permissions(&packet, fs::Permissions::from_mode(0o600)).unwrap();
    if output.status.code() != Some(0) {
        assert_eq!(output.status.code(), Some(4));
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec!["packet_unreadable"]
        );
    }
    // A directory on stdin produces a read error, not a schema error.
    let dir = fs::File::open(project.packets.path()).unwrap();
    let output = project
        .command(
            &project.root,
            &[
                "ack",
                "src/execution/README.md",
                "--packet",
                "-",
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
            ],
        )
        .stdin(Stdio::from(dir))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_unreadable"]
    );
    assert_eq!(project.status_label("src/execution/README.md"), "pending");
}

#[test]
fn stale_policy_and_file_set_snapshots_are_rejected_with_exact_entries() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let (packet, token) = project.review_packet("src/execution/README.md");
    let before = project.state();
    // Policy change with an unchanged file set.
    project.append("memoria.yml", "include:\n  - \"nothing/**\"\n");
    let output = ack_json(&project, "src/execution/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    let value = parse_json(&output.stdout);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    assert_eq!(get_str(&changes[0], &["kind"]), "policy");
    assert_ne!(
        get_str(&changes[0], &["before_hash"]),
        get_str(&changes[0], &["after_hash"])
    );
    assert_eq!(project.state(), before);
    project.write(
        "memoria.yml",
        project
            .read_string("memoria.yml")
            .replace("include:\n  - \"nothing/**\"\n", ""),
    );
    // File added and removed.
    project.write("src/execution/extra.rs", "x\n");
    project.remove("src/execution/runner.rs");
    let output = ack_json(&project, "src/execution/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    let value = parse_json(&output.stdout);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
        panic!()
    };
    let listed: Vec<(String, String)> = changes
        .iter()
        .map(|c| {
            (
                get_str(c, &["change"]).to_string(),
                get_str(c, &["identity"]).to_string(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("added".to_string(), "src/execution/extra.rs".to_string()),
            ("removed".to_string(), "src/execution/runner.rs".to_string())
        ]
    );
    let removed = &changes[1];
    assert!(matches!(get(removed, &["after_bytes"]), Json::Null));
    assert!(get_str(removed, &["diff"]).contains("-// pending"));
    assert_eq!(project.state(), before);
}
