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
    let (code, init) = project.json(&["init"]);
    assert_eq!(code, 0, "{init:?}");
    assert_eq!(
        strings(get(&init, &["data", "created"])),
        ["memoria.toml", "README.md", ".memoria/state.json"]
    );
    assert!(project.read_string("memoria.toml").contains("version = 1"));
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
fn legacy_sidecars_are_ordinary_files_without_rules_or_instructions() {
    let project = Project::seed();
    project.write("src/retrieval/README.md", "# Retrieval\n\n<!-- memoria:export id=\"summary\" -->\nRetrieval summary.\n<!-- /memoria:export -->\n");
    project.remove("src/retrieval/README.memoria.toml");
    for name in ["README.memoria.yml", "README.memoria.yaml"] {
        project.write(
            &format!("src/retrieval/{name}"),
            "include:\n  - fixtures/**\ndocumentation:\n  instruction_files: [missing.md]\n",
        );
        // No orphan-sidecar error: this is an ordinary source file.
        project.write(&format!("orphan/{name}"), "not valid configuration: [");
    }
    assert_eq!(project.json(&["init"]).0, 0);
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
    let (packet, _) = project.review_packet("src/retrieval/README.md");
    let value = parse_json(&std::fs::read(packet).unwrap());
    let Json::Array(instructions) = get(&value, &["data", "context", "instructions"]) else {
        panic!()
    };
    assert!(instructions.iter().all(|entry| {
        let source = get_str(entry, &["source"]);
        !source.ends_with(".yml") && !source.ends_with(".yaml")
    }));
}

#[test]
fn toml_sidecar_restores_files_and_reports_exact_instruction_sources() {
    let project = Project::seed();
    project.write("src/retrieval/README.md", "# Retrieval\n\n<!-- memoria:export id=\"summary\" -->\nRetrieval summary.\n<!-- /memoria:export -->\n");
    project.write("src/retrieval/README.memoria.toml", "include = ['fixtures/**']\n[documentation]\ninstructions = ['Local TOML rule.']\ninstruction_files = ['rules.txt']\n");
    project.write("src/retrieval/rules.txt", "Exact local instruction.\n");
    assert_eq!(project.json(&["init"]).0, 0);
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
    let Json::Array(instructions) = get(&value, &["data", "context", "instructions"]) else {
        panic!()
    };
    for (source, text) in [
        ("memoria.toml", "Use short sentences."),
        ("src/retrieval/README.memoria.toml", "Local TOML rule."),
        ("src/retrieval/rules.txt", "Exact local instruction.\n"),
    ] {
        assert!(
            instructions
                .iter()
                .any(|entry| get_str(entry, &["source"]) == source
                    && get_str(entry, &["text"]) == text),
            "missing {source}: {instructions:?}"
        );
    }
}
