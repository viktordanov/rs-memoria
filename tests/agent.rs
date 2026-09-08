//! Managed skill installation for Codex and Claude.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

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
    // Uninstall is idempotent: an absent package succeeds without writes.
    let (code, missing) = project.json(&["agent", "uninstall", "--target", "claude"]);
    assert_eq!(code, 0, "{missing:?}");
    assert!(get_bool(&missing, &["data", "plan", "no_change"]));
    assert_eq!(get_str(&missing, &["data", "plan", "state"]), "absent");
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
    // A path outside the worktree never implies a global installation.
    let (code, outside) = project.json(&[
        "agent", "install", "--target", "claude", "--path", parent_str,
    ]);
    assert_eq!(code, 2, "{outside:?}");
    assert_eq!(diagnostic_codes(&outside), vec!["path_outside_worktree"]);
    // Unmanaged content needs explicit replacement.
    let (code, refused) = project.json(&[
        "agent", "install", "--target", "claude", "--scope", "global", "--path", parent_str,
    ]);
    assert_eq!(code, 1, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["skill_replace_required"]);
    let (code, plan) = project.json(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--scope",
        "global",
        "--path",
        parent_str,
        "--replace-existing",
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{plan:?}");
    assert_eq!(
        strings(get(&plan, &["data", "plan", "replaced"])),
        vec!["notes.md"]
    );
    assert!(get_str(&plan, &["data", "plan", "backup"]).ends_with("memoria.backup"));
    assert!(fs::read_to_string(parent.join("memoria/notes.md")).is_ok());
    let (code, applied) = project.json(&[
        "agent",
        "install",
        "--target",
        "claude",
        "--scope",
        "global",
        "--path",
        parent_str,
        "--replace-existing",
    ]);
    assert_eq!(code, 0, "{applied:?}");
    assert_eq!(
        fs::read_to_string(parent.join("memoria.backup/notes.md")).unwrap(),
        "keep me\n"
    );
    assert!(parent.join("memoria/SKILL.md").exists());
    let (code, removed) = project.json(&[
        "agent",
        "uninstall",
        "--target",
        "claude",
        "--scope",
        "global",
        "--path",
        parent_str,
    ]);
    assert_eq!(code, 0, "{removed:?}");
    assert_eq!(
        fs::read_to_string(parent.join("memoria/notes.md")).unwrap(),
        "keep me\n"
    );
    assert!(!parent.join("memoria/SKILL.md").exists());
}

#[test]
fn local_is_default_and_global_requires_explicit_scope() {
    let project = Project::seed();
    project.baseline();
    // The default scope is local: the package lands inside the worktree.
    let (code, installed) = project.json(&["agent", "install", "--target", "codex"]);
    assert_eq!(code, 0, "{installed:?}");
    assert_eq!(get_str(&installed, &["data", "plan", "scope"]), "local");
    assert!(
        get_str(&installed, &["data", "plan", "destination"])
            .starts_with(project.root.to_str().unwrap()),
        "{installed:?}"
    );
    assert!(project.exists(".agents/skills/memoria/SKILL.md"));
    // Global installation requires an explicit target.
    let (code, refused) = project.json(&["agent", "install", "--scope", "global"]);
    assert_eq!(code, 2, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["target_required"]);
    // With a target, the global destination is under the isolated home.
    let (code, global) =
        project.json(&["agent", "install", "--target", "codex", "--scope", "global"]);
    assert_eq!(code, 0, "{global:?}");
    let destination = get_str(&global, &["data", "plan", "destination"]).to_string();
    assert!(
        destination.starts_with(project.home.path().to_str().unwrap()),
        "{destination}"
    );
    assert!(std::path::Path::new(&destination).join("SKILL.md").exists());
    // Status now reports the overlapping local installation.
    let (code, status) =
        project.json(&["agent", "status", "--target", "codex", "--scope", "global"]);
    assert_eq!(code, 0, "{status:?}");
    let Json::Array(overlapping) = get(&status, &["data", "plan", "overlapping"]) else {
        panic!()
    };
    assert!(
        overlapping
            .iter()
            .any(|o| get_str(o, &["scope"]) == "local"),
        "{overlapping:?}"
    );
    // Removing the global package leaves the local one untouched.
    assert_eq!(
        project
            .json(&[
                "agent",
                "uninstall",
                "--target",
                "codex",
                "--scope",
                "global"
            ])
            .0,
        0
    );
    assert!(project.exists(".agents/skills/memoria/SKILL.md"));
}

#[test]
fn global_skill_lifecycle_works_outside_git() {
    // An isolated home with no Git repository and no Memoria configuration.
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let run = |args: &[&str], extra: Option<(&str, &str)>| {
        let mut command = std::process::Command::new(memoria_bin());
        command
            .current_dir(elsewhere.path())
            .args(args)
            .args(["--format", "json"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", home.path())
            .env_remove("CLAUDE_CONFIG_DIR");
        if let Some((key, value)) = extra {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        (output.status.code().unwrap(), parse_json(&output.stdout))
    };
    for target in ["codex", "claude"] {
        let (code, installed) = run(
            &["agent", "install", "--target", target, "--scope", "global"],
            None,
        );
        assert_eq!(code, 0, "{target}: {installed:?}");
        let destination = get_str(&installed, &["data", "plan", "destination"]).to_string();
        assert!(std::path::Path::new(&destination).join("SKILL.md").exists());
        let (code, status) = run(
            &["agent", "status", "--target", target, "--scope", "global"],
            None,
        );
        assert_eq!(code, 0, "{target}");
        assert_eq!(get_str(&status, &["data", "plan", "state"]), "current");
        let (code, _) = run(
            &[
                "agent",
                "uninstall",
                "--target",
                target,
                "--scope",
                "global",
            ],
            None,
        );
        assert_eq!(code, 0, "{target}");
        assert!(!std::path::Path::new(&destination).join("SKILL.md").exists());
    }
    // The Claude configuration override selects a different destination.
    let override_dir = tempfile::tempdir().unwrap();
    let (code, installed) = run(
        &[
            "agent", "install", "--target", "claude", "--scope", "global",
        ],
        Some(("CLAUDE_CONFIG_DIR", override_dir.path().to_str().unwrap())),
    );
    assert_eq!(code, 0, "{installed:?}");
    assert!(override_dir.path().join("skills/memoria/SKILL.md").exists());
    // A relative override is an actionable usage error.
    let (code, refused) = run(
        &["agent", "status", "--target", "claude", "--scope", "global"],
        Some(("CLAUDE_CONFIG_DIR", "relative/path")),
    );
    assert_eq!(code, 2, "{refused:?}");
    assert_eq!(
        diagnostic_codes(&refused),
        vec!["claude_config_dir_relative"]
    );
}

#[test]
fn skill_status_and_dry_run_preserve_every_byte() {
    let project = Project::seed();
    project.baseline();
    let before = project.tree_snapshot();
    // Absent.
    let (code, absent) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{absent:?}");
    assert_eq!(get_str(&absent, &["data", "plan", "state"]), "absent");
    assert_eq!(
        get(&absent, &["data", "plan", "package_version"]),
        &Json::Null
    );
    assert!(!get_str(&absent, &["data", "plan", "embedded_version"]).is_empty());
    assert_eq!(project.tree_snapshot(), before, "status wrote nothing");
    // A dry run reports the plan and writes nothing.
    let (code, dry) = project.json(&["agent", "install", "--target", "codex", "--dry-run"]);
    assert_eq!(code, 0, "{dry:?}");
    assert!(strings(get(&dry, &["data", "plan", "writes"])).contains(&"SKILL.md".to_string()));
    assert_eq!(project.tree_snapshot(), before);
    assert!(!project.exists(".agents/skills/memoria.install.lock"));
    // Current.
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    let (_, current) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(get_str(&current, &["data", "plan", "state"]), "current");
    // Modified: the package is preserved and reported.
    let installed = project.tree_snapshot();
    project.write(".agents/skills/memoria/SKILL.md", "# Edited\n");
    let (code, modified) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{modified:?}");
    assert_eq!(get_str(&modified, &["data", "plan", "state"]), "modified");
    assert!(!strings(get(&modified, &["data", "plan", "modified_paths"])).is_empty());
    assert_eq!(
        project.read_string(".agents/skills/memoria/SKILL.md"),
        "# Edited\n"
    );
    // Unknown files are reported without removal.
    project.write(".agents/skills/memoria/extra.md", "mine\n");
    let (_, unknown) = project.json(&["agent", "status", "--target", "codex"]);
    assert!(!strings(get(&unknown, &["data", "plan", "unknown_paths"])).is_empty());
    let _ = installed;
    // Unmanaged.
    let project = Project::seed();
    project.baseline();
    project.write(".agents/skills/memoria/notes.md", "mine\n");
    let (code, unmanaged) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{unmanaged:?}");
    assert_eq!(get_str(&unmanaged, &["data", "plan", "state"]), "unmanaged");
}

#[test]
fn uninstall_is_idempotent_and_explains_the_retained_lock() {
    let project = Project::seed();
    project.baseline();
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    let lock = ".agents/skills/memoria.install.lock";
    assert!(project.exists(lock), "installation created the parent lock");
    for attempt in 0..2 {
        let (code, removed) = project.json(&["agent", "uninstall", "--target", "codex"]);
        assert_eq!(code, 0, "attempt {attempt}: {removed:?}");
        assert!(!project.exists(".agents/skills/memoria"));
        assert!(
            !project.exists(".agents/skills/memoria/.memoria-install.json"),
            "the package record is gone"
        );
        // The deliberate lock stays, and the report explains it.
        assert!(project.exists(lock), "attempt {attempt}");
        let Json::Array(retained) = get(&removed, &["data", "plan", "retained_artifacts"]) else {
            panic!()
        };
        let entry = retained
            .iter()
            .find(|a| get_str(a, &["path"]).ends_with("memoria.install.lock"))
            .unwrap_or_else(|| panic!("attempt {attempt}: {retained:?}"));
        assert_eq!(get_str(entry, &["reason"]), "synchronization_lock");
        assert!(!get_bool(entry, &["removable_by_uninstall"]));
    }
    // An absent installation creates no directory and no lock.
    let fresh = Project::seed();
    fresh.baseline();
    let before = fresh.tree_snapshot();
    let (code, none) = fresh.json(&["agent", "uninstall", "--target", "codex"]);
    assert_eq!(code, 0, "{none:?}");
    assert!(get_bool(&none, &["data", "plan", "no_change"]));
    assert_eq!(fresh.tree_snapshot(), before);
    assert!(!fresh.exists(".agents/skills/memoria.install.lock"));
}

#[test]
fn upgrade_restores_original_user_content_once() {
    let project = Project::seed();
    project.baseline();
    // Explicit unmanaged content, backed up on replacement.
    project.write(".agents/skills/memoria/user.txt", "original user content\n");
    // Install refuses an older managed package, and upgrade is explicit.
    let (code, refused) = project.json(&["agent", "install", "--target", "codex"]);
    assert_eq!(code, 1, "{refused:?}");
    assert_eq!(diagnostic_codes(&refused), vec!["skill_replace_required"]);
    assert_eq!(
        project
            .json(&[
                "agent",
                "install",
                "--target",
                "codex",
                "--replace-existing"
            ])
            .0,
        0
    );
    assert!(project.exists(".agents/skills/memoria.backup/user.txt"));
    // Upgrading an unchanged current package is a no-op.
    let (code, same) = project.json(&["agent", "upgrade", "--target", "codex"]);
    assert_eq!(code, 0, "{same:?}");
    assert!(get_bool(&same, &["data", "plan", "no_change"]));
    // An older managed package upgrades, and the obsolete managed version
    // does not become a second retained backup.
    for round in 0..2 {
        let record = project.read_string(".agents/skills/memoria/.memoria-install.json");
        let older = record.replace(
            &format!("\"package_version\": \"{}\"", env!("CARGO_PKG_VERSION")),
            "\"package_version\": \"0.0.1\"",
        );
        assert_ne!(
            older, record,
            "round {round}: the fixture must age the record"
        );
        project.write(".agents/skills/memoria/.memoria-install.json", &older);
        let (_, outdated) = project.json(&["agent", "status", "--target", "codex"]);
        assert_eq!(
            get_str(&outdated, &["data", "plan", "state"]),
            "outdated",
            "round {round}"
        );
        // Install refuses an older managed package: upgrading is explicit.
        let (code, refused) = project.json(&["agent", "install", "--target", "codex"]);
        assert_eq!(code, 1, "round {round}: {refused:?}");
        assert_eq!(diagnostic_codes(&refused), vec!["skill_upgrade_required"]);
        let (code, upgraded) = project.json(&["agent", "upgrade", "--target", "codex"]);
        assert_eq!(code, 0, "round {round}: {upgraded:?}");
        let backups: Vec<String> = std::fs::read_dir(project.root.join(".agents/skills"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("memoria.backup"))
            .collect();
        assert_eq!(
            backups,
            vec!["memoria.backup".to_string()],
            "round {round}: only the original user backup is retained"
        );
    }
    // Uninstall restores the original unmanaged directory exactly once.
    assert_eq!(
        project.json(&["agent", "uninstall", "--target", "codex"]).0,
        0
    );
    assert_eq!(
        project.read_string(".agents/skills/memoria/user.txt"),
        "original user content\n"
    );
    assert!(
        !project.exists(".agents/skills/memoria/SKILL.md"),
        "no obsolete Memoria package returned"
    );
    // Upgrading an absent package is an explicit validation failure.
    std::fs::remove_dir_all(project.root.join(".agents/skills/memoria")).unwrap();
    let (code, missing) = project.json(&["agent", "upgrade", "--target", "codex"]);
    assert_eq!(code, 1, "{missing:?}");
    assert_eq!(diagnostic_codes(&missing), vec!["skill_not_installed"]);
}

#[test]
fn overlapping_local_global_and_legacy_packages_are_reported() {
    let project = Project::seed();
    project.baseline();
    // A legacy Codex user package that Memoria reports but never touches.
    let legacy = project.home.path().join(".codex/skills/memoria");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("SKILL.md"), "# Legacy package\n").unwrap();
    // A local and a global installation of the same name.
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    assert_eq!(
        project
            .json(&["agent", "install", "--target", "codex", "--scope", "global"])
            .0,
        0
    );
    let (code, status) = project.json(&["agent", "status", "--target", "codex"]);
    assert_eq!(code, 0, "{status:?}");
    let Json::Array(overlapping) = get(&status, &["data", "plan", "overlapping"]) else {
        panic!()
    };
    let scopes: Vec<String> = overlapping
        .iter()
        .map(|o| get_str(o, &["scope"]).to_string())
        .collect();
    assert!(scopes.contains(&"global".to_string()), "{scopes:?}");
    assert!(scopes.contains(&"legacy".to_string()), "{scopes:?}");
    for entry in overlapping {
        assert!(
            !get_str(entry, &["note"]).is_empty(),
            "each overlap explains the precedence"
        );
    }
    // Removing the local package leaves the other scopes untouched.
    assert_eq!(
        project.json(&["agent", "uninstall", "--target", "codex"]).0,
        0
    );
    assert!(legacy.join("SKILL.md").exists(), "the legacy copy survives");
    assert!(
        project
            .home
            .path()
            .join(".agents/skills/memoria/SKILL.md")
            .exists(),
        "the global copy survives"
    );
}

#[test]
fn managed_backup_chain_requires_verified_ownership() {
    let project = Project::seed();
    project.baseline();
    project.write(".agents/skills/memoria/user.txt", "original\n");
    assert_eq!(
        project
            .json(&[
                "agent",
                "install",
                "--target",
                "codex",
                "--replace-existing"
            ])
            .0,
        0
    );
    let record_path = ".agents/skills/memoria/.memoria-install.json";
    let record = project.read_string(record_path);
    assert!(record.contains("\"schema_version\": 2"), "{record}");
    assert!(record.contains("\"scope\": \"local\""), "{record}");
    assert!(
        record.contains("\"backup\": \"memoria.backup\""),
        "the record names a sibling, never a machine path: {record}"
    );

    // A backup name with a path separator is rejected without writes.
    for invalid in ["../elsewhere", "nested/backup", "unrelated"] {
        project.write(
            record_path,
            record.replace(
                "\"backup\": \"memoria.backup\"",
                &format!("\"backup\": \"{invalid}\""),
            ),
        );
        let (code, refused) = project.json(&["agent", "uninstall", "--target", "codex"]);
        assert_eq!(code, 3, "{invalid}: {refused:?}");
        assert_eq!(
            diagnostic_codes(&refused),
            vec!["skill_conflict"],
            "{invalid}"
        );
        assert!(
            project.exists(".agents/skills/memoria/SKILL.md"),
            "{invalid}: the package is preserved"
        );
        assert_eq!(
            project.read_string(".agents/skills/memoria.backup/user.txt"),
            "original\n",
            "{invalid}: the real backup is untouched"
        );
    }

    // A self-referential chain is a cycle, refused before any removal.
    project.write(record_path, &record);
    let backup_record = ".agents/skills/memoria.backup/.memoria-install.json";
    project.write(backup_record, &record);
    let (code, cycle) = project.json(&["agent", "uninstall", "--target", "codex"]);
    assert_eq!(code, 3, "{cycle:?}");
    assert_eq!(diagnostic_codes(&cycle), vec!["skill_conflict"]);
    assert!(project.exists(".agents/skills/memoria/SKILL.md"));

    // An edited backup is preserved and reported.
    fs::remove_file(project.root.join(backup_record)).unwrap();
    project.write(record_path, &record);
    project.write(".agents/skills/memoria.backup/user.txt", "original\n");
    let (code, ok) = project.json(&["agent", "uninstall", "--target", "codex"]);
    assert_eq!(code, 0, "{ok:?}");
    assert_eq!(
        project.read_string(".agents/skills/memoria/user.txt"),
        "original\n",
        "the verified original backup is restored"
    );
}

#[test]
fn a_change_between_plan_and_apply_is_a_conflict_that_preserves_bytes() {
    use memoria_application::ports::{
        AgentScope, AgentTarget, SkillFailure, SkillOperation, SkillPackageStore, SkillRequest,
    };

    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("skills");
    let store = memoria_infrastructure::FsSkillStore::new(
        dir.path().to_path_buf(),
        "# Skill\n",
        env!("CARGO_PKG_VERSION"),
    );
    let request = |operation: SkillOperation| SkillRequest {
        operation,
        target: AgentTarget::Codex,
        scope: AgentScope::Local,
        parent: parent.display().to_string(),
        replace_existing: true,
        other_parents: vec![],
    };

    // Install, then plan a removal.
    let install = request(SkillOperation::Install);
    let plan = store.plan(&install).unwrap();
    store.apply(&install, &plan).unwrap();
    let uninstall = request(SkillOperation::Uninstall);
    let plan = store.plan(&uninstall).unwrap();

    // A concurrent editor adds a file after the plan and before the apply.
    let unknown = parent.join("memoria/user.txt");
    fs::write(&unknown, "written between plan and apply\n").unwrap();

    let failure = store
        .apply(&uninstall, &plan)
        .expect_err("a stale plan must not be applied");
    assert!(
        matches!(failure, SkillFailure::Conflict { .. }),
        "{failure:?}"
    );
    assert_eq!(
        fs::read_to_string(&unknown).unwrap(),
        "written between plan and apply\n",
        "the concurrent file survives"
    );
    assert!(
        parent.join("memoria/SKILL.md").exists(),
        "the package survives"
    );

    // The same rule protects install and upgrade.
    let install = request(SkillOperation::Install);
    let plan = store.plan(&install);
    // The package is now modified, so planning itself refuses.
    assert!(
        matches!(plan, Err(SkillFailure::Conflict { .. })),
        "{plan:?}"
    );
    fs::remove_file(&unknown).unwrap();

    // A plan made while the package was current, applied after it became
    // unmanaged, is also refused.
    let uninstall = request(SkillOperation::Uninstall);
    let plan = store.plan(&uninstall).unwrap();
    fs::remove_dir_all(parent.join("memoria")).unwrap();
    fs::create_dir_all(parent.join("memoria")).unwrap();
    fs::write(parent.join("memoria/mine.md"), "entirely mine\n").unwrap();
    let failure = store
        .apply(&uninstall, &plan)
        .expect_err("an unmanaged replacement must not be removed");
    assert!(
        matches!(failure, SkillFailure::Conflict { .. }),
        "{failure:?}"
    );
    assert_eq!(
        fs::read_to_string(parent.join("memoria/mine.md")).unwrap(),
        "entirely mine\n"
    );
}
