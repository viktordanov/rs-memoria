//! The TOML configuration contract has exact filenames and no legacy discovery.

mod common;

use common::*;
use memoria_infrastructure::json::Json;

#[test]
fn yaml_root_names_do_not_satisfy_configuration_discovery() {
    for name in ["memoria.yml", "memoria.yaml"] {
        let project = Project::empty_repo();
        project.write(name, "version: 1\n");
        project.write("README.md", "# Root\n");
        let before = project.tree_snapshot();
        let (code, status) = project.json(&["status"]);
        assert_eq!(code, 1, "{name}: {status:?}");
        assert_eq!(diagnostic_codes(&status), ["configuration_missing"]);
        assert_eq!(project.tree_snapshot(), before);
        assert!(!project.exists("memoria.toml"));
    }
}

#[test]
fn init_creates_only_the_toml_contract_and_preserves_legacy_files() {
    let project = Project::empty_repo();
    // Neither YAML nor TOML content under an old filename has a configuration role.
    project.write("memoria.yml", "version: 99\nunknown: true\n");
    project.write("memoria.yaml", "version = 99\n");
    project.write("README.md", "# Root\n\nThe author writes this file.\n");
    // The preview writes nothing at all.
    let before = project.tree_snapshot();
    let (code, preview) = project.json(&["init"]);
    assert_eq!(code, 0, "{preview:?}");
    assert!(!matches!(
        get(&preview, &["data", "applied"]),
        Json::Bool(true)
    ));
    assert_eq!(project.tree_snapshot(), before);
    assert!(!project.exists("memoria.lock"));
    let (code, init) = project.json(&["init", "--apply"]);
    assert_eq!(code, 0, "{init:?}");
    assert_eq!(
        strings(get(&init, &["data", "created"])),
        ["memoria.toml", "memoria.lock"]
    );
    assert!(project.read_string("memoria.toml").contains("version = 3"));
    // Apply never writes README prose.
    assert_eq!(
        project.read_string("README.md"),
        "# Root\n\nThe author writes this file.\n"
    );
    assert!(!project.exists("README.memoria.toml"));
    assert_eq!(
        project.read_string("memoria.yml"),
        "version: 99\nunknown: true\n"
    );
    assert_eq!(project.read_string("memoria.yaml"), "version = 99\n");
    for path in ["memoria.yml", "memoria.yaml"] {
        let (code, status) = project.json(&["status", "--explain", path]);
        assert_eq!(code, 0, "{status:?}");
        assert_eq!(
            get_str(&status, &["data", "explanation", "outcome"]),
            "selected"
        );
    }
    let (code, status) = project.json(&["status", "--explain", "memoria.toml"]);
    assert_eq!(code, 0, "{status:?}");
    assert_ne!(
        get_str(&status, &["data", "explanation", "outcome"]),
        "selected"
    );
}

#[test]
fn legacy_sidecars_are_ordinary_files_without_rules_or_guidance() {
    let project = Project::seed();
    project.write("src/retrieval/README.md", "# Retrieval\n\n<!-- memoria:export id=\"summary\" -->\nRetrieval summary.\n<!-- /memoria:export -->\n");
    project.remove("src/retrieval/README.memoria.toml");
    for name in ["README.memoria.yml", "README.memoria.yaml"] {
        project.write(
            &format!("src/retrieval/{name}"),
            "include:\n  - fixtures/**\ndocumentation:\n  guidance_files: [missing.md]\n",
        );
        // No orphan-sidecar error: this is an ordinary source file.
        project.write(&format!("orphan/{name}"), "not valid configuration: [");
    }
    assert_eq!(project.json(&["init", "--apply"]).0, 0);
    assert_eq!(project.json(&["render"]).0, 0);
    let (code, status) =
        project.json(&["status", "--explain", "src/retrieval/fixtures/sample.txt"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(
        get_str(&status, &["data", "explanation", "outcome"]),
        "excluded"
    );
    for name in ["README.memoria.yml", "README.memoria.yaml"] {
        let (code, status) = project.json(&["status", "--explain", &format!("orphan/{name}")]);
        assert_eq!(code, 0, "{status:?}");
        assert_eq!(
            get_str(&status, &["data", "explanation", "outcome"]),
            "selected"
        );
    }
    // The default manifest names the effective guidance sources without
    // copying their prose.
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&std::fs::read(packet).unwrap());
    let Json::Array(references) = get(&value, &["data", "guidance", "references"]) else {
        panic!()
    };
    assert!(!references.is_empty());
    assert!(references.iter().all(|entry| {
        let source = get_str(entry, &["source"]);
        !source.ends_with(".yml") && !source.ends_with(".yaml")
    }));
}

#[test]
fn toml_sidecar_restores_files_and_reports_exact_guidance_sources() {
    let project = Project::seed();
    project.write("src/retrieval/README.md", "# Retrieval\n\n<!-- memoria:export id=\"summary\" -->\nRetrieval summary.\n<!-- /memoria:export -->\n");
    project.write("src/retrieval/README.memoria.toml", "include = ['fixtures/**']\n[documentation]\nguidance = ['Local TOML rule.']\nguidance_files = ['rules.txt']\n");
    project.write("src/retrieval/rules.txt", "Exact local guidance.\n");
    assert_eq!(project.json(&["init", "--apply"]).0, 0);
    assert_eq!(project.json(&["render"]).0, 0);
    let (code, status) =
        project.json(&["status", "--explain", "src/retrieval/fixtures/sample.txt"]);
    assert_eq!(code, 0, "{status:?}");
    assert_eq!(
        get_str(&status, &["data", "explanation", "outcome"]),
        "selected"
    );
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&std::fs::read(packet).unwrap());
    let Json::Array(references) = get(&value, &["data", "guidance", "references"]) else {
        panic!()
    };
    // The default manifest names every effective source in authored order
    // and carries none of their prose.
    for source in [
        "memoria.toml",
        "src/retrieval/README.memoria.toml",
        "src/retrieval/rules.txt",
    ] {
        assert!(
            references
                .iter()
                .any(|entry| get_str(entry, &["source"]) == source),
            "missing {source}: {references:?}"
        );
    }
    assert!(
        references
            .iter()
            .all(|entry| matches!(entry, Json::Object(map) if !map.contains_key("text"))),
        "a manifest never copies authored guidance prose"
    );
    // The exact text stays one ordinary command away.
    let (code, guidance) = project.json(&["guidance", "src/retrieval/README.md"]);
    assert_eq!(code, 0, "{guidance:?}");
    let Json::Array(entries) = get(&guidance, &["data", "entries"]) else {
        panic!()
    };
    for (source, text) in [
        ("memoria.toml", "Use short sentences."),
        ("src/retrieval/README.memoria.toml", "Local TOML rule."),
        ("src/retrieval/rules.txt", "Exact local guidance.\n"),
    ] {
        assert!(
            entries
                .iter()
                .any(|entry| get_str(entry, &["source"]) == source
                    && get_str(entry, &["text"]) == text),
            "missing {source}: {entries:?}"
        );
    }
    assert_eq!(
        get_str(&guidance, &["data", "digest"]),
        get_str(&value, &["data", "guidance", "digest"])
    );
    // The full export still carries the complete prose for offline reading.
    let (full, _) = project.review_full("src/retrieval/README.md");
    let full = parse_json(&std::fs::read(full).unwrap());
    let Json::Array(full_entries) = get(&full, &["data", "context", "guidance", "entries"]) else {
        panic!()
    };
    assert_eq!(full_entries.len(), entries.len());
}

/// Add entries to the fixture's `section_guidance_files` list.
fn register(project: &Project, extra: &[&str]) {
    let lines: String = extra
        .iter()
        .map(|path| format!("    \"{path}\",\n"))
        .collect();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "    \"docs/templates/agent-rules.md\",\n",
            &format!("    \"docs/templates/agent-rules.md\",\n{lines}"),
        ),
    );
}

/// `(code, path, message)` of every error that `memoria check` reports.
fn errors(project: &Project) -> Vec<(String, String, String)> {
    let (code, value) = project.json(&["check"]);
    assert_ne!(code, 0, "check passed unexpectedly");
    let Json::Array(items) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    items
        .iter()
        .filter(|d| get_str(d, &["severity"]) == "error")
        .map(|d| {
            (
                get_str(d, &["code"]).to_string(),
                match get(d, &["path"]) {
                    Json::String(path) => path.clone(),
                    _ => String::new(),
                },
                get_str(d, &["message"]).to_string(),
            )
        })
        .collect()
}

#[test]
fn a_sidecar_cannot_register_a_section_guide() {
    let project = Project::agent_instructions();
    project.write(
        "src/README.memoria.toml",
        "[documentation]\nsection_guidance_files = [\"../docs/templates/agent-rules.md\"]\n",
    );
    let errors = errors(&project);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, "configuration_invalid");
    assert_eq!(errors[0].1, "src/README.memoria.toml");
    assert!(
        errors[0].2.contains("allowed only in memoria.toml"),
        "{errors:?}"
    );
}

#[test]
fn every_registration_rule_refuses_with_a_named_cause() {
    // A duplicate entry, even when spelled differently.
    let project = Project::agent_instructions();
    register(&project, &["docs/../docs/templates/agent-rules.md"]);
    let found = errors(&project);
    assert_eq!(found[0].0, "guidance_file_invalid", "{found:?}");
    assert!(found[0].2.contains("listed more than once"), "{found:?}");

    // Overlap with project guidance.
    let project = Project::agent_instructions();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "guidance_files = []",
            "guidance_files = [\"docs/templates/agent-rules.md\"]",
        ),
    );
    let found = errors(&project);
    assert!(
        found
            .iter()
            .any(|(code, path, message)| code == "guidance_file_invalid"
                && path == "docs/templates/agent-rules.md"
                && message.contains("already applies as project guidance through `memoria.toml`")),
        "{found:?}"
    );

    // A file that carries a Memoria marker outside code: registering it
    // would untrack a document in silence.
    let project = Project::agent_instructions();
    project.write(
        "docs/templates/marked.md",
        "# Marked\n\n```markdown\n<!-- memoria:export id=\"x\" -->\n```\n\n<!-- memoria:export id=\"summary\" -->\nA summary.\n<!-- /memoria:export -->\n",
    );
    register(&project, &["docs/templates/marked.md"]);
    let found = errors(&project);
    assert!(
        found.iter().any(|(code, path, message)| code == "guidance_file_invalid"
            && path == "docs/templates/marked.md"
            && message == "carries a Memoria marker on line 7, so it would stop being a tracked document; remove the markers or the registration"),
        "{found:?}"
    );

    // Size: 65,536 bytes are accepted, 65,537 are refused.
    let project = Project::agent_instructions();
    project.write("docs/templates/big.md", "a".repeat(65_536));
    register(&project, &["docs/templates/big.md"]);
    let (code, value) = project.json(&["check"]);
    assert_eq!(code, 0, "{value:?}");
    project.write("docs/templates/big.md", "a".repeat(65_537));
    let found = errors(&project);
    assert_eq!(
        found,
        vec![(
            "guidance_file_invalid".to_string(),
            "docs/templates/big.md".to_string(),
            "section guide is 65537 bytes; at most 65536 are permitted".to_string()
        )]
    );

    // Missing, README, and non-UTF-8 files use the existing rules.
    for (path, content, code) in [
        ("docs/templates/missing.md", None, "guidance_file_missing"),
        (
            "docs/README.md",
            Some(&b"# Docs\n"[..]),
            "guidance_file_invalid",
        ),
        (
            "docs/templates/latin1.md",
            Some(&b"caf\xe9\n"[..]),
            "guidance_file_invalid",
        ),
    ] {
        let project = Project::agent_instructions();
        if let Some(content) = content {
            project.write(path, content);
        }
        register(&project, &[path]);
        let found = errors(&project);
        assert!(
            found.iter().any(|(c, p, _)| c == code && p == path),
            "{path}: {found:?}"
        );
    }

    // A path outside the project.
    let project = Project::agent_instructions();
    register(&project, &["../outside.md"]);
    let found = errors(&project);
    assert!(
        found
            .iter()
            .any(|(code, path, _)| code == "guidance_file_invalid" && path == "memoria.toml"),
        "{found:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_section_guide_is_refused() {
    let project = Project::agent_instructions();
    std::os::unix::fs::symlink(
        "agent-rules.md",
        project.root.join("docs/templates/link.md"),
    )
    .unwrap();
    register(&project, &["docs/templates/link.md"]);
    let found = errors(&project);
    assert!(
        found
            .iter()
            .any(|(code, path, message)| code == "guidance_file_invalid"
                && path == "docs/templates/link.md"
                && message.contains("symlink")),
        "{found:?}"
    );
}

#[test]
fn a_git_ignored_section_guide_is_read() {
    let project = Project::agent_instructions();
    project.write(".gitignore", "local-guides/\n");
    project.write(
        "local-guides/agent.md",
        "# Local guide\n\nThe local rule.\n",
    );
    register(&project, &["local-guides/agent.md"]);
    project.write(
        "AGENTS.md",
        project
            .read_string("AGENTS.md")
            .replace("docs/templates/agent-rules.md", "local-guides/agent.md"),
    );
    let (code, value) = project.json(&["guidance", "AGENTS.md"]);
    assert_eq!(code, 0, "{value:?}");
    let Json::Array(items) = get(&value, &["data", "entries"]) else {
        panic!()
    };
    assert!(
        items
            .iter()
            .any(|e| get_str(e, &["source"]) == "local-guides/agent.md"
                && get_str(e, &["text"]) == "# Local guide\n\nThe local rule.\n"),
        "{value:?}"
    );
}

#[test]
fn a_section_guide_key_is_refused_by_name_when_misplaced() {
    // The key lives under [documentation]. Anywhere else, strict parsing
    // names it as an unknown field.
    let project = Project::agent_instructions();
    project.write(
        "memoria.toml",
        project.read_string("memoria.toml").replace(
            "include = []\n",
            "include = []\nsection_guidance_files = []\n",
        ),
    );
    let (code, value) = project.json(&["status"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["configuration_invalid"]);
}
