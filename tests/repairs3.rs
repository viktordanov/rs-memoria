//! Regression tests for the round-3 triage findings (MEM-005, MEM-023,
//! MEM-024, MEM-026). MEM-025 is covered by installer fault tests in
//! `crates/memoria-infrastructure/src/skill.rs`.

mod common;

use std::fs;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::*;
use memoria_infrastructure::json::Json;

// MEM-005
#[test]
fn malformed_and_nested_inline_links_cannot_acquire_consumer_destinations() {
    for body in [
        "See [guide](not a valid URL).",
        "See [outer [guide]](https://example.com).",
        "![alt](not a valid URL)",
        "![outer [img]](https://example.com/i.png)",
    ] {
        let project = Project::seed();
        project.baseline();
        project.write("src/execution/README.md", format!("# Execution\n\n<!-- memoria:export id=\"summary\" -->\n{body}\n<!-- /memoria:export -->\n"));
        project.append(
            "README.md",
            "\n[guide]: ../outside.md\n[img]: ../outside.png\n",
        );
        let (code, lint) = project.json(&["lint"]);
        assert_eq!(code, 1, "{body}");
        assert!(
            diagnostic_codes(&lint).contains(&"export_invalid".to_string()),
            "{body}: {lint:?}"
        );
        let before = project.read("README.md");
        assert_eq!(project.json(&["render"]).0, 1, "{body}");
        assert_eq!(project.read("README.md"), before, "{body}: nothing copied");
    }
    // A complete inline link with escaped brackets and an image render unchanged.
    let project = Project::seed();
    project.baseline();
    project.write("src/execution/README.md", "# Execution\n\n<!-- memoria:export id=\"summary\" -->\nSee [guide \\[v2\\]](https://example.com/a) and ![alt](https://example.com/i.png).\n<!-- /memoria:export -->\n");
    project.append("README.md", "\n[guide]: ../outside.md\n");
    assert_eq!(project.json(&["lint"]).0, 0);
    assert_eq!(project.json(&["render"]).0, 0);
    assert!(project.read_string("README.md").contains(
        "See [guide \\[v2\\]](https://example.com/a) and ![alt](https://example.com/i.png)."
    ));
}

// MEM-023
#[test]
fn many_ignored_directories_do_not_deadlock_the_ignore_inventory() {
    let project = Project::seed();
    project.baseline();
    project.write(".gitignore", "ignored-output/**\nignored-*/\n");
    for i in 0..2000 {
        let name = format!("ignored-{i:04}{}", "x".repeat(90));
        fs::create_dir_all(project.root.join(&name)).unwrap();
        // A file inside keeps the directory visible to the walk before pruning.
        fs::write(project.root.join(&name).join("data.bin"), b"x").unwrap();
    }
    // Input and output of the batched check-ignore exchange both exceed a pipe buffer.
    let mut child = project
        .command(&project.root, &["status", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("memoria status did not finish within the deadline: pipe deadlock");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = parse_json(&output.stdout);
    assert_eq!(
        get_u64(&value, &["data", "selected_files"]),
        7,
        "ignored directories contribute no sources"
    );
    // The root ignore rule changed, so every owner's policy changed: all pending, none lost.
    assert_eq!(get_u64(&value, &["data", "reviews", "pending"]), 6);
    let (_, explain) = project.json(&[
        "status",
        "--explain",
        &format!("ignored-0000{}/data.bin", "x".repeat(90)),
    ]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "git-ignored"
    );
    // Policy edits inside those ignored directories change nothing.
    let (_, before) = project.json(&["status"]);
    fs::write(
        project
            .root
            .join(format!("ignored-0001{}", "x".repeat(90)))
            .join(".gitignore"),
        "x\n",
    )
    .unwrap();
    let (_, after) = project.json(&["status"]);
    assert_eq!(
        get(&before, &["data", "documents"]),
        get(&after, &["data", "documents"])
    );
    assert_eq!(
        project.json(&["check"]).0,
        1,
        "the root rule change itself still needs review"
    );
}

// MEM-024
#[test]
fn unrelated_memoria_prefixed_siblings_stay_owned_sources() {
    let project = Project::seed();
    project.write("custom/memoria.rs", "important source\n");
    project.write("custom/memoria.config", "setting = 1\n");
    project.write("custom/memoria.tools/x.rs", "tool\n");
    project.commit_all("custom sources");
    project.baseline();
    let owned = |path: &str| {
        let (_, explain) = project.json(&["status", "--explain", path]);
        (
            get_str(&explain, &["data", "explanation", "outcome"]).to_string(),
            get_str(&explain, &["data", "explanation", "owner"]).to_string(),
        )
    };
    for path in [
        "custom/memoria.rs",
        "custom/memoria.config",
        "custom/memoria.tools/x.rs",
    ] {
        assert_eq!(
            owned(path),
            ("selected".to_string(), "README.md".to_string()),
            "{path} before install"
        );
    }
    let parent = project.root.join("custom");
    assert_eq!(
        project
            .json(&[
                "agent",
                "install",
                "--target",
                "codex",
                "--path",
                parent.to_str().unwrap()
            ])
            .0,
        0
    );
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "installing changes no source coverage"
    );
    for path in [
        "custom/memoria.rs",
        "custom/memoria.config",
        "custom/memoria.tools/x.rs",
    ] {
        assert_eq!(
            owned(path),
            ("selected".to_string(), "README.md".to_string()),
            "{path} after install"
        );
    }
    for path in [
        "custom/memoria/SKILL.md",
        "custom/memoria/.memoria-install.json",
        "custom/memoria.install.lock",
    ] {
        let (_, explain) = project.json(&["status", "--explain", path]);
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "excluded",
            "{path}"
        );
    }
    // Real edits to the siblings become stale input.
    project.append("custom/memoria.rs", "// changed\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    assert_eq!(project.json(&["check"]).0, 1);
    project.ack_ok("README.md");
    // New siblings after installation are sources; a simulated interrupted transaction changes nothing for them.
    project.write("custom/memoria.notes", "new sibling\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    project.ack_ok("README.md");
    fs::rename(parent.join("memoria"), parent.join("memoria.removing")).unwrap();
    fs::write(parent.join("memoria.install-txn.json"), format!("{{\"schema_version\":1,\"phase\":\"removing\",\"destination\":\"{}\",\"staging\":\"{}\",\"removing\":\"{}\",\"backup\":null}}", parent.join("memoria").display(), parent.join("memoria.staging").display(), parent.join("memoria.removing").display())).unwrap();
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "transaction artifacts are reserved, siblings unchanged"
    );
    assert_eq!(
        owned("custom/memoria.notes"),
        ("selected".to_string(), "README.md".to_string())
    );
    let (_, explain) = project.json(&["status", "--explain", "custom/memoria.removing/SKILL.md"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "excluded"
    );
    assert_eq!(
        project
            .json(&[
                "agent",
                "uninstall",
                "--target",
                "codex",
                "--path",
                parent.to_str().unwrap()
            ])
            .0,
        0
    );
    assert_eq!(project.json(&["check"]).0, 0);
    // The default bootstrap location follows the same exact identities.
    project.write(".agents/skills/memoria.rs", "not a package\n");
    assert_eq!(owned(".agents/skills/memoria.rs").0, "selected");
}

// MEM-026
#[test]
fn packets_above_the_decoded_budget_are_refused_and_smaller_ones_round_trip() {
    let project = Project::seed();
    project.baseline();
    let big = "A".repeat(9 * 1024 * 1024) + "\n";
    project.write("src/execution/big.txt", &big);
    project.commit_all("large baseline");
    let (packet, token) = {
        let output = project.run(&[
            "review",
            "src/execution/README.md",
            "--max-bytes",
            "33554432",
            "--format",
            "json",
        ]);
        assert_eq!(output.status.code(), Some(0));
        let path = project.packets.path().join("large-a.json");
        fs::write(&path, &output.stdout).unwrap();
        (
            path,
            get_str(&parse_json(&output.stdout), &["data", "token"]).to_string(),
        )
    };
    assert_eq!(
        project
            .ack(
                "src/execution/README.md",
                &packet,
                &token,
                "no-update",
                NOTE
            )
            .status
            .code(),
        Some(0)
    );
    // A 9 MiB A→B line: current + verified old + generated diff exceed the 32 MiB decoded cap.
    project.write("src/execution/big.txt", "B".repeat(9 * 1024 * 1024) + "\n");
    let before = project.state();
    let (code, refused) = project.json(&[
        "review",
        "src/execution/README.md",
        "--max-bytes",
        "33554432",
    ]);
    assert_eq!(
        code,
        1,
        "{}",
        memoria_infrastructure::json::to_compact(&refused)
            .chars()
            .take(400)
            .collect::<String>()
    );
    assert_eq!(diagnostic_codes(&refused), vec!["packet_too_large"]);
    assert!(matches!(get(&refused, &["data"]), Json::Object(map) if !map.contains_key("token")));
    assert_eq!(get_str(&refused, &["data", "kind"]), "packet_refused");
    assert!(get_u64(&refused, &["data", "size", "raw_input_bytes"]) > 9 * 1024 * 1024);
    assert_eq!(project.state(), before);
    // A change that fits the budget round-trips through the decoder.
    project.write("src/execution/big.txt", &big);
    project.append("src/execution/big.txt", "trailing change\n");
    let output = project.run(&[
        "review",
        "src/execution/README.md",
        "--max-bytes",
        "33554432",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let value = parse_json(&output.stdout);
    let Json::Array(diffs) = get(&value, &["data", "context", "diffs"]) else {
        panic!()
    };
    let entry = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/big.txt")
        .unwrap();
    assert_eq!(get_str(entry, &["status"]), "available");
    assert!(get_str(entry, &["text"]).contains("+trailing change"));
    let path = project.packets.path().join("large-b.json");
    fs::write(&path, &output.stdout).unwrap();
    let token = get_str(&value, &["data", "token"]).to_string();
    let output = project.ack("src/execution/README.md", &path, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}
