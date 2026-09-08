//! Explicit, reversible project-level `Stop` hooks and the bounded runner.
//!
//! Installation is a local change. The client still reviews and activates the
//! hook, and the runner never continues an agent turn.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use memoria_infrastructure::json::Json;

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agent-hooks")
            .join(name),
    )
    .unwrap()
}

/// Client probing is real, so tests that need a supported client provide a
/// stub `codex`/`claude` on PATH.
fn stub_clients(project: &Project) -> std::path::PathBuf {
    let bin = project.home.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, version) in [("codex", "codex-cli 0.153.0"), ("claude", "2.1.259")] {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\necho '{version}'\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn run_with_clients(project: &Project, args: &[&str]) -> std::process::Output {
    let bin = stub_clients(project);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    project
        .command(&project.root, args)
        .env("PATH", path)
        .output()
        .unwrap()
}

/// Run a command, retrying briefly while the executable is still busy.
///
/// These tests copy the executable and run the copy. Another test thread can
/// hold a write descriptor for that new file across its own fork, and the
/// kernel then refuses the exec with `ETXTBSY`. That is a property of the
/// test harness, not of Memoria.
fn output_when_ready(command: &mut std::process::Command) -> std::process::Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match command.output() {
            Ok(output) => return output,
            Err(err) if err.raw_os_error() == Some(26) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => panic!("cannot run the executable: {err}"),
        }
    }
}

/// Run a copy of the executable that lives at another path.
fn json_with_clients_at(project: &Project, executable: &str, args: &[&str]) -> (i32, Json) {
    let bin = stub_clients(project);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = std::process::Command::new(executable);
    command
        .current_dir(&project.root)
        .args(args)
        .args(["--format", "json"])
        .env("PATH", path)
        .env("HOME", project.home.path())
        .env("XDG_CONFIG_HOME", project.home.path().join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("CLAUDE_CONFIG_DIR");
    let output = output_when_ready(&mut command);
    (output.status.code().unwrap(), parse_json(&output.stdout))
}

fn json_with_clients(project: &Project, args: &[&str]) -> (i32, Json) {
    let mut full: Vec<&str> = args.to_vec();
    full.push("--format");
    full.push("json");
    let output = run_with_clients(project, &full);
    (output.status.code().unwrap(), parse_json(&output.stdout))
}

#[test]
fn hook_install_is_explicit_idempotent_and_reversible() {
    for (target, configuration, record) in [
        ("codex", ".codex/hooks.json", ".codex/memoria-hook.json"),
        (
            "claude",
            ".claude/settings.local.json",
            ".claude/memoria-hook.json",
        ),
    ] {
        let project = Project::seed();
        project.baseline();
        // A dry run changes nothing.
        let before = project.tree_snapshot();
        let (code, plan) = json_with_clients(
            &project,
            &["agent", "hook", "install", "--target", target, "--dry-run"],
        );
        assert_eq!(code, 0, "{target}: {plan:?}");
        assert_eq!(get_str(&plan, &["data", "plan", "state"]), "absent");
        assert_eq!(
            project.tree_snapshot(),
            before,
            "{target}: dry run wrote nothing"
        );

        let (code, installed) =
            json_with_clients(&project, &["agent", "hook", "install", "--target", target]);
        assert_eq!(code, 0, "{target}: {installed:?}");
        assert!(get_bool(&installed, &["data", "applied"]));
        assert_eq!(
            get_str(&installed, &["data", "plan", "activation"]),
            "requires-client-review",
            "{target}: the client still approves the hook"
        );
        assert!(project.exists(configuration), "{target}");
        assert!(project.exists(record), "{target}");

        // The owned group has one command handler, no matcher, timeout 5.
        let value = parse_json(&project.read(configuration));
        let Json::Array(groups) = get(&value, &["hooks", "Stop"]) else {
            panic!("{target}: no Stop array")
        };
        assert_eq!(groups.len(), 1, "{target}");
        let Json::Array(handlers) = get(&groups[0], &["hooks"]) else {
            panic!()
        };
        assert_eq!(handlers.len(), 1, "{target}");
        assert_eq!(get_str(&handlers[0], &["type"]), "command");
        assert_eq!(get_u64(&handlers[0], &["timeout"]), 5);
        let command = get_str(&handlers[0], &["command"]);
        assert!(command.contains("agent hook run"), "{target}: {command}");
        assert!(command.contains("--protocol 1"), "{target}");
        assert!(command.ends_with("|| printf '{}\\n'"), "{target}");
        let Json::Object(group) = &groups[0] else {
            panic!()
        };
        assert_eq!(
            group.keys().cloned().collect::<Vec<_>>(),
            vec!["hooks".to_string()],
            "{target}: the group has no matcher"
        );

        // Installing again is a no-op.
        let after = project.tree_snapshot();
        let (code, again) =
            json_with_clients(&project, &["agent", "hook", "install", "--target", target]);
        assert_eq!(code, 0, "{target}: {again:?}");
        assert!(get_bool(&again, &["data", "plan", "no_change"]), "{target}");
        assert_eq!(project.tree_snapshot(), after, "{target}");

        // Status reports the installed state without writing.
        let (code, status) =
            json_with_clients(&project, &["agent", "hook", "status", "--target", target]);
        assert_eq!(code, 0, "{target}: {status:?}");
        assert_eq!(get_str(&status, &["data", "plan", "state"]), "installed");
        assert_eq!(project.tree_snapshot(), after, "{target}");

        // Uninstall removes the owned group and the record.
        let (code, removed) = json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", target],
        );
        assert_eq!(code, 0, "{target}: {removed:?}");
        assert!(!project.exists(record), "{target}: the record is gone");
        let (code, twice) = json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", target],
        );
        assert_eq!(code, 0, "{target}: {twice:?}");
        assert!(get_bool(&twice, &["data", "plan", "no_change"]), "{target}");
    }
}

#[test]
fn hook_uninstall_preserves_later_user_edits() {
    let project = Project::seed();
    project.baseline();
    project.write(
        ".claude/settings.local.json",
        fixture("shared-settings.json"),
    );
    let original = project.read_string(".claude/settings.local.json");
    let (code, _) = json_with_clients(
        &project,
        &["agent", "hook", "install", "--target", "claude"],
    );
    assert_eq!(code, 0);
    // A user adds an unrelated key afterwards, without reformatting the
    // exact numbers the file already holds.
    let text = project.read_string(".claude/settings.local.json");
    let edited = text.replacen('{', "{\n  \"addedLater\": \"keep\",", 1);
    project.write(".claude/settings.local.json", &edited);
    let (code, _) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "claude"],
    );
    assert_eq!(code, 0);
    // The user's own Stop group, other events, and later keys all survive.
    // The file holds numbers wider than 64 bits, so it is checked as text.
    let after = project.read_string(".claude/settings.local.json");
    assert!(after.contains("echo user stop"), "{after}");
    assert!(after.contains("\"PreToolUse\""), "{after}");
    assert!(after.contains("\"addedLater\""), "{after}");
    assert!(after.contains("Bash(ls:*)"), "{after}");
    assert!(
        !after.contains("agent hook run"),
        "the owned entry is gone: {after}"
    );
    // Exact numbers survive the parse and serialize round trip.
    assert!(
        after.contains("123456789012345678901234567890"),
        "exact numbers are preserved: {after}"
    );
    assert!(after.contains("-2.5e-8"), "{after}");
    assert!(original.contains("123456789012345678901234567890"));
}

#[test]
fn hook_ownership_never_adopts_user_entries() {
    let project = Project::seed();
    project.baseline();
    // A user entry whose command looks like Memoria's, with no record.
    project.write(
        ".codex/hooks.json",
        "{\n  \"hooks\": {\n    \"Stop\": [\n      { \"hooks\": [ { \"type\": \"command\", \"command\": \"memoria agent hook run --target codex\" } ] }\n    ]\n  }\n}\n",
    );
    let before = project.read_string(".codex/hooks.json");
    let (code, refused) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["hook_unmanaged"]);
    assert_eq!(project.read_string(".codex/hooks.json"), before);
    assert!(!project.exists(".codex/memoria-hook.json"));

    // A modified owned entry is a conflict, and nothing is changed.
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]).0,
        0
    );
    let mut value = parse_json(&project.read(".codex/hooks.json"));
    set_path(
        &mut value,
        &["hooks", "Stop"],
        Json::Array(vec![Json::Object(
            [(
                "hooks".to_string(),
                Json::Array(vec![Json::Object(
                    [
                        ("type".to_string(), Json::String("command".into())),
                        ("command".to_string(), Json::String("edited".into())),
                    ]
                    .into_iter()
                    .collect(),
                )]),
            )]
            .into_iter()
            .collect(),
        )]),
    );
    project.write(
        ".codex/hooks.json",
        memoria_infrastructure::json::to_pretty(&value),
    );
    let edited = project.read_string(".codex/hooks.json");
    let (code, conflict) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 3, "{conflict:?}");
    assert_eq!(diagnostic_codes(&conflict), vec!["hook_conflict"]);
    assert_eq!(project.read_string(".codex/hooks.json"), edited);

    // A corrupt ownership record is a conflict too.
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]).0,
        0
    );
    project.write(".codex/memoria-hook.json", "{\"version\": 1}");
    let (code, corrupt) =
        json_with_clients(&project, &["agent", "hook", "status", "--target", "codex"]);
    assert_eq!(code, 3, "{corrupt:?}");
    assert_eq!(diagnostic_codes(&corrupt), vec!["hook_conflict"]);
}

#[test]
fn codex_hook_representation_follows_existing_configuration() {
    // An existing inline hook table selects the TOML representation.
    let project = Project::seed();
    project.baseline();
    project.write(".codex/config.toml", fixture("inline-config.toml"));
    let (code, installed) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{installed:?}");
    assert!(
        get_str(&installed, &["data", "plan", "configuration"]).ends_with(".codex/config.toml"),
        "{installed:?}"
    );
    let text = project.read_string(".codex/config.toml");
    assert!(text.contains("model = \"gpt-5\""), "unrelated values kept");
    assert!(text.contains("# kept exactly"), "comments kept");
    assert!(text.contains("echo user inline stop"), "user group kept");
    assert!(text.contains("agent hook run"), "the owned group was added");
    assert!(
        !project.exists(".codex/hooks.json"),
        "no second representation"
    );
    // Removal restores the user's own inline configuration.
    let (code, _) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 0);
    let text = project.read_string(".codex/config.toml");
    assert!(text.contains("echo user inline stop"));
    assert!(!text.contains("agent hook run"));
    assert!(text.contains("# kept exactly"));

    // Both representations populated is an ambiguity, refused without writes.
    let project = Project::seed();
    project.baseline();
    project.write(".codex/config.toml", fixture("inline-config.toml"));
    project.write(
        ".codex/hooks.json",
        "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo json\"}]}]}}",
    );
    let before = project.tree_snapshot();
    let (code, ambiguous) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{ambiguous:?}");
    assert_eq!(
        diagnostic_codes(&ambiguous),
        vec!["hook_configuration_ambiguous"]
    );
    assert_eq!(project.tree_snapshot(), before);

    // An empty inline table permits the JSON representation.
    let project = Project::seed();
    project.baseline();
    project.write(".codex/config.toml", "model = \"gpt-5\"\n\n[hooks]\n");
    let (code, installed) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{installed:?}");
    assert!(project.exists(".codex/hooks.json"));
    assert_eq!(
        project.read_string(".codex/config.toml"),
        "model = \"gpt-5\"\n\n[hooks]\n",
        "the TOML file is untouched"
    );
}

#[test]
fn hook_configuration_rejects_unsafe_or_oversized_inputs() {
    let project = Project::seed();
    project.baseline();
    // Duplicate keys.
    project.write(".codex/hooks.json", "{\"hooks\":{},\"hooks\":{}}");
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["hook_configuration_invalid"]);
    // Corrupt syntax.
    project.write(".codex/hooks.json", "{ not json");
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(diagnostic_codes(&value), vec!["hook_configuration_invalid"]);
    // Oversized configuration.
    project.write(
        ".codex/hooks.json",
        format!("{{\"pad\":\"{}\"}}", "x".repeat(2 * 1024 * 1024)),
    );
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(
        diagnostic_codes(&value),
        vec!["hook_configuration_too_large"]
    );
    // A symlinked configuration is refused, not followed.
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("hooks.json"), "{}").unwrap();
    fs::remove_file(project.root.join(".codex/hooks.json")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("hooks.json"),
        project.root.join(".codex/hooks.json"),
    )
    .unwrap();
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 3, "{value:?}");
    assert_eq!(
        fs::read_to_string(outside.path().join("hooks.json")).unwrap(),
        "{}"
    );
}

#[test]
fn hook_client_support_is_probed_before_any_write() {
    let project = Project::seed();
    project.baseline();
    let before = project.tree_snapshot();
    // A PATH that offers Git but no supported agent client.
    let bin = project.home.path().join("git-only");
    fs::create_dir_all(&bin).unwrap();
    let git = which("git");
    std::os::unix::fs::symlink(&git, bin.join("git")).unwrap();
    let only_git = bin.display().to_string();
    // Install refuses before writing.
    let output = project
        .command(
            &project.root,
            &[
                "agent", "hook", "install", "--target", "codex", "--format", "json",
            ],
        )
        .env("PATH", &only_git)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["hook_client_unsupported"]
    );
    assert_eq!(project.tree_snapshot(), before);
    // Status and uninstall stay available without a client.
    let output = project
        .command(
            &project.root,
            &[
                "agent", "hook", "status", "--target", "codex", "--format", "json",
            ],
        )
        .env("PATH", &only_git)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
}

/// The absolute path of a program on the current PATH.
fn which(program: &str) -> std::path::PathBuf {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {program}"))
        .output()
        .unwrap();
    std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

#[test]
fn stop_runner_reports_counts_without_continuing_the_agent() {
    let project = Project::seed();
    project.baseline();
    let event = |cwd: &str, recursive: bool| {
        fixture("codex-stop.json")
            .replace("REPLACED_BY_TEST", cwd)
            .replace(
                "\"stop_hook_active\": false",
                &format!("\"stop_hook_active\": {recursive}"),
            )
    };
    let run = |body: String| {
        project.run_stdin(
            &[
                "agent",
                "hook",
                "run",
                "--target",
                "codex",
                "--protocol",
                "1",
                "--configuration-root",
                project.root.to_str().unwrap(),
            ],
            body.as_bytes(),
        )
    };
    // A current project says nothing.
    let output = run(event(project.root.to_str().unwrap(), false));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
    // Pending work produces one bounded advisory message.
    project.append("src/execution/runner.rs", "// pending\n");
    let output = run(event(project.root.to_str().unwrap(), false));
    assert_eq!(output.status.code(), Some(0));
    let value = parse_json(stdout(&output).trim().as_bytes());
    let message = get_str(&value, &["systemMessage"]).to_string();
    assert!(message.contains("memoria review"), "{message}");
    assert!(message.len() <= 512, "{message}");
    let Json::Object(fields) = &value else {
        panic!()
    };
    assert_eq!(
        fields.keys().cloned().collect::<Vec<_>>(),
        vec!["systemMessage".to_string()],
        "the runner never emits a decision, a prompt, or a tool request"
    );
    // Recursion, another event, and an unrelated project stay silent.
    for body in [
        event(project.root.to_str().unwrap(), true),
        event(project.root.to_str().unwrap(), false).replace("\"Stop\"", "\"PreToolUse\""),
        event("/nonexistent", false),
    ] {
        let output = run(body);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(stdout(&output).trim(), "{}");
    }
    // An event from a different registered project stays silent.
    let other = Project::seed();
    other.baseline();
    let output = run(event(other.root.to_str().unwrap(), false));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
}

#[test]
fn stop_runner_handles_malformed_input_and_unsupported_protocols() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let run = |args: &[&str], body: &[u8]| project.run_stdin(args, body);
    let base: Vec<&str> = vec![
        "agent",
        "hook",
        "run",
        "--target",
        "codex",
        "--protocol",
        "1",
        "--configuration-root",
    ];
    let root = project.root.to_str().unwrap();
    for body in [
        &b""[..],
        b"not json",
        b"[]",
        b"{}",
        b"{\"hook_event_name\": 5}",
        &vec![b'x'; 70 * 1024],
    ] {
        let mut args = base.clone();
        args.push(root);
        let output = run(&args, body);
        assert_eq!(output.status.code(), Some(0), "body {:?}", body.len());
        assert_eq!(stdout(&output).trim(), "{}");
    }
    // An unsupported protocol is silent, never a usage exit.
    let body = fixture("codex-stop.json").replace("REPLACED_BY_TEST", root);
    let output = run(
        &[
            "agent",
            "hook",
            "run",
            "--target",
            "codex",
            "--protocol",
            "2",
            "--configuration-root",
            root,
        ],
        body.as_bytes(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
    // A project without configuration is silent.
    let bare = Project::empty_repo();
    bare.write("README.md", "# Root\n");
    bare.commit_all("seed");
    let body = fixture("codex-stop.json").replace("REPLACED_BY_TEST", bare.root.to_str().unwrap());
    let output = run(
        &[
            "agent",
            "hook",
            "run",
            "--target",
            "codex",
            "--protocol",
            "1",
            "--configuration-root",
            bare.root.to_str().unwrap(),
        ],
        body.as_bytes(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
}

#[test]
fn stop_runner_reports_failed_inspection_without_claiming_clean() {
    let project = Project::seed();
    project.baseline();
    // Corrupt state: the runner names the diagnostic, never silence.
    project.write("memoria.lock", b"broken");
    let body =
        fixture("codex-stop.json").replace("REPLACED_BY_TEST", project.root.to_str().unwrap());
    let output = project.run_stdin(
        &[
            "agent",
            "hook",
            "run",
            "--target",
            "codex",
            "--protocol",
            "1",
            "--configuration-root",
            project.root.to_str().unwrap(),
        ],
        body.as_bytes(),
    );
    assert_eq!(output.status.code(), Some(0));
    let value = parse_json(stdout(&output).trim().as_bytes());
    let message = get_str(&value, &["systemMessage"]).to_string();
    assert!(message.contains("state_corrupt"), "{message}");
    assert!(message.contains("memoria status"), "{message}");
}

#[test]
fn installed_integrations_do_not_change_freshness() {
    let project = Project::seed();
    project.baseline();
    let state = project.state();
    let manifests = project.inspect_state();
    // Installing a hook and a skill changes no manifest and no freshness.
    assert_eq!(
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]).0,
        0
    );
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "installation is not a review"
    );
    assert_eq!(project.state(), state);
    assert_eq!(project.inspect_state(), manifests);
    // The reserved paths are labeled as agent context, not product inputs.
    for path in [".codex/hooks.json", ".codex/memoria-hook.json"] {
        let (code, explain) = project.json(&["status", "--explain", path]);
        assert_eq!(code, 0, "{path}: {explain:?}");
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "excluded",
            "{path}"
        );
        assert!(
            get_str(&explain, &["data", "explanation", "reason"]).contains("agent-hook"),
            "{path}: {}",
            get_str(&explain, &["data", "explanation", "reason"])
        );
    }
    // An unrelated file in the same directory stays an ordinary input.
    project.write(".codex/notes.md", "Project notes.\n");
    let (_, explain) = project.json(&["status", "--explain", ".codex/notes.md"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected"
    );
    // Removing both integrations is equally neutral.
    assert_eq!(
        json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", "codex"]
        )
        .0,
        0
    );
    assert_eq!(
        project.json(&["agent", "uninstall", "--target", "codex"]).0,
        0
    );
    fs::remove_file(project.root.join(".codex/notes.md")).unwrap();
    assert_eq!(project.json(&["check"]).0, 0);
    assert_eq!(project.state(), state);
}

#[test]
fn hook_launcher_quotes_paths_and_absorbs_usage_exit() {
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]).0,
        0
    );
    let value = parse_json(&project.read(".codex/hooks.json"));
    let Json::Array(groups) = get(&value, &["hooks", "Stop"]) else {
        panic!()
    };
    let Json::Array(handlers) = get(&groups[0], &["hooks"]) else {
        panic!()
    };
    let command = get_str(&handlers[0], &["command"]).to_string();

    // The wrapper is a single POSIX command line. Running it directly must
    // produce exactly one JSON object and exit 0.
    let body =
        fixture("codex-stop.json").replace("REPLACED_BY_TEST", project.root.to_str().unwrap());
    let run_wrapper = |command: &str| -> (i32, String) {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&project.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", project.home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        let payload = body.clone().into_bytes();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&payload);
        });
        let output = child.wait_with_output().unwrap();
        writer.join().unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )
    };
    let (code, out) = run_wrapper(&command);
    assert_eq!(code, 0, "the wrapper exits 0: {out}");
    assert_eq!(out, "{}", "a current project produces one empty object");

    // A moved or missing executable still produces one safe native result.
    let missing = command.replacen(
        &format!("'{}'", project.root.join("../nowhere/memoria").display()),
        "'/nonexistent/memoria'",
        1,
    );
    let missing = if missing == command {
        // Replace the first quoted absolute path with a missing one.
        let start = command.find('\'').unwrap();
        let end = command[start + 1..].find('\'').unwrap() + start + 1;
        format!(
            "{}'/nonexistent/memoria'{}",
            &command[..start],
            &command[end + 1..]
        )
    } else {
        missing
    };
    let (code, out) = run_wrapper(&missing);
    assert_eq!(code, 0, "a missing executable still exits 0: {out}");
    assert_eq!(out, "{}", "a missing executable emits an empty object");

    // An older CLI that returns a usage exit must not become a decision.
    let stub = project.home.path().join("old-memoria");
    std::fs::write(
        &stub,
        "#!/bin/sh\necho 'error: unknown argument' >&2\nexit 2\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let start = command.find('\'').unwrap();
    let end = command[start + 1..].find('\'').unwrap() + start + 1;
    let older = format!(
        "{}'{}'{}",
        &command[..start],
        stub.display(),
        &command[end + 1..]
    );
    let (code, out) = run_wrapper(&older);
    assert_eq!(code, 0, "a usage exit is absorbed: {out}");
    assert_eq!(out, "{}", "a usage exit emits an empty object");
}

#[test]
fn hook_paths_with_metacharacters_survive_quoting() {
    // A worktree whose name holds a space, a quote, and shell metacharacters.
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("odd 'name' $(x);dir");
    std::fs::create_dir_all(&root).unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(root.join("README.md"), "# Odd\n\nAuthored root.\n").unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    // The shared isolated seed: no host global or system Git configuration
    // reaches this fixture, so commit signing on the developer's machine
    // cannot make the acceptance test command fail.
    seed_repository(&root, home.path());

    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("codex"), "#!/bin/sh\necho 'codex-cli 0.153.0'\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |args: &[&str]| {
        std::process::Command::new(memoria_bin())
            .current_dir(&root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home.path())
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    assert_eq!(run(&["init", "--apply"]).status.code(), Some(0));
    let installed = run(&[
        "agent", "hook", "install", "--target", "codex", "--format", "json",
    ]);
    assert_eq!(
        installed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&installed.stdout)
    );
    let value = parse_json(&std::fs::read(root.join(".codex/hooks.json")).unwrap());
    let Json::Array(groups) = get(&value, &["hooks", "Stop"]) else {
        panic!()
    };
    let Json::Array(handlers) = get(&groups[0], &["hooks"]) else {
        panic!()
    };
    let command = get_str(&handlers[0], &["command"]).to_string();
    assert!(
        command.contains("odd "),
        "the odd path is present: {command}"
    );
    // The quoted form survives a shell round trip without executing anything.
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "{}",
        "the wrapper produced one empty object"
    );
}

#[test]
fn hook_install_requires_semantically_valid_configuration() {
    // Parsing the TOML file is not the precondition; the configuration must
    // also mean something valid. Nothing is written when it does not.
    for (label, config, code) in [
        (
            "invalid ignore pattern",
            "version = 2\nignore = [\"../outside\"]\n",
            "configuration_invalid",
        ),
        (
            "missing guidance file",
            "version = 2\n[documentation]\nguidance_files = [\"absent.md\"]\n",
            "guidance_file_missing",
        ),
    ] {
        let project = Project::seed();
        project.baseline();
        project.write("memoria.toml", config);
        // `status` already rejects it.
        assert_eq!(project.json(&["status"]).0, 1, "{label}");
        let before = project.tree_snapshot();
        let (exit, value) =
            json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
        assert_eq!(exit, 1, "{label}: {value:?}");
        assert!(
            diagnostic_codes(&value).contains(&code.to_string()),
            "{label}: {:?}",
            diagnostic_codes(&value)
        );
        assert_eq!(
            project.tree_snapshot(),
            before,
            "{label}: nothing was written"
        );
        assert!(!project.exists(".codex/hooks.json"), "{label}");
        assert!(!project.exists(".codex/memoria-hook.json"), "{label}");
        // Status and uninstall stay available without valid configuration.
        assert_eq!(
            json_with_clients(&project, &["agent", "hook", "status", "--target", "codex"]).0,
            0,
            "{label}: status stays available"
        );
        assert_eq!(
            json_with_clients(
                &project,
                &["agent", "hook", "uninstall", "--target", "codex"]
            )
            .0,
            0,
            "{label}: uninstall stays available"
        );
        assert_eq!(project.tree_snapshot(), before, "{label}");
    }
}

/// A directory holding stub clients, and a PATH that finds only those.
fn stub_client_path(project: &Project, stubs: &[(&str, &str)]) -> String {
    let bin = project.home.path().join("stub-bin");
    std::fs::create_dir_all(&bin).unwrap();
    // A real `git` must stay reachable; nothing else from the host does.
    let git = which("git");
    let _ = std::fs::remove_file(bin.join("git"));
    std::os::unix::fs::symlink(&git, bin.join("git")).unwrap();
    for (name, script) in stubs {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin.display().to_string()
}

#[test]
fn hook_install_enforces_the_frozen_client_version_floors() {
    for (target, program, too_old, exact, newer) in [
        (
            "codex",
            "codex",
            "codex-cli 0.152.999",
            "codex-cli 0.153.0",
            "codex-cli 0.154.1",
        ),
        (
            "claude",
            "claude",
            "2.1.258 (Claude Code)",
            "2.1.259 (Claude Code)",
            "3.0.0 (Claude Code)",
        ),
    ] {
        // Below the floor: refused before any write.
        let project = Project::seed();
        project.baseline();
        let path = stub_client_path(
            &project,
            &[(program, &format!("#!/bin/sh\necho '{too_old}'\n"))],
        );
        let before = project.tree_snapshot();
        let output = project
            .command(
                &project.root,
                &[
                    "agent", "hook", "install", "--target", target, "--format", "json",
                ],
            )
            .env("PATH", &path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{target}: {}",
            stdout(&output)
        );
        let value = parse_json(&output.stdout);
        assert_eq!(diagnostic_codes(&value), vec!["hook_client_unsupported"]);
        let message = memoria_infrastructure::json::to_compact(&value);
        assert!(message.contains("below the supported floor"), "{message}");
        assert_eq!(project.tree_snapshot(), before, "{target}: nothing written");

        // A client that prints no version is unsupported too.
        let path = stub_client_path(&project, &[(program, "#!/bin/sh\necho 'unknown build'\n")]);
        let output = project
            .command(
                &project.root,
                &[
                    "agent", "hook", "install", "--target", target, "--format", "json",
                ],
            )
            .env("PATH", &path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{target}");
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec!["hook_client_unsupported"]
        );
        assert_eq!(project.tree_snapshot(), before, "{target}");

        // Exactly the floor, and a newer version, both install.
        for version in [exact, newer] {
            let project = Project::seed();
            project.baseline();
            let path = stub_client_path(
                &project,
                &[(program, &format!("#!/bin/sh\necho '{version}'\n"))],
            );
            let output = project
                .command(
                    &project.root,
                    &[
                        "agent", "hook", "install", "--target", target, "--format", "json",
                    ],
                )
                .env("PATH", &path)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{target} {version}: {}",
                stdout(&output)
            );
        }
    }
}

#[test]
fn ordinary_commands_never_execute_an_optional_client() {
    let project = Project::seed();
    project.baseline();
    let marker = project.home.path().join("client-ran");
    // A shell redirect, not `touch`: the stub PATH holds no coreutils.
    let script = format!(
        "#!/bin/sh\n: > '{}'\necho 'codex-cli 0.153.0'\n",
        marker.display()
    );
    let path = stub_client_path(&project, &[("codex", &script), ("claude", &script)]);
    let run = |args: &[&str]| {
        let output = project
            .command(&project.root, args)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(
            !marker.exists(),
            "{args:?} executed an optional client executable"
        );
        output
    };
    for args in [
        vec!["status", "--format", "json"],
        vec!["status", "--summary", "--format", "json"],
        vec!["review", "--format", "json"],
        vec!["check", "--format", "json"],
        vec!["lint", "--format", "json"],
        vec!["guidance", "--format", "json"],
        vec!["graph", "--format", "json"],
        vec!["state", "inspect", "--format", "json"],
        vec!["init", "--format", "json"],
        // Skill lifecycle never probes a client either.
        vec!["agent", "status", "--target", "codex", "--format", "json"],
        // Hook status and uninstall stay available without one.
        vec![
            "agent", "hook", "status", "--target", "codex", "--format", "json",
        ],
        vec![
            "agent",
            "hook",
            "uninstall",
            "--target",
            "codex",
            "--format",
            "json",
        ],
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stdout(&output)
        );
    }
    // Only an installation probes, and only its own target.
    let output = project
        .command(
            &project.root,
            &[
                "agent", "hook", "install", "--target", "codex", "--format", "json",
            ],
        )
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(marker.exists(), "installation probes its own target");
}

#[test]
fn the_native_endpoint_rejects_an_explicit_output_format() {
    let project = Project::seed();
    project.baseline();
    let body =
        fixture("codex-stop.json").replace("REPLACED_BY_TEST", project.root.to_str().unwrap());
    let base: Vec<&str> = vec![
        "agent",
        "hook",
        "run",
        "--target",
        "codex",
        "--protocol",
        "1",
        "--configuration-root",
        project.root.to_str().unwrap(),
    ];
    // Both placements of the global option are rejected.
    for extra in [
        vec!["--format", "json"],
        vec!["--format=json"],
        vec!["--format", "human"],
    ] {
        let mut args = base.clone();
        args.extend_from_slice(&extra);
        let output = project.run_stdin(&args, body.as_bytes());
        assert_eq!(
            output.status.code(),
            Some(2),
            "{extra:?}: {}",
            stdout(&output)
        );
        let mut leading = extra.clone();
        leading.extend_from_slice(&base);
        let output = project.run_stdin(&leading, body.as_bytes());
        assert_eq!(
            output.status.code(),
            Some(2),
            "leading {extra:?}: {}",
            stdout(&output)
        );
    }
    // Without the option, the endpoint keeps its native protocol and its
    // fail-open behavior for events.
    let output = project.run_stdin(&base, body.as_bytes());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
    let output = project.run_stdin(&base, b"not an event");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output).trim(), "{}");
}

/// Run the endpoint with a Git stub built from `script`, where `{pid_file}`
/// names a file the stub writes an identifier into. Returns the elapsed
/// time, the output, and that identifier.
fn run_with_git_script(
    project: &Project,
    args: &[&str; 9],
    body: &str,
    label: &str,
    script: &str,
) -> (std::time::Duration, std::process::Output, u32) {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;

    let directory = project
        .home
        .path()
        .join(format!("stub-git-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let pid_file = directory.join("git.pid");
    let _ = std::fs::remove_file(&pid_file);
    let stub = directory.join("git");
    std::fs::write(
        &stub,
        script.replace("{pid_file}", &pid_file.display().to_string()),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let started = std::time::Instant::now();
    let mut child = project
        .command(&project.root, args)
        .env("PATH", directory.display().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let payload = body.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });
    let output = child.wait_with_output().unwrap();
    let elapsed = started.elapsed();
    let _ = writer.join();
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap_or_else(|e| panic!("the stub never recorded an identifier: {e}"))
        .trim()
        .parse()
        .unwrap();
    (elapsed, output, pid)
}

/// Run the endpoint with a Git stub that records its identifier, waits
/// `delay_ms`, and then blocks. Returns the elapsed time, the output, and
/// the identifier of the process that Memoria started.
fn run_with_stub_git(
    project: &Project,
    args: &[&str; 9],
    body: &str,
    delay_ms: u64,
) -> (std::time::Duration, std::process::Output, u32) {
    // `exec` keeps the recorded identifier: the sleep replaces this shell.
    let script = format!(
        "#!/bin/sh\necho $$ > '{{pid_file}}'\n{}exec /bin/sleep 30\n",
        if delay_ms == 0 {
            String::new()
        } else {
            format!("/bin/sleep {}.{:03}\n", delay_ms / 1000, delay_ms % 1000)
        }
    );
    run_with_git_script(project, args, body, &format!("stall{delay_ms}"), &script)
}

/// Whether a process identifier is gone within two seconds. A terminated
/// child can appear briefly as a zombie before its parent's exit reaps it.
fn wait_until_gone(pid: u32) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if !process_is_alive(pid) {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // The state field follows the executable name in parentheses.
            Ok(text) => match text.rsplit_once(')') {
                Some((_, rest)) => !rest.trim_start().starts_with('Z'),
                None => false,
            },
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .arg("-o")
            .arg("state=")
            .output();
        match output {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                !state.is_empty() && !state.starts_with('Z')
            }
            Err(_) => false,
        }
    }
}

#[test]
fn stop_runner_enforces_process_and_output_limits() {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    let project = Project::seed();
    project.baseline();
    let root = project.root.to_str().unwrap().to_string();
    let args = [
        "agent",
        "hook",
        "run",
        "--target",
        "codex",
        "--protocol",
        "1",
        "--configuration-root",
        root.as_str(),
    ];
    // The whole endpoint is bounded, including the client's own five-second
    // timeout. Three seconds is the endpoint budget; the assertions leave
    // room for scheduling without accepting an unbounded wait.
    let ceiling = Duration::from_millis(4_500);

    // 1. A producer holds stdin open below the byte cap and never closes it.
    #[allow(clippy::zombie_processes)]
    let status = {
        let mut child = project
            .command(&project.root, &args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        // A partial event, then silence. The pipe stays open.
        let _ = stdin.write_all(b"{\"hook_event_name\":\"Stop\"");
        let _ = stdin.flush();
        let started = Instant::now();
        let mut finished = None;
        while started.elapsed() < ceiling {
            if let Some(status) = child.try_wait().unwrap() {
                finished = Some(status);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let elapsed = started.elapsed();
        if finished.is_none() {
            let _ = child.kill();
        }
        drop(stdin);
        let _ = child.wait();
        finished.unwrap_or_else(|| {
            panic!("the endpoint was still running after {elapsed:?} with stdin held open")
        })
    };
    assert_eq!(status.code(), Some(0), "the endpoint still exits 0");

    // 2. A stalled Git during discovery must not extend the endpoint, and
    //    must not survive it. The stub records its own identifier and then
    //    blocks in a real sleep, which `exec` keeps under that identifier.
    let body = fixture("codex-stop.json").replace("REPLACED_BY_TEST", &root);
    let (elapsed, output, stalled) = run_with_stub_git(&project, &args, &body, 0);
    assert!(
        elapsed < ceiling,
        "a stalled Git kept the endpoint for {elapsed:?}"
    );
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.trim().starts_with('{'), "one JSON object: {text}");
    assert!(
        wait_until_gone(stalled),
        "discovery process {stalled} outlived the endpoint"
    );

    // 2b. Discovery that finishes just before the deadline leaves no room
    //     for the status child. The endpoint still answers in time, and
    //     nothing it started is still running afterwards.
    let (elapsed, output, late) = run_with_stub_git(&project, &args, &body, 2_800);
    assert!(
        elapsed < ceiling,
        "late discovery kept the endpoint for {elapsed:?}"
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .starts_with('{'),
        "one JSON object"
    );
    assert!(
        wait_until_gone(late),
        "late discovery process {late} outlived the endpoint"
    );

    // 3. A status child that floods stderr and outlives its bound is
    //    terminated with its descendants, and the endpoint still answers.
    let flood = project.home.path().join("flood-bin");
    std::fs::create_dir_all(&flood).unwrap();
    let git_link = flood.join("git");
    let _ = std::fs::remove_file(&git_link);
    std::os::unix::fs::symlink(which("git"), &git_link).unwrap();
    let started = Instant::now();
    let output = project.run_stdin(&args, body.as_bytes());
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(0));
    assert!(
        elapsed < ceiling,
        "an ordinary run took {elapsed:?}, above the endpoint bound"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .starts_with('{'),
        "one JSON object"
    );
}

#[test]
fn a_descendant_that_holds_a_pipe_dies_with_the_endpoint() {
    // The leader exits just before the endpoint's deadline and leaves a
    // descendant holding its output pipes. Answering in time is not enough:
    // that descendant must not outlive the endpoint that started it.
    use std::time::Duration;

    let project = Project::seed();
    project.baseline();
    let root = project.root.to_str().unwrap().to_string();
    let args = [
        "agent",
        "hook",
        "run",
        "--target",
        "codex",
        "--protocol",
        "1",
        "--configuration-root",
        root.as_str(),
    ];
    let ceiling = Duration::from_millis(4_500);
    let body = fixture("codex-stop.json").replace("REPLACED_BY_TEST", &root);

    for (label, redirection) in [
        // The descendant holds stdout and stderr.
        ("both-pipes", ""),
        // The descendant holds stderr only.
        ("stderr-only", ">/dev/null"),
    ] {
        let script = format!(
            "#!/bin/sh\n/bin/sleep 2.900\n/bin/sleep 30 {redirection} &\necho $! > '{{pid_file}}'\nexit 0\n"
        );
        let (elapsed, output, descendant) =
            run_with_git_script(&project, &args, &body, label, &script);
        assert!(elapsed < ceiling, "{label}: the endpoint took {elapsed:?}");
        assert_eq!(output.status.code(), Some(0), "{label}");
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .starts_with('{'),
            "{label}: one JSON object"
        );
        assert!(
            wait_until_gone(descendant),
            "{label}: descendant {descendant} outlived the endpoint"
        );
    }
}

#[test]
fn the_bounded_status_process_terminates_descendants_and_bounds_output() {
    use memoria_application::ports::BoundedStatusProcess as _;
    use std::time::Instant;

    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("descendant-alive");
    // A stand-in for the status executable: it starts a descendant that
    // holds the inherited pipes open and outlives the parent, then sleeps
    // past its own deadline.
    let script = home.path().join("slow-status");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n( while true; do echo alive >> '{}'; sleep 0.2; done ) &\nsleep 30\n",
            marker.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let process = memoria_infrastructure::SelfStatusProcess::new(script.clone());
    let started = Instant::now();
    let output = process
        .run_summary(home.path().to_str().unwrap(), 400, 4096)
        .unwrap();
    let elapsed = started.elapsed();
    assert!(output.timed_out, "the child exceeded its deadline");
    assert!(
        elapsed < std::time::Duration::from_millis(2_000),
        "the bounded call took {elapsed:?}"
    );
    // The descendant is terminated with the group, so the marker stops
    // growing after the call returns.
    let size_after = std::fs::metadata(&marker).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let size_later = std::fs::metadata(&marker).map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        size_after, size_later,
        "a descendant survived the process-group termination"
    );

    // A flood of stderr is discarded in bounded chunks, and stdout stays
    // inside its limit.
    let noisy = home.path().join("noisy-status");
    std::fs::write(
        &noisy,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 400 ]; do\n  echo 'error line that is reasonably long and repeated many times' >&2\n  echo 'stdout line that is also reasonably long and repeated' \n  i=$((i+1))\ndone\n",
    )
    .unwrap();
    std::fs::set_permissions(&noisy, std::fs::Permissions::from_mode(0o755)).unwrap();
    let process = memoria_infrastructure::SelfStatusProcess::new(noisy);
    let output = process
        .run_summary(home.path().to_str().unwrap(), 3_000, 1_024)
        .unwrap();
    assert!(
        output.stdout.len() as u64 <= 1_024,
        "stdout stayed inside its limit: {} bytes",
        output.stdout.len()
    );
    assert!(
        output.output_truncated,
        "the flood was reported as truncated"
    );
}

#[test]
fn a_relocated_executable_can_remove_and_reinstall_its_hook() {
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]).0,
        0
    );
    let original = project.read_string(".codex/hooks.json");
    // The same executable, at a different path.
    let moved_dir = project.home.path().join("moved");
    std::fs::create_dir_all(&moved_dir).unwrap();
    let moved = moved_dir.join("memoria");
    std::fs::copy(memoria_bin(), &moved).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin = stub_client_path(
        &project,
        &[("codex", "#!/bin/sh\necho 'codex-cli 0.153.0'\n")],
    );
    let run_moved = |args: &[&str]| {
        let mut command = std::process::Command::new(&moved);
        command
            .current_dir(&project.root)
            .args(args)
            .args(["--format", "json"])
            .env("PATH", &bin)
            .env("HOME", project.home.path())
            .env("XDG_CONFIG_HOME", project.home.path().join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("CLAUDE_CONFIG_DIR");
        let output = output_when_ready(&mut command);
        (output.status.code().unwrap(), parse_json(&output.stdout))
    };

    // The relocated executable still recognizes its own recorded ownership.
    let (code, status) = run_moved(&["agent", "hook", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(
        get_str(&status, &["data", "plan", "state"]),
        "relocated",
        "intact ownership is not the same as agreement with a new command"
    );
    assert_eq!(project.read_string(".codex/hooks.json"), original);

    // Reinstalling from the new path replaces the owned entry in place.
    let (code, installed) = run_moved(&["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{installed:?}");
    let after = parse_json(&project.read(".codex/hooks.json"));
    let Json::Array(groups) = get(&after, &["hooks", "Stop"]) else {
        panic!()
    };
    assert_eq!(groups.len(), 1, "exactly one owned group remains");
    let Json::Array(handlers) = get(&groups[0], &["hooks"]) else {
        panic!()
    };
    assert!(
        get_str(&handlers[0], &["command"]).contains(moved.to_str().unwrap()),
        "the command now names the relocated executable"
    );
    let (code, status) = run_moved(&["agent", "hook", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(get_str(&status, &["data", "plan", "state"]), "installed");

    // Removal from the relocated executable works too.
    let (code, removed) = run_moved(&["agent", "hook", "uninstall", "--target", "codex"]);
    assert_eq!(code, 0, "{removed:?}");
    assert!(!project.exists(".codex/memoria-hook.json"));
}

#[test]
fn malformed_shared_configuration_is_refused_without_any_write() {
    // A signed Unicode escape is not valid JSON. An accepting reader would
    // silently reinterpret the user's bytes and then rewrite the file.
    for (label, configuration) in [
        ("signed escape", "{\"custom\":\"\\u+001\"}\n"),
        ("short escape", "{\"custom\":\"\\u00\"}\n"),
        ("nonhex escape", "{\"custom\":\"\\u00g1\"}\n"),
        ("lone surrogate", "{\"custom\":\"\\ud83d\"}\n"),
        ("control character", "{\"custom\":\"a\u{1}b\"}\n"),
    ] {
        let project = Project::seed();
        project.baseline();
        project.write(".codex/hooks.json", configuration);
        let before = project.read(".codex/hooks.json");
        for arguments in [
            vec!["agent", "hook", "install", "--target", "codex"],
            vec!["agent", "hook", "uninstall", "--target", "codex"],
        ] {
            let (code, value) = json_with_clients(&project, &arguments);
            assert_eq!(code, 3, "{label} {arguments:?}: {value:?}");
            assert_eq!(
                diagnostic_codes(&value),
                vec!["hook_configuration_invalid"],
                "{label} {arguments:?}"
            );
        }
        assert_eq!(
            project.read(".codex/hooks.json"),
            before,
            "{label}: the file is byte for byte unchanged"
        );
        assert!(
            !project.exists(".codex/memoria-hook.json"),
            "{label}: no ownership record"
        );
        assert!(
            !project.exists(".git/memoria/hook-codex.transaction.json"),
            "{label}: no transaction record"
        );
    }
}

#[test]
fn hook_install_refuses_unsupported_container_shapes_without_writing() {
    for (label, configuration) in [
        (
            "string Stop",
            "{\"hooks\":{\"Stop\":\"user-original-value\"},\"other\":42}",
        ),
        ("string hooks", "{\"hooks\":\"mine\",\"other\":42}"),
        ("array root", "[1,2,3]"),
        ("number Stop", "{\"hooks\":{\"Stop\":7}}"),
    ] {
        let project = Project::seed();
        project.baseline();
        project.write(".codex/hooks.json", configuration);
        let before = project.read_string(".codex/hooks.json");
        let (code, value) =
            json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
        assert_eq!(code, 3, "{label}: {value:?}");
        assert_eq!(
            diagnostic_codes(&value),
            vec!["hook_configuration_invalid"],
            "{label}"
        );
        assert_eq!(
            project.read_string(".codex/hooks.json"),
            before,
            "{label}: the user's data survives byte for byte"
        );
        assert!(!project.exists(".codex/memoria-hook.json"), "{label}");
    }
    // A valid but unusual shape still installs, creating only what is absent.
    let project = Project::seed();
    project.baseline();
    project.write(
        ".codex/hooks.json",
        "{\"hooks\":{\"PreToolUse\":[]},\"unrelated\":{\"deep\":[1,2]}}",
    );
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{value:?}");
    let after = parse_json(&project.read(".codex/hooks.json"));
    assert!(matches!(
        get(&after, &["hooks", "PreToolUse"]),
        Json::Array(_)
    ));
    assert!(matches!(get(&after, &["unrelated"]), Json::Object(_)));
    // Uninstall removes only the Stop container Memoria created.
    assert_eq!(
        json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", "codex"]
        )
        .0,
        0
    );
    let after = parse_json(&project.read(".codex/hooks.json"));
    assert!(matches!(
        get(&after, &["hooks", "PreToolUse"]),
        Json::Array(_)
    ));
    assert!(matches!(get(&after, &["unrelated"]), Json::Object(_)));
}

#[test]
fn inline_uninstall_removes_only_the_exact_owned_group() {
    let project = Project::seed();
    project.baseline();
    project.write(".codex/config.toml", fixture("inline-config.toml"));
    let (code, installed) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{installed:?}");
    let command = get_str(&installed, &["data", "plan", "command"]).to_string();

    // A user group that runs the same command, with its own matcher and
    // timeout, is a different group and must survive.
    let mut text = project.read_string(".codex/config.toml");
    text.push_str(&format!(
        "\n[[hooks.Stop]]\nmatcher = \"user-matcher\"\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = '''{command}'''\ntimeout = 99\n"
    ));
    project.write(".codex/config.toml", &text);
    assert!(
        project
            .read_string(".codex/config.toml")
            .contains("user-matcher")
    );

    let (code, removed) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 0, "{removed:?}");
    let after = project.read_string(".codex/config.toml");
    assert!(
        after.contains("user-matcher"),
        "the user group with the same command survives: {after}"
    );
    assert!(
        after.contains("timeout = 99"),
        "its own timeout survives: {after}"
    );
    assert!(
        after.contains("echo user inline stop"),
        "the unrelated pre-existing group survives: {after}"
    );
    assert!(after.contains("# kept exactly"), "comments survive");
    // Exactly one owned group was removed: the command appears once now.
    assert_eq!(
        after.matches("agent hook run").count(),
        1,
        "only the owned group was removed: {after}"
    );
}

/// The digest Memoria records for configuration bytes.
fn digest(bytes: &[u8]) -> String {
    format!("{:016x}", memoria_infrastructure::hash::xxh3_64(bytes))
}

/// One transaction record, shaped exactly like the ones Memoria writes.
struct Interrupted<'a> {
    operation: &'a str,
    configuration: &'a str,
    expected: Option<String>,
    desired: Option<String>,
    /// The digest of the ownership record the operation found, or `None`
    /// when there was none.
    expected_record: Option<String>,
    entry_hash: &'a str,
    record: Option<&'a str>,
    phase: &'a str,
}

fn write_intent(project: &Project, interrupted: &Interrupted<'_>) -> std::path::PathBuf {
    let private = project.root.join(".git/memoria");
    std::fs::create_dir_all(&private).unwrap();
    let path = private.join("hook-codex.transaction.json");
    let quoted = |value: &Option<String>| match value {
        Some(text) => format!("\"{text}\""),
        None => "null".to_string(),
    };
    std::fs::write(
        &path,
        format!(
            "{{\"version\":3,\"operation\":\"{}\",\"target\":\"codex\",\"configuration\":\"{}\",\"expected_digest\":{},\"desired_digest\":{},\"expected_record\":{},\"entry_hash\":\"{}\",\"desired_record\":{},\"phase\":\"{}\"}}",
            interrupted.operation,
            interrupted.configuration,
            quoted(&interrupted.expected),
            quoted(&interrupted.desired),
            quoted(&interrupted.expected_record),
            interrupted.entry_hash,
            interrupted.record.unwrap_or("null"),
            interrupted.phase,
        ),
    )
    .unwrap();
    path
}

fn entry_hash_of(record: &str) -> String {
    get_str(&parse_json(record.as_bytes()), &["entry_hash"]).to_string()
}

/// Install once, and return the resulting configuration and record text.
fn install_and_capture(project: &Project, configuration: &str) -> (String, String) {
    assert_eq!(
        json_with_clients(project, &["agent", "hook", "install", "--target", "codex"]).0,
        0,
        "the reference installation"
    );
    (
        project.read_string(configuration),
        project.read_string(".codex/memoria-hook.json"),
    )
}

#[test]
fn an_interrupted_install_is_finished_by_the_next_install() {
    // The configuration was written but the ownership record was not. The
    // reported symptom was `hook_unmanaged`: the entry looked like an
    // unowned look-alike. Recovery restores the exact recorded ownership.
    for phase in ["started", "configuration-written"] {
        let project = Project::seed();
        project.baseline();
        let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
        let hash = entry_hash_of(&record);
        std::fs::remove_file(project.root.join(".codex/memoria-hook.json")).unwrap();
        let intent = write_intent(
            &project,
            &Interrupted {
                operation: "install",
                configuration: ".codex/hooks.json",
                expected: None,
                desired: Some(digest(configuration.as_bytes())),
                expected_record: None,
                entry_hash: &hash,
                record: Some(&record),
                phase,
            },
        );
        let (code, value) =
            json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
        assert_eq!(code, 0, "{phase}: {value:?}");
        assert!(!intent.exists(), "{phase}: the recovery record is cleared");
        assert_eq!(
            project.read_string(".codex/memoria-hook.json"),
            record,
            "{phase}: the exact recorded ownership is restored"
        );
        assert_eq!(
            project.read_string(".codex/hooks.json"),
            configuration,
            "{phase}: the configuration is untouched"
        );
        // The installation is ordinary again: uninstall removes everything.
        assert_eq!(
            json_with_clients(
                &project,
                &["agent", "hook", "uninstall", "--target", "codex"]
            )
            .0,
            0
        );
        assert!(!project.exists(".codex/memoria-hook.json"));
    }
}

#[test]
fn an_interrupted_install_that_never_landed_leaves_nothing_behind() {
    // The intent was written, the configuration was not. The next install
    // rolls the record side back and then installs ordinarily.
    let project = Project::seed();
    project.baseline();
    let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
    let hash = entry_hash_of(&record);
    assert_eq!(
        json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", "codex"]
        )
        .0,
        0
    );
    assert!(!project.exists(".codex/hooks.json"), "the file was removed");
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "install",
            configuration: ".codex/hooks.json",
            expected: None,
            desired: Some(digest(configuration.as_bytes())),
            expected_record: None,
            entry_hash: &hash,
            record: Some(&record),
            phase: "started",
        },
    );
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists(), "the recovery record is cleared");
    assert_eq!(project.read_string(".codex/hooks.json"), configuration);
    assert_eq!(project.read_string(".codex/memoria-hook.json"), record);
}

#[test]
fn an_interrupted_uninstall_is_finished_by_the_next_uninstall() {
    // The owned group was removed but the record was not. The reported
    // symptom was `hook_conflict` on every later uninstall.
    let project = Project::seed();
    project.baseline();
    let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
    let hash = entry_hash_of(&record);
    // The configuration reached its post-removal state: Memoria created the
    // file, so removal deletes it.
    std::fs::remove_file(project.root.join(".codex/hooks.json")).unwrap();
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "uninstall",
            configuration: ".codex/hooks.json",
            expected: Some(digest(configuration.as_bytes())),
            desired: None,
            expected_record: Some(digest(record.as_bytes())),
            entry_hash: &hash,
            record: None,
            phase: "configuration-written",
        },
    );
    let (code, value) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists(), "the recovery record is cleared");
    assert!(!project.exists(".codex/memoria-hook.json"));
    assert!(!project.exists(".codex/hooks.json"));

    // The mirror case: the removal never reached the configuration, so the
    // ordinary uninstall continues from the interrupted state.
    let project = Project::seed();
    project.baseline();
    let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
    let hash = entry_hash_of(&record);
    write_intent(
        &project,
        &Interrupted {
            operation: "uninstall",
            configuration: ".codex/hooks.json",
            expected: Some(digest(configuration.as_bytes())),
            desired: None,
            expected_record: Some(digest(record.as_bytes())),
            entry_hash: &hash,
            record: None,
            phase: "started",
        },
    );
    let (code, value) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 0, "{value:?}");
    assert!(!project.exists(".codex/memoria-hook.json"));
    assert!(!project.exists(".codex/hooks.json"));
}

#[test]
fn a_pending_transaction_is_never_reported_as_a_no_op() {
    // An intact installation with an outstanding record. The reported
    // symptom was a successful no-op that left the record behind forever.
    let project = Project::seed();
    project.baseline();
    let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
    let hash = entry_hash_of(&record);
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "install",
            configuration: ".codex/hooks.json",
            expected: None,
            desired: Some(digest(configuration.as_bytes())),
            expected_record: None,
            entry_hash: &hash,
            record: Some(&record),
            phase: "configuration-written",
        },
    );
    // Status reports the artifact and changes nothing at all.
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(get_bool(&value, &["data", "plan", "recovery_needed"]));
    assert!(intent.exists(), "status recovers nothing");
    // A dry run is read-only too.
    let (code, value) = json_with_clients(
        &project,
        &["agent", "hook", "install", "--target", "codex", "--dry-run"],
    );
    assert_eq!(code, 0, "{value:?}");
    assert!(intent.exists(), "a dry run recovers nothing");
    assert_eq!(project.read_string(".codex/hooks.json"), configuration);
    // The explicit operation settles it.
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists(), "the pending record is cleared");
    assert_eq!(project.read_string(".codex/hooks.json"), configuration);
    assert_eq!(project.read_string(".codex/memoria-hook.json"), record);
}

#[test]
fn recovery_refuses_an_outside_edit_and_a_malformed_record() {
    let project = Project::seed();
    project.baseline();
    let (configuration, record) = install_and_capture(&project, ".codex/hooks.json");
    let hash = entry_hash_of(&record);
    let landed = digest(configuration.as_bytes());

    // 1. The configuration matches neither identity: someone edited it in
    //    the interrupted window. Nothing is changed, and the record stays
    //    for the operator to see.
    let edited = configuration.replace("{\n", "{\n  \"user\": 1,\n");
    assert_ne!(edited, configuration, "the fixture really differs");
    project.write(".codex/hooks.json", &edited);
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "install",
            configuration: ".codex/hooks.json",
            expected: None,
            desired: Some(landed.clone()),
            expected_record: None,
            entry_hash: &hash,
            record: Some(&record),
            phase: "configuration-written",
        },
    );
    for arguments in [
        vec!["agent", "hook", "install", "--target", "codex"],
        vec!["agent", "hook", "uninstall", "--target", "codex"],
    ] {
        let (code, value) = json_with_clients(&project, &arguments);
        assert_eq!(code, 3, "{arguments:?}: {value:?}");
        assert_eq!(diagnostic_codes(&value), vec!["hook_conflict"]);
        assert_eq!(
            project.read_string(".codex/hooks.json"),
            edited,
            "{arguments:?}: the outside edit survives byte for byte"
        );
        assert!(intent.exists(), "{arguments:?}: the record is kept");
    }

    // 2. Malformed records are refused without any write.
    project.write(".codex/hooks.json", &configuration);
    let private = project.root.join(".git/memoria");
    let path = private.join("hook-codex.transaction.json");
    let valid = format!(
        "{{\"version\":3,\"operation\":\"install\",\"target\":\"codex\",\"configuration\":\".codex/hooks.json\",\"expected_digest\":null,\"desired_digest\":\"{landed}\",\"expected_record\":null,\"entry_hash\":\"{hash}\",\"desired_record\":{record},\"phase\":\"configuration-written\"}}"
    );
    for (label, text) in [
        ("not json", "{ not json".to_string()),
        ("version 2", valid.replace("\"version\":3", "\"version\":2")),
        (
            "numeric digest",
            valid.replace(&format!("\"{landed}\""), "12345"),
        ),
        (
            "unknown field",
            valid.replace("{\"version\":3", "{\"extra\":true,\"version\":3"),
        ),
        (
            "unknown phase",
            valid.replace("configuration-written", "halfway"),
        ),
        (
            "record without an operation",
            valid.replace(record.as_str(), "null"),
        ),
    ] {
        std::fs::write(&path, &text).unwrap();
        let before = project.read_string(".codex/hooks.json");
        let (code, value) = json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", "codex"],
        );
        assert_eq!(code, 3, "{label}: {value:?}");
        assert_eq!(diagnostic_codes(&value), vec!["hook_conflict"], "{label}");
        assert_eq!(project.read_string(".codex/hooks.json"), before, "{label}");
        assert!(project.exists(".codex/memoria-hook.json"), "{label}");
    }
    // Removing the malformed record restores the ordinary path.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        json_with_clients(
            &project,
            &["agent", "hook", "uninstall", "--target", "codex"]
        )
        .0,
        0
    );
}

#[test]
fn recovery_refuses_an_ownership_record_edited_during_the_interruption() {
    // Planning defers its ordinary gates while a transaction is pending, so
    // recovery is responsible for the identities those gates protect. An
    // ownership record changed in the interrupted window is a conflict, not
    // something to repair: the reported symptom was a silent overwrite of a
    // changed target.
    for (representation, configuration, seed) in [
        ("json", ".codex/hooks.json", None),
        (
            "inline toml",
            ".codex/config.toml",
            Some(
                "[[hooks.Stop]]\nmatcher = \"user\"\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo user inline stop\"\n",
            ),
        ),
    ] {
        for edit in ["target", "entry", "created_containers"] {
            let project = Project::seed();
            project.baseline();
            if let Some(seed) = seed {
                project.write(".codex/config.toml", seed);
            }
            let (installed, record) = install_and_capture(&project, configuration);
            let hash = entry_hash_of(&record);
            let label = format!("{representation}/{edit}");

            // Change exactly one part of the ownership record, and nothing
            // else. The shared configuration stays byte for byte the same.
            let changed = match edit {
                "target" => record.replace("\"target\": \"codex\"", "\"target\": \"claude\""),
                "entry" => record.replace("\"timeout\": 5", "\"timeout\": 9"),
                // Flip whichever provenance the installation recorded.
                _ if record.contains("\"stop\": true") => {
                    record.replace("\"stop\": true", "\"stop\": false")
                }
                _ => record.replace("\"stop\": false", "\"stop\": true"),
            };
            assert_ne!(changed, record, "{label}: the fixture really differs");
            project.write(".codex/memoria-hook.json", &changed);

            // A changed target or entry is refused by the ordinary gates.
            // Changed provenance is not: the entry still hashes to the
            // recorded value, so an ordinary install is a no-op. That is
            // exactly why recovery has to check the record itself.
            let (code, value) =
                json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
            let ordinary = if edit == "created_containers" { 0 } else { 3 };
            assert_eq!(code, ordinary, "{label}: the ordinary path: {value:?}");
            assert_eq!(project.read_string(".codex/memoria-hook.json"), changed);

            // With one pending, the same change must still be refused.
            let intent = write_intent(
                &project,
                &Interrupted {
                    operation: "install",
                    configuration,
                    expected: None,
                    desired: Some(digest(installed.as_bytes())),
                    expected_record: None,
                    entry_hash: &hash,
                    record: Some(&record),
                    phase: "configuration-written",
                },
            );
            for arguments in [
                vec!["agent", "hook", "install", "--target", "codex"],
                vec!["agent", "hook", "uninstall", "--target", "codex"],
            ] {
                let (code, value) = json_with_clients(&project, &arguments);
                assert_eq!(code, 3, "{label} {arguments:?}: {value:?}");
                assert_eq!(
                    diagnostic_codes(&value),
                    vec!["hook_conflict"],
                    "{label} {arguments:?}"
                );
                assert_eq!(
                    project.read_string(".codex/memoria-hook.json"),
                    changed,
                    "{label} {arguments:?}: the edited record survives byte for byte"
                );
                assert_eq!(
                    project.read_string(configuration),
                    installed,
                    "{label} {arguments:?}: the configuration is untouched"
                );
                assert!(intent.exists(), "{label} {arguments:?}: the record is kept");
            }

            // Restoring the record restores the ordinary path.
            project.write(".codex/memoria-hook.json", &record);
            let (code, value) =
                json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
            assert_eq!(code, 0, "{label}: {value:?}");
            assert!(!intent.exists(), "{label}: the transaction is settled");
            assert_eq!(project.read_string(".codex/memoria-hook.json"), record);
        }
    }
}

#[test]
fn recovery_keeps_the_record_of_an_installation_it_did_not_interrupt() {
    // A relocated install was interrupted before it wrote anything. The
    // record on disk belongs to the installation that is still in place, so
    // recovery must leave it alone: removing it would turn an owned entry
    // into an unadoptable look-alike.
    let project = Project::seed();
    project.baseline();
    let (original_configuration, original_record) =
        install_and_capture(&project, ".codex/hooks.json");

    let moved = project.home.path().join("moved-memoria");
    std::fs::copy(memoria_bin(), &moved).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let relocated = moved.display().to_string();
    let (code, value) = json_with_clients_at(
        &project,
        &relocated,
        &["agent", "hook", "install", "--target", "codex"],
    );
    assert_eq!(code, 0, "the relocated installation: {value:?}");
    let relocated_configuration = project.read_string(".codex/hooks.json");
    let relocated_record = project.read_string(".codex/memoria-hook.json");
    let relocated_hash = entry_hash_of(&relocated_record);

    // Rebuild the interrupted state: the original installation is in place,
    // and the relocated install had only written its intent.
    project.write(".codex/hooks.json", &original_configuration);
    project.write(".codex/memoria-hook.json", &original_record);
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "install",
            configuration: ".codex/hooks.json",
            expected: Some(digest(original_configuration.as_bytes())),
            desired: Some(digest(relocated_configuration.as_bytes())),
            expected_record: Some(digest(original_record.as_bytes())),
            entry_hash: &relocated_hash,
            record: Some(&relocated_record),
            phase: "started",
        },
    );
    let (code, value) = json_with_clients_at(
        &project,
        &relocated,
        &["agent", "hook", "install", "--target", "codex"],
    );
    // Had recovery removed the original record, this installation would be
    // an unowned look-alike and the command would refuse.
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists());
    assert_eq!(
        project.read_string(".codex/memoria-hook.json"),
        relocated_record,
        "the relocated installation owns exactly one group"
    );
    assert_eq!(
        project
            .read_string(".codex/hooks.json")
            .matches("agent hook run")
            .count(),
        1,
        "the old group was replaced, not duplicated"
    );
}

#[test]
fn inline_transactions_recover_through_the_public_lifecycle() {
    // The inline TOML representation has the same durable boundaries.
    let project = Project::seed();
    project.baseline();
    project.write(
        ".codex/config.toml",
        "[[hooks.Stop]]\nmatcher = \"user\"\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo user inline stop\"\n",
    );
    let (configuration, record) = install_and_capture(&project, ".codex/config.toml");
    let hash = entry_hash_of(&record);

    // 1. The document was written; the record was not.
    std::fs::remove_file(project.root.join(".codex/memoria-hook.json")).unwrap();
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "install",
            configuration: ".codex/config.toml",
            expected: None,
            desired: Some(digest(configuration.as_bytes())),
            expected_record: None,
            entry_hash: &hash,
            record: Some(&record),
            phase: "configuration-written",
        },
    );
    let (code, value) =
        json_with_clients(&project, &["agent", "hook", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists());
    assert_eq!(project.read_string(".codex/memoria-hook.json"), record);
    assert_eq!(project.read_string(".codex/config.toml"), configuration);

    // 2. The owned group was removed; the record was not.
    let removed = {
        let project = Project::seed();
        project.baseline();
        project.write(
            ".codex/config.toml",
            "[[hooks.Stop]]\nmatcher = \"user\"\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo user inline stop\"\n",
        );
        install_and_capture(&project, ".codex/config.toml");
        assert_eq!(
            json_with_clients(
                &project,
                &["agent", "hook", "uninstall", "--target", "codex"]
            )
            .0,
            0
        );
        project.read_string(".codex/config.toml")
    };
    project.write(".codex/config.toml", &removed);
    let intent = write_intent(
        &project,
        &Interrupted {
            operation: "uninstall",
            configuration: ".codex/config.toml",
            expected: Some(digest(configuration.as_bytes())),
            desired: Some(digest(removed.as_bytes())),
            expected_record: Some(digest(record.as_bytes())),
            entry_hash: &hash,
            record: None,
            phase: "configuration-written",
        },
    );
    let (code, value) = json_with_clients(
        &project,
        &["agent", "hook", "uninstall", "--target", "codex"],
    );
    assert_eq!(code, 0, "{value:?}");
    assert!(!intent.exists());
    assert!(!project.exists(".codex/memoria-hook.json"));
    assert_eq!(project.read_string(".codex/config.toml"), removed);
    assert!(
        project.read_string(".codex/config.toml").contains("user"),
        "the user's own group survives"
    );
}

#[test]
fn a_concurrent_configuration_edit_is_a_conflict_not_an_overwrite() {
    use memoria_application::ports::HookStore as _;

    let project = Project::seed();
    project.baseline();
    project.write(
        ".codex/hooks.json",
        "{\"hooks\":{\"PreToolUse\":[]},\"mine\":1}",
    );
    let store = memoria_infrastructure::FsHookStore::new(
        project.root.clone(),
        project.root.clone(),
        std::path::PathBuf::from(memoria_bin()),
        project.root.join(".git"),
        Box::new(AlwaysSupported),
    );
    let plan = store.plan_install(AgentTargetCodex::TARGET).unwrap();
    // A concurrent writer changes the configuration after the plan.
    project.write(
        ".codex/hooks.json",
        "{\"hooks\":{\"PreToolUse\":[]},\"mine\":2}",
    );
    let before = project.read_string(".codex/hooks.json");
    let failure = store
        .apply_install(&plan)
        .expect_err("a concurrent edit must not be overwritten");
    let message = format!("{failure:?}");
    assert!(message.contains("hook_conflict"), "{message}");
    assert_eq!(
        project.read_string(".codex/hooks.json"),
        before,
        "the concurrent edit survives"
    );
    assert!(!project.exists(".codex/memoria-hook.json"));
}

/// A probe that reports a supported client without running anything.
struct AlwaysSupported;

impl memoria_infrastructure::ClientProbe for AlwaysSupported {
    fn probe(
        &self,
        _: memoria_application::ports::AgentTarget,
    ) -> memoria_infrastructure::client_probe::Probe {
        memoria_infrastructure::client_probe::Probe::Supported(
            memoria_infrastructure::client_probe::CODEX_FLOOR,
        )
    }
}

struct AgentTargetCodex;

impl AgentTargetCodex {
    const TARGET: memoria_application::ports::AgentTarget =
        memoria_application::ports::AgentTarget::Codex;
}
