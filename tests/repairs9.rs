//! Regression tests for the round-9 triage findings (MEM-022 reopened,
//! MEM-041).

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::{self, Json, Limits};

// MEM-022 (reopened): init rejects an existing configuration whose meaning is invalid, before any write.
#[test]
fn init_rejects_invalid_existing_configuration_before_writing() {
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "parent glob",
            "version: 1\nignore: [\"../outside/**\"]\n".to_string(),
            "configuration_invalid",
        ),
        (
            "negation glob",
            "version: 1\ninclude: [\"!keep\"]\n".to_string(),
            "configuration_invalid",
        ),
        (
            "missing instruction",
            "version: 1\ndocumentation:\n  instruction_files: [missing.md]\n".to_string(),
            "instruction_file_missing",
        ),
        (
            "escaping instruction",
            "version: 1\ndocumentation:\n  instruction_files: [\"../outside.md\"]\n".to_string(),
            "instruction_file_invalid",
        ),
        (
            "symlinked instruction",
            "version: 1\ndocumentation:\n  instruction_files: [\"rules.md\"]\n".to_string(),
            "instruction_file_invalid",
        ),
    ];
    for (name, config, code) in cases {
        let project = Project::empty_repo();
        project.write("memoria.yml", &config);
        if name == "symlinked instruction" {
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("rules.md"), "# rules\n").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("rules.md"),
                project.root.join("rules.md"),
            )
            .unwrap();
        }
        project.write("source.txt", "original\n");
        project.commit_all("seed");
        let before = project.tree_snapshot();
        let (exit, init) = project.json(&["init"]);
        assert_eq!(exit, 1, "{name}: {init:?}");
        assert!(
            diagnostic_codes(&init).contains(&code.to_string()),
            "{name}: {:?}",
            diagnostic_codes(&init)
        );
        assert!(!project.exists("README.md"), "{name}: no README written");
        assert!(
            !project.exists(".memoria"),
            "{name}: no state directory, lock, or state"
        );
        assert_eq!(project.tree_snapshot(), before, "{name}: nothing written");
    }
    // A valid existing configuration still initializes the missing files, and unresolved imports are not init's concern.
    let project = Project::empty_repo();
    project.write("memoria.yml", "version: 1\nignore: [\"**/generated/**\"]\ndocumentation:\n  instruction_files: [\"rules.md\"]\n");
    project.write("rules.md", "# rules\n");
    project.write("README.md", "# Root\n\n<!-- memoria:import src=\"missing/README.md#summary\" -->\n<!-- /memoria:import -->\n");
    project.commit_all("seed");
    let (exit, init) = project.json(&["init"]);
    assert_eq!(exit, 0, "{init:?}");
    assert_eq!(
        strings(get(&init, &["data", "created"])),
        vec![".memoria/state.json"]
    );
    assert_eq!(
        project.json(&["lint"]).0,
        1,
        "the unresolved import is a lint failure, not an init failure"
    );
}

// MEM-041: the complete-record hard cap applies before any presentation or token.
#[test]
fn human_review_refuses_packets_above_the_record_cap_like_json() {
    let make = |instructions: usize| -> Project {
        let project = Project::empty_repo();
        let mut config = String::from("version: 1\ndocumentation:\n  instructions:\n");
        for _ in 0..instructions {
            config.push_str("    - x\n");
        }
        project.write("memoria.yml", &config);
        project.write("README.md", "# Root\n");
        project.write("child/README.md", "# Child\n");
        project.commit_all("seed");
        assert_eq!(project.run(&["init"]).status.code(), Some(0));
        project
    };
    // Calibrate: every instruction is one record; the navigation diagnostic adds one.
    let probe = make(10);
    let output = probe.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let envelope = json::parse(&output.stdout, Limits::PACKET).unwrap();
    let base = get_u64(&envelope, &["data", "size", "record_count"]) - 10;
    assert!(
        base >= 1,
        "the disconnected child produces a diagnostic element"
    );
    let exact = 100_000 - base as usize;
    // Exactly 100,000 records: both formats succeed with the same count.
    let project = make(exact);
    let output = project.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let envelope = json::parse(&output.stdout, Limits::PACKET).unwrap();
    assert_eq!(
        get_u64(&envelope, &["data", "size", "record_count"]),
        100_000
    );
    let human = project.run(&["review", "README.md"]);
    assert_eq!(human.status.code(), Some(0));
    assert!(
        stdout(&human).contains("in 100000 record(s)"),
        "{}",
        stdout(&human)
    );
    assert!(stdout(&human).contains("Token           mrv1."));
    // 100,001 records: both formats refuse without a token.
    let project = make(exact + 1);
    let (code, refused) = project.json(&["review", "README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["packet_too_large"]);
    assert!(matches!(
        get(&refused, &["data"]),
        Json::Null | Json::Object(_)
    ));
    let human = project.run(&["review", "README.md"]);
    assert_eq!(human.status.code(), Some(1), "{}", stdout(&human));
    assert!(
        !stdout(&human).contains("mrv1."),
        "no token in human output: {}",
        stdout(&human)
    );
    assert!(stderr(&human).contains("packet_too_large"));
    // The project is untouched by either refusal.
    assert!(!project.exists(".memoria/state.json.tmp"));
    assert_eq!(
        project.json(&["check"]).0,
        1,
        "review still pending, nothing acknowledged"
    );
}
