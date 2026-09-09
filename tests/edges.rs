//! Discovery, paths, selection, state, and exit-status edge cases.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

#[test]
fn commands_work_from_subdirectories_and_with_explicit_root() {
    let project = Project::seed();
    project.baseline();
    let nested = project.root.join("src/retrieval/naive");
    let output = project.run_in(&nested, &["status", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let value = parse_json(&output.stdout);
    assert_eq!(get_u64(&value, &["data", "readmes"]), 6);
    let root = project.root.to_str().unwrap().to_string();
    let output = project.run_in(&nested, &["--root", &root, "check"]);
    assert_eq!(output.status.code(), Some(0));
    let output = project.run_in(
        &nested,
        &[
            "--root",
            nested.to_str().unwrap(),
            "check",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["root_mismatch"]
    );
    // Document paths stay project-root-relative from a subdirectory.
    project.append("src/retrieval/naive/search.rs", "// edit\n");
    let output = project.run_in(
        &nested,
        &[
            "review",
            "src/retrieval/naive/README.md",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let output = project.run_in(&nested, &["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn no_git_missing_config_and_unborn_head() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(memoria_bin())
        .current_dir(dir.path())
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["git_unavailable"]
    );

    let project = Project::empty_repo();
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&status), vec!["configuration_missing"]);
    // Apply refuses before any write while the root README is missing.
    let (code, refused) = project.json(&["init", "--apply"]);
    assert_eq!(code, 1, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["root_readme_missing"]);
    assert!(!project.exists("memoria.toml"));
    assert!(!project.exists("memoria.lock"));
    // With an authored root README, apply creates only the two committed files.
    project.write("README.md", "# Root\n\nAuthored before Memoria writes.\n");
    let (code, init) = project.json(&["init", "--apply"]);
    assert_eq!(code, 0, "{init:?}");
    assert_eq!(
        strings(get(&init, &["data", "created"])),
        vec!["memoria.toml", "memoria.lock"]
    );
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);
    project.ack_ok("README.md");
    let state = project.inspect_state();
    assert!(matches!(
        get(&state, &["reviews", "README.md", "git", "base_commit"]),
        Json::Null
    ));
    assert_eq!(project.json(&["check"]).0, 0);
    // Apply is idempotent and does not repair invalid configuration silently.
    let (code, again) = project.json(&["init", "--apply"]);
    assert_eq!(code, 0);
    assert!(strings(get(&again, &["data", "created"])).is_empty());
    project.write("memoria.toml", "version = 3\n");
    let (code, bad) = project.json(&["init", "--apply"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&bad), vec!["configuration_invalid"]);
}

#[test]
fn sparse_checkout_and_nested_repositories() {
    let project = Project::seed();
    project.baseline();
    project.git(&["config", "core.sparseCheckout", "true"]);
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 4);
    assert_eq!(diagnostic_codes(&status), vec!["git_unsupported"]);
    project.git(&["config", "core.sparseCheckout", "false"]);

    // A nested repository is an opaque boundary.
    fs::create_dir_all(project.root.join("vendor/lib")).unwrap();
    fs::write(project.root.join("vendor/lib/code.rs"), "fn x() {}\n").unwrap();
    project.git_in(&project.root.join("vendor/lib"), &["init", "-q"]);
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&status, &["data", "boundaries"])),
        vec!["vendor/lib"]
    );
    assert_eq!(get_u64(&status, &["data", "selected_files"]), 7);
    let (_, explain) = project.json(&["status", "--explain", "vendor/lib/code.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "boundary"
    );
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn tracked_ignored_files_stay_eligible_and_untracked_ignored_do_not() {
    let project = Project::seed();
    project.baseline();
    project.write("ignored-output/report.txt", "tracked despite ignore\n");
    project.git(&["add", "-f", "ignored-output/report.txt"]);
    let (_, explain) = project.json(&["status", "--explain", "ignored-output/report.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected"
    );
    assert_eq!(
        get_str(&explain, &["data", "explanation", "owner"]),
        "README.md"
    );
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    let (_, explain) = project.json(&["status", "--explain", "ignored-output/cache.bin"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "git-ignored"
    );
    assert!(get_str(&explain, &["data", "explanation", "reason"]).contains(".gitignore"));
    // A Memoria ignore excludes the tracked file again.
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("version = 2\n", "version = 2\ninclude = []\n"),
    );
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("ignore = [\n", "ignore = [\n    \"ignored-output/**\",\n"),
    );
    let (_, explain) = project.json(&["status", "--explain", "ignored-output/report.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "excluded"
    );
}

#[test]
fn unusual_paths_and_symlinks() {
    let project = Project::seed();
    project.baseline();
    project.write("src/corpus/with space é.rs", "pub fn unicode() {}\n");
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["input_changed"]
    );
    let (_, explain) = project.json(&["status", "--explain", "src/corpus/with space é.rs"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected"
    );
    project.ack_ok("src/corpus/README.md");

    // A selected symlink fails with its path; an excluded symlink does not.
    std::os::unix::fs::symlink("types.rs", project.root.join("src/corpus/link.rs")).unwrap();
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let bad = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "path_unsupported")
        .unwrap();
    assert_eq!(get_str(bad, &["path"]), "src/corpus/link.rs");
    project.write(
        "src/corpus/README.memoria.toml",
        "ignore = [\n    \"link.rs\",\n]\n",
    );
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);

    // Non-UTF-8 names are rejected explicitly.
    use std::os::unix::ffi::OsStrExt;
    let bad_name = std::ffi::OsStr::from_bytes(b"src/corpus/bad\xff.rs");
    fs::write(project.root.join(bad_name), "x").unwrap();
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"path_invalid".to_string()));
    fs::remove_file(project.root.join(bad_name)).unwrap();

    // Absolute and escaping import paths are rejected.
    project.append(
        "src/corpus/README.md",
        "\n<!-- memoria:import src=\"/README.md#summary\" -->\n<!-- /memoria:import -->\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"import_invalid".to_string()));
}

#[test]
fn markdown_edge_cases() {
    let project = Project::seed();
    project.baseline();
    // CRLF README with an export keeps exact bytes.
    project.write("src/corpus/README.md", "# Corpus\r\n\r\n<!-- memoria:export id=\"summary\" -->\r\nA corpus.\r\n<!-- /memoria:export -->\r\n");
    let (code, _) = project.json(&["lint"]);
    assert_eq!(code, 0);
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        vec!["document_changed"]
    );
    project.ack_ok("src/corpus/README.md");
    // Markers inside fenced code are literal.
    project.append(
        "src/corpus/README.md",
        "\r\n```\r\n<!-- memoria:import src=\"nothing/README.md#x\" -->\r\n```\r\n",
    );
    assert_eq!(project.json(&["lint"]).0, 0);
    // Forbidden export links fail lint.
    project.append("src/corpus/README.md", "\r\n<!-- memoria:export id=\"more\" -->\r\n[rel](../README.md)\r\n<!-- /memoria:export -->\r\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"export_invalid".to_string()));
    // A sidecar without a README fails.
    project.write(
        "src/corpus/README.md",
        "# Corpus\n\n<!-- memoria:export id=\"summary\" -->\nA corpus.\n<!-- /memoria:export -->\n",
    );
    project.write("src/orphan/README.memoria.toml", "ignore = []\n");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"sidecar_orphan".to_string()));
    fs::remove_dir_all(project.root.join("src/orphan")).unwrap();
    // Missing root README is a coverage failure.
    let root_readme = project.read("README.md");
    project.remove("README.md");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    let codes = diagnostic_codes(&lint);
    assert!(codes.contains(&"root_readme_missing".to_string()));
    assert!(codes.contains(&"coverage_unowned".to_string()));
    project.write("README.md", root_readme);
}

#[test]
fn state_corruption_is_never_reset() {
    let project = Project::seed();
    project.baseline();
    let good = project.state();
    let flip = |source: &[u8], index: usize| -> Vec<u8> {
        let mut bytes = source.to_vec();
        bytes[index] ^= 0x01;
        bytes
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("empty", Vec::new(), "state_corrupt"),
        ("text", b"not a lock file".to_vec(), "state_corrupt"),
        ("magic", flip(&good, 1), "state_corrupt"),
        (
            "version",
            {
                let mut bytes = good.clone();
                bytes[4] = 9;
                bytes
            },
            "state_unsupported_schema",
        ),
        (
            "codec",
            {
                let mut bytes = good.clone();
                bytes[5] = 7;
                bytes
            },
            "state_unsupported_codec",
        ),
        ("body", flip(&good, good.len() / 2), "state_corrupt"),
        ("checksum", flip(&good, good.len() - 1), "state_corrupt"),
        (
            "truncated",
            good[..good.len() - 1].to_vec(),
            "state_corrupt",
        ),
    ];
    for (name, bytes, expected) in cases {
        project.write("memoria.lock", &bytes);
        let (code, status) = project.json(&["status"]);
        assert_eq!(code, 4, "{name}");
        let codes = diagnostic_codes(&status);
        assert!(codes.iter().all(|c| c == expected), "{name}: {codes:?}");
        let (code, _) = project.json(&[
            "invalidate",
            "all",
            "--reason",
            "Should not reset the state file.",
        ]);
        assert_eq!(code, 4, "{name}");
        assert_eq!(
            project.state(),
            bytes,
            "{name}: state must not be rewritten"
        );
        let (code, _) = project.json(&["init", "--apply"]);
        assert_eq!(code, 4, "{name}");
        assert_eq!(project.state(), bytes);
        // Read-only inspection reports the same diagnostic and repairs nothing.
        let (code, inspect) = project.json(&["state", "inspect"]);
        assert_eq!(code, 4, "{name}");
        assert_eq!(diagnostic_codes(&inspect), vec![expected], "{name}");
        assert_eq!(project.state(), bytes);
    }
    project.write("memoria.lock", &good);
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn legacy_and_dual_state_paths_require_clean_cutover() {
    let project = Project::seed();
    project.baseline();
    const LEGACY: &str = ".memoria/state.json";
    let good = project.state();
    // Both paths present: ambiguous, even before either file is decoded.
    project.write(LEGACY, b"{\"schema_version\":1}");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 4, "{status:?}");
    assert_eq!(diagnostic_codes(&status), vec!["state_ambiguous"]);
    // The ambiguity holds even when the legacy file is corrupt.
    project.write(LEGACY, b"not json at all");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 4, "{status:?}");
    assert_eq!(diagnostic_codes(&status), vec!["state_ambiguous"]);
    // A lone legacy file names the cutover, and nothing is migrated.
    std::fs::remove_file(project.root.join("memoria.lock")).unwrap();
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 4, "{status:?}");
    assert_eq!(diagnostic_codes(&status), vec!["state_legacy"]);
    assert!(!project.exists("memoria.lock"), "no state was created");
    // Removing the legacy file restores the ordinary missing-state behavior.
    std::fs::remove_file(project.root.join(LEGACY)).unwrap();
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(
        get(&status, &["data", "reviews", "never_reviewed"]),
        &Json::Number(6)
    );
    project.write("memoria.lock", &good);
    assert_eq!(project.json(&["check"]).0, 0);
}

#[test]
fn usage_and_exit_status_contract() {
    let project = Project::seed();
    project.baseline();
    let output = project.run(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).starts_with(&format!("memoria {}", env!("CARGO_PKG_VERSION"))),
        "{}",
        stdout(&output)
    );
    let output = project.run(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    for command in [
        "init",
        "status",
        "lint",
        "review",
        "render",
        "ack",
        "invalidate",
        "check",
        "graph",
        "agent",
    ] {
        assert!(stdout(&output).contains(command), "help lists {command}");
    }
    let output = project.run(&["bogus"]);
    assert_eq!(output.status.code(), Some(2));
    let output = project.run(&["ack", "README.md"]);
    assert_eq!(output.status.code(), Some(2));
    let (code, value) = project.json(&[
        "invalidate",
        "everything",
        "--reason",
        "A perfectly valid reason.",
    ]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["scope_invalid"]);
    let (code, value) = project.json(&["invalidate", "all", "--reason", "short"]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["reason_invalid"]);
    let (code, value) = project.json(&["review", "src/corpus/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["review_not_pending"]);
    let (code, value) = project.json(&["review", "src/nothing/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["document_not_found"]);
    let (code, value) = project.json(&["render", "src/nothing/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["document_not_found"]);
    // JSON envelope shape. The release has one clean cutover, so every
    // envelope advertises schema version 2, on success and on failure.
    let (_, value) = project.json(&["status"]);
    assert_eq!(get_u64(&value, &["schema_version"]), 2);
    assert_eq!(get_str(&value, &["command"]), "status");
    assert!(get_bool(&value, &["ok"]));
    assert!(matches!(get(&value, &["diagnostics"]), Json::Array(_)));
    let (_, failure) = project.json(&["review", "src/nothing/README.md"]);
    assert_eq!(get_u64(&failure, &["schema_version"]), 2);
    assert!(!get_bool(&failure, &["ok"]));
    for command in [
        vec!["guidance"],
        vec!["state", "inspect"],
        vec!["status", "--summary"],
        vec!["lint"],
        vec!["check"],
        vec!["graph"],
        vec!["review"],
        vec!["init"],
    ] {
        let (_, value) = project.json(&command);
        assert_eq!(
            get_u64(&value, &["schema_version"]),
            2,
            "{command:?}: every envelope uses schema version 2"
        );
    }
    // Human diagnostics go to stderr; stdout stays clean for JSON.
    let output = project.run(&["lint"]);
    assert!(stderr(&output).contains("hint [missing_import_hint]"));
    assert!(!stdout(&output).contains("missing_import_hint"));
    let output = project.run(&["lint", "--format", "json"]);
    assert!(stderr(&output).is_empty());
}

#[test]
fn state_file_permissions_and_durability() {
    let project = Project::seed();
    project.baseline();
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(project.root.join("memoria.lock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode & 0o077, 0, "state is private by default: {mode:o}");
    // A custom mode is preserved across replacements.
    fs::set_permissions(
        project.root.join("memoria.lock"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    project.json(&[
        "invalidate",
        "all",
        "--reason",
        "Check permissions are preserved.",
    ]);
    let mode = fs::metadata(project.root.join("memoria.lock"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o640);
    // A write failure leaves the previous state intact and no temporary files.
    let before = project.state();
    fs::set_permissions(&project.root, fs::Permissions::from_mode(0o500)).unwrap();
    let (code, value) = project.json(&[
        "invalidate",
        "all",
        "--reason",
        "This write must fail safely.",
    ]);
    fs::set_permissions(&project.root, fs::Permissions::from_mode(0o755)).unwrap();
    if code != 0 {
        assert_eq!(code, 4);
        assert!(diagnostic_codes(&value).iter().all(|c| c == "io_error"));
        assert_eq!(project.state(), before);
        let leftovers: Vec<String> = fs::read_dir(&project.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".memoria.lock.tmp."))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
