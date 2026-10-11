//! Reviews of any document: sections over scope, relationships, downstream
//! bounds, full exports, and render convergence (plan §2.7, §6, §7).

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

fn section_warnings(project: &Project) -> Vec<String> {
    project
        .lint_diagnostics("section_mapping_invalid")
        .iter()
        .map(|d| get_str(d, &["message"]).to_string())
        .collect()
}

fn mapping_state(project: &Project, document: &str) -> String {
    let (packet, _) = project.review_full(document);
    let value = parse_json(&fs::read(&packet).unwrap());
    get_str(&value, &["data", "binding", "context", "mapping_state"]).to_string()
}

#[test]
fn sections_validate_against_the_scope() {
    let project = Project::worked_example();
    // Valid: login.rs is in the guide's scope.
    assert!(section_warnings(&project).is_empty());
    project.append("auth/login.rs", "// edit\n");
    assert_eq!(mapping_state(&project, "auth/flows.md"), "valid");

    // A tracked document is not a source: the migration message.
    let project = Project::worked_example();
    project.write(
        "README.md",
        project.read_string("README.md")
            + "\n<!-- memoria:section id=\"docs\" files=\"docs/guide.md\" -->\n## Docs\n\nThe guide.\n<!-- /memoria:section -->\n",
    );
    let warnings = section_warnings(&project);
    assert_eq!(
        warnings,
        vec![
            "section \"docs\": docs/guide.md is now a tracked document, not a source in this document's scope; name the sources that the section describes"
        ]
    );

    // A handed-off source is invalid, and names the handoff.
    let project = Project::worked_example();
    project.write(
        "README.md",
        project.read_string("README.md")
            + "\n<!-- memoria:section id=\"login\" files=\"auth/login.rs\" -->\n## Login\n\nLogin.\n<!-- /memoria:section -->\n",
    );
    let warnings = section_warnings(&project);
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("handed off to auth/README.md"),
        "{warnings:?}"
    );
    project.append("app.rs", "// change\n");
    assert_eq!(mapping_state(&project, "README.md"), "invalid");
    assert!(
        project
            .fallbacks("README.md")
            .iter()
            .any(|(code, _)| code == "mapping_invalid")
    );

    // An unselected file is invalid too. The grammar has no `..`, so a
    // section can never name another folder's file.
    let project = Project::worked_example();
    project.write(
        "docs/guide.md",
        project.read_string("docs/guide.md")
            + "\n<!-- memoria:section id=\"x\" files=\"missing.rs\" -->\n## X\n\nX.\n<!-- /memoria:section -->\n",
    );
    let warnings = section_warnings(&project);
    assert!(
        warnings[0].contains("not a selected regular source"),
        "{warnings:?}"
    );

    // Brace alternation is refused by the parser.
    project.write(
        "docs/guide.md",
        project
            .read_string("docs/guide.md")
            .replace("missing.rs", "{a,b}.rs"),
    );
    let warnings = section_warnings(&project);
    assert!(warnings[0].contains("not permitted"), "{warnings:?}");
}

#[test]
fn every_relationship_kind_appears_where_it_should() {
    let project = Project::worked_example();
    // own text, a scope source, and an import in one review.
    project.append("app.rs", "// source\n");
    project.append("README.md", "\nMore text.\n");
    project.write(
        "docs/guide.md",
        project
            .read_string("docs/guide.md")
            .replace("The guide overview.", "A new overview."),
    );
    project.ack_ok("docs/guide.md");
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    let kinds: Vec<(&str, &str)> = changes
        .iter()
        .map(|c| (get_str(c, &["kind"]), get_str(c, &["relationship", "kind"])))
        .collect();
    assert!(kinds.contains(&("document", "own_text")), "{kinds:?}");
    assert!(kinds.contains(&("file", "scope_source")), "{kinds:?}");
    assert!(kinds.contains(&("import", "import")), "{kinds:?}");
    let import = changes
        .iter()
        .find(|c| get_str(c, &["kind"]) == "import")
        .unwrap();
    assert_eq!(
        get_str(import, &["relationship", "provider"]),
        "docs/guide.md"
    );
    assert_eq!(get_str(import, &["relationship", "export_id"]), "overview");
    // A source covered by two documents says so.
    let project = Project::worked_example();
    project.append("legacy/old.rs", "// shared\n");
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    assert_eq!(
        get_u64(&changes[0], &["relationship", "also_covered_by_total"]),
        1
    );
    let Json::Array(co) = get(&manifest, &["data", "downstream", "co_covering"]) else {
        panic!()
    };
    assert_eq!(get_str(&co[0], &["document"]), "legacy/README.md");
    assert_eq!(get_str(&co[0], &["document_kind"]), "readme");
    assert_eq!(get_str(&co[0], &["status"]), "pending");
    // A policy change is the selection-policy relationship.
    let project = Project::worked_example();
    project.write(".gitignore", "*.tmp\n");
    // The rule reaches every document; the provider comes first.
    project.ack_ok("docs/guide.md");
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let Json::Array(changes) = get(&manifest, &["data", "changes"]) else {
        panic!()
    };
    assert!(
        changes
            .iter()
            .any(|c| get_str(c, &["relationship", "kind"]) == "selection_policy")
    );
}

#[test]
fn scope_and_consumers_are_reported_for_the_reviewed_document() {
    let project = Project::worked_example();
    project.append("docs/notes.md", "change\n");
    let (_, manifest) = project.json(&["review", "docs/guide.md"]);
    assert_eq!(get_str(&manifest, &["data", "document_kind"]), "opted_in");
    assert_eq!(get_u64(&manifest, &["data", "scope", "files"]), 1);
    let Json::Array(incoming) = get(&manifest, &["data", "scope", "handed_off_by"]) else {
        panic!()
    };
    assert_eq!(get_str(&incoming[0], &["parent"]), "README.md");
    assert_eq!(get_str(&incoming[0], &["via"]), "both");
    let Json::Array(consumers) = get(&manifest, &["data", "downstream", "consumers"]) else {
        panic!()
    };
    assert_eq!(consumers.len(), 1);
    assert_eq!(get_str(&consumers[0], &["consumer"]), "README.md");
    assert_eq!(get_str(&consumers[0], &["export_id"]), "overview");
    assert!(get_bool(&consumers[0], &["waits_for_this_document"]));
    // The human view states the scope and the downstream wait.
    let output = project.run(&["review", "docs/guide.md"]);
    let text = stdout(&output);
    assert!(
        text.starts_with("Review docs/guide.md — opted-in document, pending since revision 1\n"),
        "{text}"
    );
    assert!(
        text.contains("Scope: 1 source in docs/ and below"),
        "{text}"
    );
    assert!(
        text.contains("export overview → README.md (waits for this review)"),
        "{text}"
    );
    // The pointer to detail appears only when the view cut a hunk.
    assert!(!text.contains("--details"), "{text}");
    assert!(
        text.contains("      +"),
        "the changed source shows its hunk: {text}"
    );
    // The root's own scope lists its handoffs.
    project.ack_ok("docs/guide.md");
    project.append("app.rs", "// x\n");
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    assert_eq!(get_u64(&manifest, &["data", "scope", "handoffs_total"]), 2);
    assert_eq!(get_u64(&manifest, &["data", "counts", "handoffs"]), 2);
    let output = project.run(&["review", "README.md"]);
    let text = stdout(&output);
    assert!(
        text.contains("hands off auth/ (link, line 3) and docs/ (both, line 3)"),
        "{text}"
    );
}

#[test]
fn downstream_lists_are_bounded_with_totals() {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n\nSee [the provider](p/README.md).\n");
    project.write(
        "p/README.md",
        "# Provider\n\n<!-- memoria:export id=\"summary\" -->\nThe provider.\n<!-- /memoria:export -->\n",
    );
    project.write("p/lib.rs", "fn lib() {}\n");
    project.write("shared.rs", "fn shared() {}\n");
    for i in 0..70 {
        project.write(
            &format!("g{i:02}.md"),
            "# Guide\n\n<!-- memoria:import src=\"p/README.md#summary\" -->\n<!-- /memoria:import -->\n",
        );
    }
    project.commit_all("seventy consumers");
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    while !project.canonical_loop().is_empty() {}
    assert_eq!(project.json(&["check"]).0, 0);
    // Seventy consumers of one export: 64 listed, and the total.
    project.append("p/lib.rs", "// change\n");
    let (_, manifest) = project.json(&["review", "p/README.md"]);
    let Json::Array(consumers) = get(&manifest, &["data", "downstream", "consumers"]) else {
        panic!()
    };
    assert_eq!(consumers.len(), 64);
    assert_eq!(
        get_u64(&manifest, &["data", "downstream", "consumers_total"]),
        70
    );
    let text = stdout(&project.run(&["review", "p/README.md"]));
    assert!(
        text.contains("and 60 more; `memoria graph` lists all"),
        "{text}"
    );
    project.ack_ok("p/README.md");
    // Seventy guides in the root folder share its scope: 64 listed.
    project.append("shared.rs", "// change\n");
    let (code, manifest) = project.json(&["review", "README.md"]);
    assert_eq!(code, 0, "{manifest:?}");
    let Json::Array(co) = get(&manifest, &["data", "downstream", "co_covering"]) else {
        panic!()
    };
    assert_eq!(co.len(), 64);
    assert_eq!(
        get_u64(&manifest, &["data", "downstream", "co_covering_total"]),
        70
    );
    // The artifact still decodes and acknowledges.
    project.ack_ok("README.md");
}

#[test]
fn full_exports_and_packet_view_carry_the_document() {
    let project = Project::worked_example();
    project.append("auth/login.rs", "// edit\n");
    let (packet, token) = project.review_full("auth/flows.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    assert_eq!(get_u64(&value, &["data", "packet_version"]), 5);
    assert_eq!(
        get_str(&value, &["data", "content", "document", "path"]),
        "auth/flows.md"
    );
    assert_eq!(
        get_str(&value, &["data", "binding", "context", "document_kind"]),
        "opted_in"
    );
    let output = project.run(&[
        "packet",
        "view",
        packet.to_str().unwrap(),
        "--section",
        "incremental",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let view = parse_json(&output.stdout);
    assert_eq!(get_u64(&view, &["data", "view_version"]), 3);
    assert_eq!(
        get_str(&view, &["data", "selection", "document", "path"]),
        "auth/flows.md"
    );
    let output = project.ack("auth/flows.md", &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn render_converges_through_opted_in_exports() {
    let project = Project::empty_repo();
    project.write(
        "README.md",
        "# Root\n\n<!-- memoria:import src=\"a/guide.md#one\" -->\n<!-- /memoria:import -->\n",
    );
    project.write(
        "a/guide.md",
        "# A\n\n<!-- memoria:export id=\"one\" -->\nOne.\n<!-- /memoria:export -->\n\n<!-- memoria:import src=\"b/README.md#two\" -->\n<!-- /memoria:import -->\n",
    );
    project.write(
        "a/b/README.md",
        "# B\n\n<!-- memoria:export id=\"two\" -->\nTwo.\n<!-- /memoria:export -->\n",
    );
    project.commit_all("chain");
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    let steps = project.canonical_loop();
    assert!(steps.len() <= 24, "{steps:?}");
    assert_eq!(project.json(&["check"]).0, 0);
    // Changing the deepest export flows up through the opted-in guide.
    project.write(
        "a/b/README.md",
        "# B\n\n<!-- memoria:export id=\"two\" -->\nTwo, changed.\n<!-- /memoria:export -->\n",
    );
    let steps = project.canonical_loop();
    assert!(steps.len() <= 24, "{steps:?}");
    assert!(
        steps.contains(&"render a/guide.md".to_string()),
        "{steps:?}"
    );
    assert_eq!(project.json(&["check"]).0, 0);
}
