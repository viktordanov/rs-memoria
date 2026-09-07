//! Regression tests for the round-7 triage findings (MEM-038, MEM-039, MEM-040).

mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use memoria_infrastructure::json::{self, Json, Limits};

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

// MEM-038
#[test]
fn substituted_installed_files_never_hang_or_get_removed() {
    for (target, custom) in [("codex", false), ("claude", true)] {
        let project = Project::seed();
        project.baseline();
        let mut install = vec!["agent", "install", "--target", target];
        let parent = if custom {
            project.root.join("custom-skills")
        } else {
            project.root.join(format!(
                ".{}/skills",
                if target == "codex" {
                    "agents"
                } else {
                    "claude"
                }
            ))
        };
        let parent_str = parent.to_str().unwrap().to_string();
        if custom {
            install.extend(["--path", &parent_str]);
        }
        assert_eq!(project.json(&install).0, 0);
        let skill = parent.join("memoria/SKILL.md");
        let original = fs::read(&skill).unwrap();
        // FIFO in place of the installed skill: every command terminates; mutation is refused.
        fs::remove_file(&skill).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(&skill)
                .status()
                .unwrap()
                .success()
        );
        let output = run_with_deadline(&project, &["status", "--format", "json"], 30);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let mut uninstall = vec![
            "agent",
            "uninstall",
            "--target",
            target,
            "--dry-run",
            "--format",
            "json",
        ];
        if custom {
            uninstall.extend(["--path", &parent_str]);
        }
        let output = run_with_deadline(&project, &uninstall, 30);
        assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec!["skill_conflict"]
        );
        let mut real = uninstall.clone();
        real.retain(|a| *a != "--dry-run");
        let output = run_with_deadline(&project, &real, 30);
        assert_eq!(output.status.code(), Some(3));
        assert!(
            fs::symlink_metadata(&skill)
                .map(|m| !m.file_type().is_file())
                .unwrap_or(false),
            "the FIFO is preserved"
        );
        assert_eq!(
            project.json(&["check"]).0,
            0,
            "guidance discovery still excludes the package"
        );
        fs::remove_file(&skill).unwrap();
        // A symlink to identical external bytes is a local change: preserved, conflict, target intact.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), &original).unwrap();
        std::os::unix::fs::symlink(outside.path().join("SKILL.md"), &skill).unwrap();
        let output = run_with_deadline(&project, &real, 30);
        assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
        assert!(
            fs::symlink_metadata(&skill)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink is preserved"
        );
        assert_eq!(fs::read(outside.path().join("SKILL.md")).unwrap(), original);
        let mut reinstall = install.clone();
        reinstall.extend(["--format", "json"]);
        let output = run_with_deadline(&project, &reinstall, 30);
        assert_eq!(output.status.code(), Some(3));
        // Restoring the regular file makes uninstall succeed again.
        fs::remove_file(&skill).unwrap();
        fs::write(&skill, &original).unwrap();
        let output = run_with_deadline(&project, &real, 30);
        assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    }
}

// MEM-039
#[test]
fn instruction_only_sidecars_are_context_not_policy() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    project.write(
        "src/execution/README.memoria.toml",
        "[documentation]\ninstructions = [\n    \"Use clear short sentences.\",\n]\n",
    );
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "adding an instruction-only sidecar changes no fingerprint"
    );
    project.write("src/execution/README.memoria.toml", "[documentation]\ninstructions = [\n    \"Use even shorter sentences.\",\n]\ninstruction_files = []\n");
    assert_eq!(project.json(&["check"]).0, 0);
    // Packets still inherit the local instruction.
    project.append("src/execution/runner.rs", "// edit\n");
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(packet).unwrap());
    let Json::Array(instructions) = get(&value, &["data", "context", "instructions"]) else {
        panic!()
    };
    assert!(
        instructions
            .iter()
            .any(|i| get_str(i, &["text"]) == "Use even shorter sentences."
                && get_str(i, &["source"]) == "src/execution/README.memoria.toml")
    );
    project.ack_ok("src/execution/README.md");
    fs::remove_file(project.root.join("src/execution/README.memoria.toml")).unwrap();
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "removing it changes nothing either"
    );
    // A real local rule is policy, as before.
    project.write(
        "src/execution/README.memoria.toml",
        "ignore = [\n    \"*.tmp\",\n]\n\n[documentation]\ninstructions = [\n    \"Keep it short.\",\n]\n",
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    assert_eq!(
        project.cause_codes("src/corpus/README.md"),
        Vec::<String>::new()
    );
    assert_ne!(
        project.state(),
        before,
        "the earlier acknowledgement was recorded"
    );
    // The fixture's existing rule-bearing sidecar keeps its policy meaning (baseline already covers it).
    project.write(
        "src/retrieval/README.memoria.toml",
        "include = [\n    \"fixtures/**\",\n    \"nothing/**\",\n]\n",
    );
    assert_eq!(
        project.cause_codes("src/retrieval/README.md"),
        vec!["input_changed"]
    );
}

/// Independent recursive count of every array element in a JSON value.
fn count_elements(value: &Json) -> u64 {
    match value {
        Json::Array(items) => items.len() as u64 + items.iter().map(count_elements).sum::<u64>(),
        Json::Object(map) => map.values().map(count_elements).sum(),
        _ => 0,
    }
}

// MEM-040
#[test]
fn packet_record_counts_match_the_complete_envelope() {
    // Minimal packet in a fresh repository.
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n");
    project.commit_all("seed");
    assert_eq!(project.run(&["init"]).status.code(), Some(0));
    let output = project.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let envelope = json::parse(&output.stdout, Limits::PACKET).unwrap();
    assert_eq!(
        get_u64(&envelope, &["data", "size", "record_count"]),
        count_elements(&envelope)
    );
    // A packet with previous review context, diffs, instructions, and warning/hint diagnostics.
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    project.append("src/execution/runner.rs", "// edit\n");
    let output = project.run(&["review", "src/execution/README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let envelope = json::parse(&output.stdout, Limits::PACKET).unwrap();
    assert!(!matches!(
        get(&envelope, &["data", "context", "previous_review"]),
        Json::Null
    ));
    assert!(matches!(get(&envelope, &["diagnostics"]), Json::Array(d) if !d.is_empty()));
    let counted = count_elements(&envelope);
    assert_eq!(
        get_u64(&envelope, &["data", "size", "record_count"]),
        counted
    );
    assert!(counted > 5);
    // The plan reports the manifest's own element count.
    let plan = project.plan();
    let Json::Array(tasks) = get(&plan, &["data", "tasks"]) else {
        panic!()
    };
    assert_eq!(
        get_u64(&tasks[0], &["record_count"]),
        1,
        "one owned file, no imports"
    );
    // A tampered count with a recomputed digest is rejected without mutation.
    let packet = project.packets.path().join("count.json");
    fs::write(&packet, &output.stdout).unwrap();
    let token = get_str(&envelope, &["data", "token"]).to_string();
    let mut tampered = envelope.clone();
    set_path(
        &mut tampered,
        &["data", "size", "record_count"],
        Json::Number(counted + 1),
    );
    if let Json::Object(map) = &mut tampered
        && let Some(Json::Object(data)) = map.get_mut("data")
    {
        data.remove("packet_digest");
    }
    let digest = memoria_infrastructure::packet::packet_digest(
        &memoria_infrastructure::Xxh3Hasher,
        &tampered,
    );
    set_path(
        &mut tampered,
        &["data", "packet_digest"],
        Json::String(digest),
    );
    let bad = project.packets.path().join("count-bad.json");
    fs::write(&bad, json::to_pretty(&tampered)).unwrap();
    let before = project.state();
    let output = project.run(&[
        "ack",
        "src/execution/README.md",
        "--packet",
        bad.to_str().unwrap(),
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
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_schema_invalid"]
    );
    assert_eq!(project.state(), before);
    // The untampered packet acknowledges.
    let output = project.ack(
        "src/execution/README.md",
        &packet,
        &token,
        "no-update",
        NOTE,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}
