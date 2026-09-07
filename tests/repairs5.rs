//! Regression tests for the round-5 triage findings (MEM-031 … MEM-035).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::*;
use memoria_infrastructure::json::Json;

fn install_clean_filter(
    project: &Project,
    helpers: &std::path::Path,
    attributes_path: &str,
) -> std::path::PathBuf {
    let sentinel = helpers.join("clean-called");
    let helper = helpers.join("clean.sh");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf called >> \"{}\"\ncat\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    project.write(
        attributes_path,
        "src/execution/runner.rs filter=reviewprobe\n",
    );
    project.git(&[
        "config",
        "filter.reviewprobe.clean",
        helper.to_str().unwrap(),
    ]);
    project.git(&[
        "config",
        "filter.reviewprobe.process",
        helper.to_str().unwrap(),
    ]);
    sentinel
}

// MEM-031
#[test]
fn inspection_never_executes_configured_clean_filters() {
    for attributes in [".gitattributes", ".git/info/attributes"] {
        let project = Project::seed();
        let helpers = tempfile::tempdir().unwrap();
        let sentinel = install_clean_filter(&project, helpers.path(), attributes);
        if attributes == ".gitattributes" {
            project.commit_all("attributes");
        }
        project.baseline();
        let _ = fs::remove_file(&sentinel);
        // A same-length tracked edit forces Git's content comparison path.
        let original = project.read_string("src/execution/runner.rs");
        let edited = original.replacen("docs.len()", "docs.cap()", 1);
        assert_eq!(edited.len(), original.len());
        project.write("src/execution/runner.rs", &edited);
        let before = project.tree_snapshot();
        for args in [
            vec!["status"],
            vec!["lint"],
            vec!["review"],
            vec!["review", "src/execution/README.md"],
            vec!["check"],
            vec!["graph"],
            vec!["render", "--dry-run"],
        ] {
            let _ = fs::remove_file(&sentinel);
            let mut json_args = args.clone();
            json_args.extend(["--format", "json"]);
            let output = project.run(&json_args);
            assert!(
                output.status.code() == Some(0)
                    || (args == ["check"] && output.status.code() == Some(1)),
                "{attributes} {args:?}: {}",
                stdout(&output)
            );
            assert!(
                !sentinel.exists(),
                "{attributes} {args:?}: the clean filter ran"
            );
            let _ = project.run(&args);
            assert!(
                !sentinel.exists(),
                "{attributes} {args:?} (human): the clean filter ran"
            );
        }
        assert_eq!(
            project.tree_snapshot(),
            before,
            "{attributes}: inspection wrote nothing"
        );
        // Context is still meaningful: the worktree is reported dirty, and clean once committed.
        let (packet, _) = project.review_packet("src/execution/README.md");
        let value = parse_json(&fs::read(&packet).unwrap());
        assert!(
            get_bool(&value, &["data", "context", "git", "worktree_dirty"]),
            "{attributes}"
        );
        assert!(!sentinel.exists());
        project.ack_ok("src/execution/README.md");
        project.commit_all("edit");
        let _ = fs::remove_file(&sentinel);
        project.append("src/execution/README.md", "\nProse only.\n");
        project.commit_all("prose");
        // The harness's own `git add` legitimately applies the clean filter; Memoria must not.
        let _ = fs::remove_file(&sentinel);
        let (packet, _) = project.review_packet("src/execution/README.md");
        let value = parse_json(&fs::read(&packet).unwrap());
        assert!(
            !get_bool(&value, &["data", "context", "git", "worktree_dirty"]),
            "{attributes}: committed tree is clean"
        );
        assert!(
            !sentinel.exists(),
            "{attributes}: clean-tree context ran no filter"
        );
    }
}

// MEM-032
#[test]
fn reserved_guidance_rules_never_enter_project_policy() {
    // An unmanaged default package with its own .gitignore is backed up by installation.
    let project = Project::seed();
    project.write(".agents/skills/memoria/.gitignore", "*.log\n");
    project.write(".agents/skills/memoria/user.txt", "old user skill\n");
    project.baseline();
    assert_eq!(
        project.json(&["agent", "install", "--target", "codex"]).0,
        0
    );
    assert!(project.exists(".agents/skills/memoria.backup/user.txt"));
    assert!(project.exists(".agents/skills/memoria.backup/.gitignore"));
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "installation changes no policy"
    );
    // Rules inside the installed package, a backup, transaction artifacts, or state stay irrelevant.
    project.write(".agents/skills/memoria/.gitignore", "*.tmp\n");
    project.write(".agents/skills/memoria.backup/.gitignore", "*.other\n");
    project.write(".memoria/.gitignore", "*.bak\n");
    assert_eq!(project.json(&["check"]).0, 0);
    // Custom parent: the pre-install directory is its own documentation boundary, so
    // installing moves only that boundary's content into the backup.
    let project = Project::seed();
    project.write(
        "custom/memoria/README.md",
        "# Old skill\n\n<!-- memoria:export id=\"summary\" -->\nOld.\n<!-- /memoria:export -->\n",
    );
    project.write("custom/memoria/.gitignore", "*.log\n");
    project.write("custom/memoria/user.txt", "old\n");
    project.baseline();
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
    assert_eq!(project.json(&["check"]).0, 0);
    project.write("custom/memoria.backup/.gitignore", "*.other\n");
    assert_eq!(project.json(&["check"]).0, 0);
    // A sibling directory's rules are active project policy as before.
    project.write("custom/memoria.tools/.gitignore", "*.log\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
}

// MEM-033
#[test]
fn ignore_files_inside_excluded_directories_are_inactive_policy() {
    let project = Project::seed();
    project.write("build/tracked.txt", "tracked\n");
    project.commit_all("tracked build output");
    project.write(".gitignore", "ignored-output/**\nbuild/\n");
    project.write("build/.gitignore", "*.log\n");
    project.baseline();
    let (_, explain) = project.json(&["status", "--explain", "build/tracked.txt"]);
    assert_eq!(
        get_str(&explain, &["data", "explanation", "outcome"]),
        "selected",
        "tracked files inside an ignored directory stay eligible"
    );
    project.write("build/.gitignore", "*.other\n");
    assert_eq!(
        project.json(&["check"]).0,
        0,
        "Git never reads that nested rule file"
    );
    // The tracked content itself is still hashed.
    project.append("build/tracked.txt", "more\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    project.ack_ok("README.md");
    // Directories Git does traverse keep active rules (the MEM-008 cases).
    project.write("empty/.gitignore", "*.log\n");
    project.write("empty/cache.log", "x\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    project.ack_ok("README.md");
    project.write("empty/.gitignore", "cache.log\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
}

// MEM-034
#[test]
fn quoted_instructions_with_escaped_quotes_and_hashes_are_accepted() {
    let project = Project::seed();
    project.write("memoria.yml", "version: 1\n\nignore:\n  - \"**/generated/**\"\n  - \"**/fixtures/**\"\n\ndocumentation:\n  instructions:\n    - \"Use \\\" # \\\" for headings.\"  # trailing comment\n    - 'Prefer ''plain'' words # not a comment'\n  instruction_files: [\".agents/writing.md\"] # files\n");
    project.write("src/retrieval/README.memoria.yml", "include:\n  - \"fixtures/**\"\ndocumentation:\n  instructions: [\"Rank \\\"# first\\\"\", 'quote ''ok''']\n");
    project.baseline();
    project.append("src/retrieval/engine.rs", "// edit\n");
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&fs::read(packet).unwrap());
    let Json::Array(instructions) = get(&value, &["data", "context", "instructions"]) else {
        panic!()
    };
    let texts: Vec<&str> = instructions.iter().map(|i| get_str(i, &["text"])).collect();
    for expected in [
        "Use \" # \" for headings.",
        "Prefer 'plain' words # not a comment",
        "Rank \"# first\"",
        "quote 'ok'",
    ] {
        assert!(
            texts.contains(&expected),
            "{expected:?} missing from {texts:?}"
        );
    }
    assert!(
        texts.iter().any(|t| t.contains("Explain each part")),
        "instruction file still loaded"
    );
    // Unsupported forms are still rejected.
    project.write(
        "memoria.yml",
        "version: 1\ndocumentation:\n  instructions:\n    - \"unterminated\n",
    );
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&lint), vec!["configuration_invalid"]);
}

// MEM-035
#[test]
fn ignore_files_without_effective_rules_do_not_change_policy() {
    let project = Project::seed();
    project.baseline();
    let before = project.state();
    project.write("empty/.gitignore", "# comment only\n");
    project.write("src/corpus/.gitignore", "\n   \n# nothing\n");
    project.write(".git/info/exclude", "# comment\n");
    assert_eq!(project.json(&["check"]).0, 0);
    fs::remove_file(project.root.join("empty/.gitignore")).unwrap();
    assert_eq!(project.json(&["check"]).0, 0);
    // A real rule, and its removal, are policy changes.
    project.write("empty/.gitignore", "*.log\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    project.write("empty/.gitignore", "# rule removed\n");
    assert_eq!(project.json(&["check"]).0, 0);
    project.write(".git/info/exclude", "*.tmp\n");
    assert_eq!(project.cause_codes("README.md"), vec!["input_changed"]);
    assert_eq!(project.state(), before);
}
