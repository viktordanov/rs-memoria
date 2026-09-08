//! Regression tests for the round-4 triage findings (MEM-012 reopened,
//! MEM-027, MEM-028, MEM-029, MEM-030).

mod common;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use common::*;
use memoria_infrastructure::json::Json;

// MEM-012 (reopened): an export change during the final validation phase is
// reported with exact differences, not hidden behind readiness.
#[test]
fn export_changes_during_final_validation_report_exact_differences() {
    let project = Project::seed();
    project.baseline();
    project.append("README.md", "\nMore root prose.\n");
    let (packet, token) = project.review_packet("README.md");
    let before = project.state();
    let shim_dir = tempfile::tempdir().unwrap();
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    let provider = project.root.join("src/execution/README.md");
    let sentinel = shim_dir.path().join("triggered");
    fs::write(
        shim_dir.path().join("git"),
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" status \"*) if [ ! -e \"{s}\" ]; then touch \"{s}\"; sed -i 's/one at a time\\./one at a time, changed during acknowledgement./' \"{p}\"; fi ;; esac\nexec {git} \"$@\"\n",
            s = sentinel.display(),
            p = provider.display(),
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
    assert!(sentinel.exists(), "the shim mutated the provider export");
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
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
        .expect("import difference listed");
    assert_eq!(
        get_str(import, &["identity"]),
        "src/execution/README.md#summary"
    );
    assert!(get_u64(import, &["before_bytes"]) < get_u64(import, &["after_bytes"]));
    assert_ne!(
        get_str(import, &["before_hash"]),
        get_str(import, &["after_hash"])
    );
    assert!(
        get_str(import, &["diff"]).contains("changed during acknowledgement"),
        "{}",
        get_str(import, &["diff"])
    );
    assert_eq!(project.state(), before, "state bytes unchanged");
    // An unchanged consumer manifest with a newly pending provider still rejects on readiness alone.
    let project = Project::seed();
    project.baseline();
    project.append("README.md", "\nMore root prose.\n");
    let (packet, token) = project.review_packet("README.md");
    let shim_dir = tempfile::tempdir().unwrap();
    let source = project.root.join("src/execution/runner.rs");
    fs::write(shim_dir.path().join("git"), format!("#!/bin/sh\ncase \" $* \" in *\" status \"*) printf '// pending\\n' >> \"{}\" ;; esac\nexec {} \"$@\"\n", source.display(), real.trim())).unwrap();
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
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["dependencies_pending"]
    );
}

// MEM-027
#[test]
fn reserved_trees_never_contain_documents() {
    for parent in [".agents/skills", "custom"] {
        let project = Project::seed();
        project.write(
            &format!("{parent}/memoria/README.md"),
            "# Old skill guide\n",
        );
        project.write(&format!("{parent}/memoria/SKILL.md"), "old guidance\n");
        project.write(
            &format!("{parent}/memoria/README.memoria.toml"),
            "ignore = []\n",
        );
        project.baseline();
        let (_, status) = project.json(&["status"]);
        let readmes = get_u64(&status, &["data", "readmes"]);
        if parent == "custom" {
            assert_eq!(
                readmes, 7,
                "an ordinary custom directory README is a document before installation"
            );
        } else {
            assert_eq!(
                readmes, 6,
                "the default guidance location is reserved even before installation"
            );
        }
        let install_parent = project.root.join(parent);
        let (code, installed) = project.json(&[
            "agent",
            "install",
            "--target",
            "codex",
            "--path",
            install_parent.to_str().unwrap(),
            "--replace-existing",
        ]);
        assert_eq!(code, 0, "{parent}: {installed:?}");
        assert!(install_parent.join("memoria.backup/README.md").exists());
        let (code, check) = project.json(&["check"]);
        assert_eq!(code, 0, "{parent}: {check:?}");
        let (_, status) = project.json(&["status"]);
        assert_eq!(
            get_u64(&status, &["data", "readmes"]),
            6,
            "{parent}: backup README is not a document"
        );
        let (_, graph) = project.json(&["graph"]);
        let Json::Array(nodes) = get(&graph, &["data", "nodes"]) else {
            panic!()
        };
        assert!(
            nodes
                .iter()
                .all(|n| !get_str(n, &["document"]).contains("memoria.backup")),
            "{parent}"
        );
        // Interrupted transaction artifacts hold no documents either.
        fs::rename(
            install_parent.join("memoria"),
            install_parent.join("memoria.removing"),
        )
        .unwrap();
        fs::write(install_parent.join("memoria.install-txn.json"), format!("{{\"schema_version\":1,\"phase\":\"removing\",\"destination\":\"{}\",\"staging\":\"{}\",\"removing\":\"{}\",\"backup\":\"memoria.backup\"}}", install_parent.join("memoria").display(), install_parent.join("memoria.staging").display(), install_parent.join("memoria.removing").display())).unwrap();
        assert_eq!(
            project.json(&["check"]).0,
            0,
            "{parent}: recovery artifacts add no work"
        );
        // Uninstall restores the backup; a restored custom README becomes a project document again.
        assert_eq!(
            project
                .json(&[
                    "agent",
                    "uninstall",
                    "--target",
                    "codex",
                    "--path",
                    install_parent.to_str().unwrap()
                ])
                .0,
            0
        );
        let (_, status) = project.json(&["status"]);
        let expected = if parent == "custom" { 7 } else { 6 };
        assert_eq!(
            get_u64(&status, &["data", "readmes"]),
            expected,
            "{parent} after uninstall"
        );
        assert_eq!(
            project.json(&["check"]).0,
            0,
            "{parent}: the restored README's review is still current"
        );
    }
    // Memoria state and Git metadata never hold documents or sidecars.
    let project = Project::seed();
    project.baseline();
    project.write(".memoria/README.md", "# Internal notes\n");
    project.write(".memoria/README.memoria.toml", "ignore = []\n");
    assert_eq!(project.json(&["check"]).0, 0);
    let (_, status) = project.json(&["status"]);
    assert_eq!(get_u64(&status, &["data", "readmes"]), 6);
    // Ordinary Memoria ignore rules still do not hide project README boundaries.
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
            .replace("ignore = [\n", "ignore = [\n    \"src/corpus/**\",\n"),
    );
    let (_, status) = project.json(&["status"]);
    assert_eq!(
        get_u64(&status, &["data", "readmes"]),
        6,
        "src/corpus/README.md remains a boundary"
    );
}

// MEM-028
#[test]
fn escaped_exclamation_before_a_link_renders() {
    let project = Project::seed();
    project.baseline();
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee \\![guide](https://example.com) now.\n<!-- /memoria:export -->\n");
    project.append("README.md", "\n[guide]: ../outside.md\n");
    assert_eq!(project.json(&["lint"]).0, 0);
    assert_eq!(project.json(&["render"]).0, 0);
    assert!(
        project
            .read_string("README.md")
            .contains("See \\![guide](https://example.com) now.")
    );
    // A real image with a relative destination is still rejected.
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee ![guide](./diagram.png) now.\n<!-- /memoria:export -->\n");
    assert_eq!(project.json(&["lint"]).0, 1);
}

// MEM-029
#[test]
fn non_utf8_arguments_are_usage_errors_not_panics() {
    let project = Project::seed();
    project.baseline();
    let before = project.tree_snapshot();
    let bad = std::ffi::OsStr::from_bytes(b"src/\xff/README.md");
    let output = Command::new(memoria_bin())
        .current_dir(&project.root)
        .arg("review")
        .arg(bad)
        .args(["--format", "json"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(get_str(&value, &["command"]), "review");
    assert_eq!(diagnostic_codes(&value), vec!["usage_error"]);
    assert!(!stderr(&output).contains("panicked"));
    let output = Command::new(memoria_bin())
        .current_dir(&project.root)
        .arg("invalidate")
        .arg(bad)
        .args(["--reason", "A perfectly valid reason."])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("not valid UTF-8"));
    assert!(!stderr(&output).contains("panicked"));
    assert_eq!(project.tree_snapshot(), before, "no project mutation");
}

// MEM-030
#[test]
fn quoted_guidance_with_colons_are_accepted_everywhere() {
    let project = Project::seed();
    project.write("memoria.toml", "version = 2\nignore = [\n    \"**/generated/**\",\n    \"**/fixtures/**\",\n]\n\n[documentation]\nguidance = [\n    \"Style: use short sentences.\",\n    \"Tone: plain words.\",\n]\nguidance_files = [\n    \".agents/writing.md\",\n]\n");
    project.write("src/retrieval/README.memoria.toml", "include = [\n    \"fixtures/**\",\n]\n\n[documentation]\nguidance = [\n    \"Retrieval: explain ranking first.\",\n]\n");
    project.baseline();
    project.append("src/retrieval/engine.rs", "// edit\n");
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&fs::read(packet).unwrap());
    let Json::Array(guidance) = get(&value, &["data", "context", "guidance", "entries"]) else {
        panic!()
    };
    let texts: Vec<&str> = guidance.iter().map(|i| get_str(i, &["text"])).collect();
    assert!(texts.contains(&"Style: use short sentences."), "{texts:?}");
    assert!(texts.contains(&"Tone: plain words."), "{texts:?}");
    assert!(
        texts.contains(&"Retrieval: explain ranking first."),
        "{texts:?}"
    );
    // The single-line array form is equivalent and does not change staleness.
    let state = project.state();
    project.write("memoria.toml", "version = 2\nignore = [\n    \"**/generated/**\",\n    \"**/fixtures/**\",\n]\n\n[documentation]\nguidance = [\"Style: use short sentences.\", 'Tone: plain words.']\nguidance_files = [\n    \".agents/writing.md\",\n]\n");
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&fs::read(packet).unwrap());
    let Json::Array(guidance) = get(&value, &["data", "context", "guidance", "entries"]) else {
        panic!()
    };
    assert!(
        guidance
            .iter()
            .any(|i| get_str(i, &["text"]) == "Style: use short sentences.")
    );
    assert_eq!(project.state(), state);
    // Table values in a string array are still rejected.
    project.write(
        "memoria.toml",
        "version = 2\n\n[documentation]\nguidance = [\n    { key = \"value\" },\n]\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&lint), vec!["configuration_invalid"]);
}
