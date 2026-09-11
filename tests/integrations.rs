//! The `memoria integrations` umbrella and its compatibility guarantees.
//!
//! The preferred spellings are `memoria integrations skill ...` and
//! `memoria integrations hook ...`. Every established `memoria agent ...`
//! spelling keeps working with the same arguments, data, command labels,
//! diagnostics, exit statuses and side effects.

mod common;

use common::{Project, diagnostic_codes, get, get_str, stdout};
use memoria_infrastructure::json::{self, Json, Limits};

/// Run one command under both spellings and require identical envelopes.
fn equivalent(project: &Project, new: &[&str], old: &[&str]) -> Json {
    let (new_code, new_value) = project.json(new);
    let (old_code, old_value) = project.json(old);
    assert_eq!(
        new_code, old_code,
        "exit statuses differ for {new:?} and {old:?}"
    );
    assert_eq!(
        json::to_pretty(&new_value),
        json::to_pretty(&old_value),
        "envelopes differ for {new:?} and {old:?}"
    );
    new_value
}

#[test]
fn skill_status_is_identical_under_both_spellings() {
    let project = Project::seed();
    let value = equivalent(
        &project,
        &["integrations", "skill", "status", "--target", "codex"],
        &["agent", "status", "--target", "codex"],
    );
    // The alias reports the established command label, so an existing script
    // that matches on `command` keeps working.
    assert_eq!(get_str(&value, &["command"]), "agent status");
    assert_eq!(get_str(&value, &["data", "plan", "operation"]), "status");
}

#[test]
fn skill_install_under_the_new_spelling_is_visible_to_the_old_one() {
    let project = Project::seed();
    let output = project.run(&["integrations", "skill", "install", "--target", "codex"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let (code, value) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(code, 0);
    assert_eq!(get_str(&value, &["data", "plan", "state"]), "current");
    assert!(project.exists(".agents/skills/memoria/SKILL.md"));

    // The reverse direction holds too.
    let (code, value) = project.json(&["integrations", "skill", "status", "--target", "codex"]);
    assert_eq!(code, 0);
    assert_eq!(get_str(&value, &["data", "plan", "state"]), "current");
}

#[test]
fn skill_dry_run_and_diagnostics_match_the_old_spelling() {
    let project = Project::seed();
    let value = equivalent(
        &project,
        &[
            "integrations",
            "skill",
            "install",
            "--target",
            "claude",
            "--dry-run",
        ],
        &["agent", "install", "--target", "claude", "--dry-run"],
    );
    assert!(!project.exists(".claude/skills/memoria/SKILL.md"));
    assert_eq!(get_str(&value, &["data", "plan", "state"]), "absent");
}

#[test]
fn ambiguous_target_keeps_its_usage_code_and_exit_status() {
    let project = Project::seed();
    // The fixture already holds .agents; a second agent directory makes the
    // target ambiguous for both spellings.
    std::fs::create_dir_all(project.root.join(".claude")).unwrap();
    let (new_code, new_value) = project.json(&["integrations", "skill", "install"]);
    let (old_code, old_value) = project.json(&["agent", "install"]);
    assert_eq!(new_code, 2);
    assert_eq!(old_code, 2);
    assert_eq!(
        json::to_pretty(&new_value),
        json::to_pretty(&old_value),
        "usage failures must stay identical"
    );
    assert_eq!(diagnostic_codes(&new_value), vec!["target_ambiguous"]);
}

#[test]
fn global_scope_resolves_before_discovery_under_both_spellings() {
    let project = Project::seed();
    let outside = project.home.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();

    let mut command = project.command(
        &outside,
        &[
            "integrations",
            "skill",
            "status",
            "--target",
            "codex",
            "--scope",
            "global",
            "--format",
            "json",
        ],
    );
    let new = command.output().unwrap();
    let mut command = project.command(
        &outside,
        &[
            "agent", "status", "--target", "codex", "--scope", "global", "--format", "json",
        ],
    );
    let old = command.output().unwrap();
    assert_eq!(new.status.code(), old.status.code());
    assert_eq!(stdout(&new), stdout(&old));
    let value = json::parse(&new.stdout, Limits::STATE).unwrap();
    assert_eq!(get_str(&value, &["command"]), "agent");
    assert_eq!(get_str(&value, &["data", "plan", "scope"]), "global");
}

#[test]
fn global_scope_still_rejects_root_under_the_new_spelling() {
    let project = Project::seed();
    let (code, value) = project.json(&[
        "--root",
        project.root.to_str().unwrap(),
        "integrations",
        "skill",
        "status",
        "--target",
        "codex",
        "--scope",
        "global",
    ]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["root_invalid"]);
}

#[test]
fn hook_status_is_identical_under_both_spellings() {
    let project = Project::seed();
    let value = equivalent(
        &project,
        &["integrations", "hook", "status", "--target", "claude"],
        &["agent", "hook", "status", "--target", "claude"],
    );
    assert_eq!(get_str(&value, &["command"]), "agent hook status");
}

#[test]
fn an_installed_launcher_keeps_the_legacy_hook_spelling() {
    let project = Project::seed();
    project.write(".claude/settings.json", "{}\n");
    let output = project.run(&["integrations", "hook", "install", "--target", "claude"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));

    let (code, value) = project.json(&["agent", "hook", "status", "--target", "claude"]);
    assert_eq!(code, 0);
    let command = get_str(&value, &["data", "plan", "command"]);
    // A launcher generated by this release must keep calling the established
    // endpoint, so an existing installation needs no migration and a 0.5.0
    // launcher still works with an older executable.
    assert!(
        command.contains("agent hook run"),
        "installed launcher must keep the legacy spelling: {command}"
    );
    assert!(!command.contains("integrations hook run"), "{command}");
}

#[test]
fn the_native_hook_endpoint_works_under_both_spellings() {
    let project = Project::seed();
    project.baseline();
    let event = format!(
        "{{\"hook_event_name\":\"Stop\",\"cwd\":{:?}}}",
        project.root.display().to_string()
    );
    let root = project.root.display().to_string();
    let new = project.run_stdin(
        &[
            "integrations",
            "hook",
            "run",
            "--target",
            "claude",
            "--protocol",
            "1",
            "--configuration-root",
            &root,
        ],
        event.as_bytes(),
    );
    let old = project.run_stdin(
        &[
            "agent",
            "hook",
            "run",
            "--target",
            "claude",
            "--protocol",
            "1",
            "--configuration-root",
            &root,
        ],
        event.as_bytes(),
    );
    assert_eq!(new.status.code(), Some(0));
    assert_eq!(old.status.code(), Some(0));
    assert_eq!(stdout(&new), stdout(&old));
}

#[test]
fn the_native_hook_endpoint_still_rejects_format_under_the_new_spelling() {
    let project = Project::seed();
    let root = project.root.display().to_string();
    let output = project.run(&[
        "integrations",
        "hook",
        "run",
        "--target",
        "claude",
        "--protocol",
        "1",
        "--configuration-root",
        &root,
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let value = json::parse(&output.stdout, Limits::STATE).unwrap();
    assert_eq!(get_str(&value, &["command"]), "agent hook run");
    assert_eq!(diagnostic_codes(&value), vec!["usage_error"]);
}

#[test]
fn alias_argument_errors_keep_the_established_command_label() {
    // A parse error is produced before the grammar normalizes, so the alias
    // has to be recognized on that path too. An existing script that matches
    // on `command` must see the same envelope for both spellings.
    let project = Project::seed();
    for (old, new) in [
        (
            vec!["agent", "status", "--target", "bogus"],
            vec!["integrations", "skill", "status", "--target", "bogus"],
        ),
        (
            vec!["agent", "install", "--scope", "nonsense"],
            vec!["integrations", "skill", "install", "--scope", "nonsense"],
        ),
        (
            vec!["agent", "hook", "status", "--target", "bogus"],
            vec!["integrations", "hook", "status", "--target", "bogus"],
        ),
        (
            vec!["agent", "hook", "install", "--target", "bogus"],
            vec!["integrations", "hook", "install", "--target", "bogus"],
        ),
    ] {
        let before = project.tree_snapshot();
        let (old_code, old_value) = project.json(&old);
        let (new_code, new_value) = project.json(&new);
        assert_eq!(old_code, 2, "{old:?}");
        assert_eq!(new_code, 2, "{new:?}");
        assert_eq!(
            get_str(&new_value, &["command"]),
            get_str(&old_value, &["command"]),
            "{new:?} must keep the label of {old:?}"
        );
        assert_eq!(get_str(&new_value, &["command"]), "agent", "{new:?}");
        assert_eq!(
            diagnostic_codes(&new_value),
            diagnostic_codes(&old_value),
            "{new:?}"
        );
        assert_eq!(project.tree_snapshot(), before, "{new:?} wrote a file");
    }
}

#[test]
fn alias_argument_errors_keep_the_label_with_a_leading_global_option() {
    let project = Project::seed();
    let root = project.root.to_str().unwrap().to_string();
    let old = project.json(&["--root", &root, "agent", "status", "--target", "bogus"]);
    let new = project.json(&[
        "--root",
        &root,
        "integrations",
        "skill",
        "status",
        "--target",
        "bogus",
    ]);
    assert_eq!(new.0, old.0);
    assert_eq!(get_str(&new.1, &["command"]), "agent");
    assert_eq!(get_str(&old.1, &["command"]), "agent");
}

#[test]
fn the_workflow_branch_keeps_its_own_error_label() {
    let project = Project::seed();
    let (code, value) = project.json(&["integrations", "github", "install", "--runner", "bogus"]);
    assert_eq!(code, 2);
    assert_eq!(get_str(&value, &["command"]), "integrations github install");
    let (code, unknown) = project.json(&["integrations", "nonsense"]);
    assert_eq!(code, 2);
    assert_eq!(get_str(&unknown, &["command"]), "integrations");
}

#[test]
fn the_umbrella_has_no_agent_level_and_no_nested_skill_hook() {
    let project = Project::seed();
    for arguments in [
        vec!["integrations", "agent", "status"],
        vec![
            "integrations",
            "skill",
            "hook",
            "status",
            "--target",
            "claude",
        ],
        vec!["integrations", "hook", "upgrade", "--target", "claude"],
        vec!["integrations", "skill", "run"],
    ] {
        let output = project.run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?} must be a usage error"
        );
    }
}

#[test]
fn help_lists_both_umbrellas_and_the_three_branches() {
    let project = Project::seed();
    let top = project.run(&["--help"]);
    assert_eq!(top.status.code(), Some(0));
    let text = stdout(&top);
    assert!(text.contains("agent"), "{text}");
    assert!(text.contains("integrations"), "{text}");

    let umbrella = stdout(&project.run(&["integrations", "--help"]));
    for branch in ["skill", "hook", "github"] {
        assert!(umbrella.contains(branch), "{umbrella}");
    }

    // Legacy help stays clean: no deprecation text contaminates a script.
    let legacy = stdout(&project.run(&["agent", "--help"]));
    for noise in ["deprecated", "Deprecated", "DEPRECATED", "will be removed"] {
        assert!(!legacy.contains(noise), "{legacy}");
    }
}

#[test]
fn completions_cover_both_spellings() {
    let project = Project::seed();
    for shell in ["bash", "zsh", "fish"] {
        let output = project.run(&["completions", shell]);
        assert_eq!(output.status.code(), Some(0));
        let script = stdout(&output);
        assert!(script.contains("integrations"), "{shell} completions");
        assert!(script.contains("agent"), "{shell} completions");
        assert!(script.contains("github"), "{shell} completions");
    }
}

#[test]
fn skill_and_hook_spellings_leave_review_state_untouched() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    assert_eq!(
        project
            .run(&["integrations", "skill", "install", "--target", "codex"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        project
            .run(&["integrations", "hook", "install", "--target", "codex"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(project.state(), before, "the lock must stay unchanged");
    let (code, _) = project.json(&["check"]);
    assert_eq!(code, 0, "installer spellings stay freshness-neutral");
}

#[test]
fn the_umbrella_reaches_the_workflow_branch() {
    let project = Project::seed();
    let (code, value) = project.json(&["integrations", "github", "status"]);
    assert_eq!(code, 0);
    assert_eq!(get_str(&value, &["command"]), "integrations github status");
    assert_eq!(get_str(&value, &["data", "plan", "state"]), "absent");
    let Json::Object(_) = get(&value, &["data", "plan"]) else {
        panic!("the workflow plan must be an object");
    };
}
