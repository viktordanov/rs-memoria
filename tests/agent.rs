//! Managed skill installation for Codex and Claude.

mod common;

use std::fs;

use common::*;

#[test]
fn detection_dry_run_install_reinstall_and_uninstall() {
    let project = Project::seed();
    project.baseline();
    // The fixture has `.agents`, so Codex is detected.
    let (code, dry) = project.json(&["agent", "install", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(get_bool(&dry, &["data", "dry_run"]));
    assert_eq!(get_str(&dry, &["data", "plan", "target"]), "codex");
    assert!(get_str(&dry, &["data", "plan", "destination"]).ends_with(".agents/skills/memoria"));
    assert_eq!(
        strings(get(&dry, &["data", "plan", "writes"])),
        vec!["SKILL.md", ".memoria-install.json"]
    );
    assert!(!project.exists(".agents/skills"));

    let (code, install) = project.json(&["agent", "install"]);
    assert_eq!(code, 0);
    assert!(get_bool(&install, &["data", "applied"]));
    let skill = project.read_string(".agents/skills/memoria/SKILL.md");
    assert!(skill.contains("memoria ack"));
    assert!(project.exists(".agents/skills/memoria/.memoria-install.json"));
    // The installed package is guidance, not a review input.
    assert_eq!(project.json(&["check"]).0, 0);
    let (_, explain) = project.json(&["status", "--explain", ".agents/skills/memoria/SKILL.md"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "excluded"
    );

    let snapshot = project.tree_snapshot();
    let (code, again) = project.json(&["agent", "install"]);
    assert_eq!(code, 0);
    assert!(get_bool(&again, &["data", "plan", "no_change"]));
    assert!(!get_bool(&again, &["data", "applied"]));
    assert_eq!(project.tree_snapshot(), snapshot);

    // Both agent directories present: ambiguous without --target.
    fs::create_dir_all(project.root.join(".claude")).unwrap();
    let (code, ambiguous) = project.json(&["agent", "install"]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&ambiguous), vec!["target_ambiguous"]);
    let (code, claude) = project.json(&["agent", "install", "--target", "claude"]);
    assert_eq!(code, 0);
    assert!(get_str(&claude, &["data", "plan", "destination"]).ends_with(".claude/skills/memoria"));
    assert_eq!(project.json(&["check"]).0, 0);

    let (code, removed) = project.json(&["agent", "uninstall", "--target", "claude"]);
    assert_eq!(code, 0);
    assert!(get_bool(&removed, &["data", "applied"]));
    assert!(!project.exists(".claude/skills/memoria"));
    let (code, missing) = project.json(&["agent", "uninstall", "--target", "claude"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&missing), vec!["skill_not_installed"]);
    let (code, _) = project.json(&["agent", "uninstall", "--target", "codex"]);
    assert_eq!(code, 0);
    assert!(!project.exists(".agents/skills/memoria"));
}

#[test]
fn edited_installed_skill_is_preserved_and_reported() {
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    project.write(".agents/skills/memoria/SKILL.md", "# My edits\n");
    let (code, conflict) = project.json(&["agent", "uninstall", "--target", "codex"]);
    assert_eq!(code, 3);
    assert_eq!(diagnostic_codes(&conflict), vec!["skill_conflict"]);
    assert_eq!(
        project.read_string(".agents/skills/memoria/SKILL.md"),
        "# My edits\n"
    );
    let (code, conflict) = project.json(&["agent", "install", "--target", "codex"]);
    assert_eq!(code, 3);
    assert_eq!(diagnostic_codes(&conflict), vec!["skill_conflict"]);
    assert_eq!(
        project.read_string(".agents/skills/memoria/SKILL.md"),
        "# My edits\n"
    );
    // Unknown files also block a silent removal.
    assert_eq!(
        project
            .json(&["agent", "uninstall", "--target", "codex", "--dry-run"])
            .0,
        3
    );
}

#[test]
fn custom_path_backs_up_unmanaged_content_and_restores_it() {
    let project = Project::seed();
    project.baseline();
    let custom = tempfile::tempdir().unwrap();
    let parent = custom.path().join("skills");
    fs::create_dir_all(parent.join("memoria")).unwrap();
    fs::write(parent.join("memoria/notes.md"), "keep me\n").unwrap();
    let parent_str = parent.to_str().unwrap();
    let (code, no_target) = project.json(&["agent", "install", "--path", parent_str]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&no_target), vec!["target_required"]);
    let (code, plan) = project.json(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--path",
        parent_str,
        "--dry-run",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        strings(get(&plan, &["data", "plan", "replaced"])),
        vec!["notes.md"]
    );
    assert!(get_str(&plan, &["data", "plan", "backup"]).ends_with("memoria.backup"));
    assert!(fs::read_to_string(parent.join("memoria/notes.md")).is_ok());
    let (code, _) = project.json(&[
        "agent", "install", "--target", "claude", "--path", parent_str,
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(parent.join("memoria.backup/notes.md")).unwrap(),
        "keep me\n"
    );
    assert!(parent.join("memoria/SKILL.md").exists());
    let (code, _) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "claude",
        "--path",
        parent_str,
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        fs::read_to_string(parent.join("memoria/notes.md")).unwrap(),
        "keep me\n"
    );
    assert!(!parent.join("memoria/SKILL.md").exists());
}
