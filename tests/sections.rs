//! Advisory section mappings, focused review selection, and the snapshot
//! safety rules that no reading-cost result may weaken.
//!
//! Sections are advice. They never create ownership, never carry their own
//! freshness, and never narrow the complete input state that acknowledgement
//! validates. Every test here states what the CLI must refuse.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

const SECTION: &str = "<!-- memoria:section id=\"types\" files=\"types.rs\" -->\n## Types\n\nThe corpus stores ordered documents.\n<!-- /memoria:section -->\n";

/// A seeded project whose corpus README carries one valid advisory section,
/// acknowledged so a focused review has a baseline to reuse.
fn mapped_project() -> Project {
    let project = Project::seed();
    let body = project.read_string("src/corpus/README.md");
    project.write("src/corpus/README.md", format!("{body}\n{SECTION}"));
    project.baseline();
    project.commit_all("mapped baseline");
    project
}

fn review(project: &Project, document: &str) -> Json {
    let (code, value) = project.json(&["review", document]);
    assert_eq!(
        code,
        0,
        "{}",
        memoria_infrastructure::json::to_pretty(&value)
    );
    value
}

fn mode(value: &Json) -> &str {
    get_str(value, &["data", "review", "mode"])
}

fn fallback_codes(value: &Json) -> Vec<String> {
    let Json::Array(reasons) = get(value, &["data", "review", "fallback_reasons"]) else {
        panic!("fallback_reasons must be an array")
    };
    reasons
        .iter()
        .map(|r| get_str(r, &["code"]).to_string())
        .collect()
}

fn section_ids(value: &Json) -> Vec<String> {
    let Json::Array(sections) = get(value, &["data", "review", "sections"]) else {
        panic!("sections must be an array")
    };
    sections
        .iter()
        .map(|s| get_str(s, &["id"]).to_string())
        .collect()
}

/// One named mutation applied to a fresh fixture.
type Mutation = (&'static str, Box<dyn Fn(&Project)>);

/// One named JSON mutation with the message fragment it must produce.
type JsonCase = (&'static str, Box<dyn Fn(&mut Json)>, &'static str);

fn ack_json(
    project: &Project,
    document: &str,
    packet: &std::path::Path,
    token: &str,
) -> std::process::Output {
    project.run(&[
        "ack",
        document,
        "--packet",
        packet.to_str().unwrap(),
        "--token",
        token,
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ])
}

fn token_of(project: &Project, document: &str) -> String {
    get_str(&review(project, document), &["data", "token"]).to_string()
}

#[test]
fn a_mapped_change_suggests_its_section_and_still_requires_the_whole_readme() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// a mapped change\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "focused_candidate");
    assert!(
        fallback_codes(&value).is_empty(),
        "{:?}",
        fallback_codes(&value)
    );
    assert_eq!(section_ids(&value), vec!["types"]);
    // The whole-README pass is never optional, whatever the advice says.
    assert!(get_bool(&value, &["data", "review", "whole_readme_pass"]));
    let Json::Array(sections) = get(&value, &["data", "review", "sections"]) else {
        panic!()
    };
    assert_eq!(get_str(&sections[0], &["heading"]), "Types");
    let Json::Array(lines) = get(&sections[0], &["lines"]) else {
        panic!()
    };
    assert_eq!(lines.len(), 2);
    assert!(matches!(lines[0], Json::Number(n) if n >= 1));
    // The README is always a suggested read, and so is the changed source.
    let Json::Array(inputs) = get(&value, &["data", "inputs"]) else {
        panic!()
    };
    let roles: Vec<(&str, &str)> = inputs
        .iter()
        .map(|i| (get_str(i, &["path"]), get_str(i, &["role"])))
        .collect();
    assert!(
        roles.contains(&("src/corpus/README.md", "whole_readme")),
        "{roles:?}"
    );
    assert!(
        roles.contains(&("src/corpus/types.rs", "changed_source")),
        "{roles:?}"
    );
    // Eligibility is not certification: the counts still describe the
    // complete boundary, not the suggested subset.
    assert_eq!(get_u64(&value, &["data", "counts", "selected_files"]), 1);
    assert_eq!(get_u64(&value, &["data", "counts", "suggested_sources"]), 1);
    // A focused candidate acknowledges normally.
    project.ack_ok("src/corpus/README.md");
    assert_eq!(project.status_label("src/corpus/README.md"), "current");
}

#[test]
fn the_default_manifest_carries_no_bodies_and_no_hunks() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// unique-marker-text\n");
    let output = project.run(&["review", "src/corpus/README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("unique-marker-text"), "no source bytes");
    assert!(!text.contains("\"body\""), "no body field");
    assert!(!text.contains("\"content\""), "no content object");
    assert!(!text.contains("@@"), "no hunks");
    // The full export, asked for explicitly, does carry them.
    let output = project.run(&[
        "review",
        "src/corpus/README.md",
        "--full",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unique-marker-text"));
}

#[test]
fn an_invalid_mapping_warns_and_withdraws_every_suggestion() {
    let project = Project::seed();
    let body = project.read_string("src/corpus/README.md");
    // One good section and one naming a path this README does not own.
    project.write(
        "src/corpus/README.md",
        format!(
            "{body}\n{SECTION}\n<!-- memoria:section id=\"other\" files=\"../execution/runner.rs\" -->\n## Other\n\nText.\n<!-- /memoria:section -->\n"
        ),
    );
    project.baseline();
    project.append("src/corpus/types.rs", "// change\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(fallback_codes(&value).contains(&"mapping_invalid".to_string()));
    assert!(
        section_ids(&value).is_empty(),
        "partial advice cannot narrow a review"
    );
    // The diagnostic is advisory: it names the file and the exact reason.
    let codes: Vec<String> = diagnostic_codes(&value);
    assert!(
        codes.contains(&"section_mapping_invalid".to_string()),
        "{codes:?}"
    );
    // Optional advice alone never fails lint or check.
    assert_eq!(project.json(&["lint"]).0, 0);
    // The review still acknowledges: structure is valid, advice is not.
    project.ack_ok("src/corpus/README.md");
}

#[test]
fn malformed_section_syntax_is_advisory_not_structural() {
    let project = Project::seed();
    let body = project.read_string("src/corpus/README.md");
    project.write(
        "src/corpus/README.md",
        format!("{body}\n<!-- memoria:section files=\"types.rs\" id=\"types\" -->\n## Types\n\nText.\n<!-- /memoria:section -->\n"),
    );
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 0, "a mistyped section never invalidates a README");
    let codes = diagnostic_codes(&lint);
    assert!(
        codes.contains(&"section_mapping_invalid".to_string()),
        "{codes:?}"
    );
    // Severity is a warning, never an error.
    let Json::Array(diagnostics) = get(&lint, &["diagnostics"]) else {
        panic!()
    };
    let entry = diagnostics
        .iter()
        .find(|d| get_str(d, &["code"]) == "section_mapping_invalid")
        .unwrap();
    assert_eq!(get_str(entry, &["severity"]), "warning");
    assert_eq!(get_str(entry, &["path"]), "src/corpus/README.md");
    assert!(matches!(get(entry, &["line"]), Json::Number(n) if *n >= 1));
}

#[test]
fn a_changed_mapping_association_forces_a_full_baseline() {
    let project = mapped_project();
    // The body and heading move, but the association does not change.
    let body = project.read_string("src/corpus/README.md");
    project.write(
        "src/corpus/README.md",
        body.replace(
            "## Types\n\nThe corpus stores ordered documents.",
            "## Type definitions\n\nThe corpus stores an ordered list.\nOne more line.",
        ),
    );
    project.commit_all("reword the section");
    let value = review(&project, "src/corpus/README.md");
    assert!(
        !fallback_codes(&value).contains(&"mapping_changed".to_string()),
        "rewording is not a mapping change: {:?}",
        fallback_codes(&value)
    );
    project.ack_ok("src/corpus/README.md");
    project.commit_all("acknowledge the reworded section");

    // Changing which sources the section names is a mapping change.
    let body = project.read_string("src/corpus/README.md");
    project.write(
        "src/corpus/README.md",
        body.replace("files=\"types.rs\"", "files=\"types.rs types.rs\""),
    );
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    // A duplicate path is refused outright, so the whole mapping is unusable.
    assert!(fallback_codes(&value).contains(&"mapping_invalid".to_string()));
}

#[test]
fn a_new_mapping_cannot_reduce_the_required_scope() {
    // A README reviewed without sections, then given one, must not become a
    // focused candidate on the strength of the new advice alone.
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    let body = project.read_string("src/corpus/README.md");
    project.write("src/corpus/README.md", format!("{body}\n{SECTION}"));
    project.append("src/corpus/types.rs", "// change\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(
        fallback_codes(&value).contains(&"mapping_changed".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    assert!(section_ids(&value).is_empty());
}

#[test]
fn an_unmapped_change_cannot_narrow_the_review() {
    let project = Project::seed();
    let body = project.read_string("src/corpus/README.md");
    // The section maps nothing that is about to change.
    project.write("src/corpus/README.md", format!("{body}\n{SECTION}"));
    project.write("src/corpus/extra.rs", "// unmapped source\n");
    project.baseline();
    project.commit_all("two sources, one mapped");
    project.append("src/corpus/extra.rs", "// change outside the section\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert_eq!(
        fallback_codes(&value),
        vec!["unmapped_change".to_string()],
        "{:?}",
        fallback_codes(&value)
    );
    assert!(section_ids(&value).is_empty());
}

#[test]
fn added_and_removed_sources_always_require_a_full_baseline() {
    let project = mapped_project();
    project.write("src/corpus/added.rs", "// new source\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(fallback_codes(&value).contains(&"path_set_changed".to_string()));
    project.ack_ok("src/corpus/README.md");
    project.commit_all("added source");

    // A rename is reported conservatively as a removal plus an addition.
    fs::rename(
        project.root.join("src/corpus/added.rs"),
        project.root.join("src/corpus/renamed.rs"),
    )
    .unwrap();
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    let kinds: Vec<(&str, &str)> = changes
        .iter()
        .map(|c| (get_str(c, &["identity"]), get_str(c, &["change"])))
        .collect();
    assert!(
        kinds.contains(&("src/corpus/added.rs", "removed")),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&("src/corpus/renamed.rs", "added")),
        "{kinds:?}"
    );
}

#[test]
fn an_added_source_is_a_suggested_read_exactly_once() {
    // SR-002: an added source exists now, so the review must read it. It
    // stays a full baseline, and it never becomes a section suggestion.
    let project = mapped_project();
    project.write("src/corpus/new.rs", "// a brand new source\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(fallback_codes(&value).contains(&"path_set_changed".to_string()));

    let Json::Array(inputs) = get(&value, &["data", "inputs"]) else {
        panic!()
    };
    let added: Vec<&Json> = inputs
        .iter()
        .filter(|i| get_str(i, &["path"]) == "src/corpus/new.rs")
        .collect();
    assert_eq!(added.len(), 1, "exactly once: {inputs:?}");
    assert_eq!(get_str(added[0], &["kind"]), "file");
    assert_eq!(get_str(added[0], &["role"]), "changed_source");
    assert_eq!(get_u64(added[0], &["bytes"]), 22);
    assert!(matches!(get(added[0], &["export_id"]), Json::Null));
    // The current hash is reported, not a previous one.
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    let entry = changes
        .iter()
        .find(|c| get_str(c, &["identity"]) == "src/corpus/new.rs")
        .unwrap();
    assert_eq!(get_str(entry, &["change"]), "added");
    assert_eq!(
        get_str(added[0], &["hash"]),
        get_str(entry, &["after_hash"])
    );
    // An added source has no previous association, so it raises no
    // unmapped-change reason of its own.
    let Json::Array(reasons) = get(&value, &["data", "review", "fallback_reasons"]) else {
        panic!()
    };
    assert!(
        !reasons
            .iter()
            .any(|r| get_str(r, &["code"]) == "unmapped_change"
                && get_str(r, &["identity"]) == "src/corpus/new.rs"),
        "{reasons:?}"
    );
    assert!(section_ids(&value).is_empty());
    project.ack_ok("src/corpus/README.md");
}

#[test]
fn a_removed_source_never_becomes_a_current_read() {
    // SR-002 boundary: removal stays in `changes` only. There is nothing
    // current to open.
    let project = mapped_project();
    project.write("src/corpus/doomed.rs", "// temporary\n");
    project.ack_ok("src/corpus/README.md");
    project.commit_all("two sources");
    project.remove("src/corpus/doomed.rs");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    let Json::Array(changes) = get(&value, &["data", "changes"]) else {
        panic!()
    };
    assert!(
        changes
            .iter()
            .any(|c| get_str(c, &["identity"]) == "src/corpus/doomed.rs"
                && get_str(c, &["change"]) == "removed")
    );
    let Json::Array(inputs) = get(&value, &["data", "inputs"]) else {
        panic!()
    };
    assert!(
        !inputs
            .iter()
            .any(|i| get_str(i, &["path"]) == "src/corpus/doomed.rs"),
        "{inputs:?}"
    );
}

#[test]
fn an_added_import_is_a_suggested_read_after_provider_first_rendering() {
    // SR-002: the added-import branch has the same requirement.
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    // The corpus README starts importing the execution summary.
    let body = project.read_string("src/corpus/README.md");
    project.write(
        "src/corpus/README.md",
        format!(
            "{body}\n<!-- memoria:import src=\"../execution/README.md#summary\" -->\n<!-- /memoria:import -->\n"
        ),
    );
    // Provider first, then render the consumer copy.
    let rendered = project.run(&["render"]);
    assert_eq!(
        rendered.status.code(),
        Some(0),
        "stdout={} stderr={}",
        stdout(&rendered),
        stderr(&rendered)
    );
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(
        fallback_codes(&value).contains(&"imports_changed".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    let Json::Array(inputs) = get(&value, &["data", "inputs"]) else {
        panic!()
    };
    let imports: Vec<&Json> = inputs
        .iter()
        .filter(|i| get_str(i, &["kind"]) == "import")
        .collect();
    assert_eq!(imports.len(), 1, "{inputs:?}");
    assert_eq!(get_str(imports[0], &["path"]), "src/execution/README.md");
    assert_eq!(get_str(imports[0], &["export_id"]), "summary");
    assert_eq!(get_str(imports[0], &["role"]), "current_import");
    assert!(get_u64(imports[0], &["bytes"]) > 0);
    project.ack_ok("src/corpus/README.md");
}

#[test]
fn a_first_review_has_no_baseline_to_reuse() {
    let project = Project::seed();
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert_eq!(fallback_codes(&value), vec!["baseline_missing".to_string()]);
    assert!(matches!(get(&value, &["data", "baseline"]), Json::Null));
}

#[test]
fn unverifiable_reviewed_bytes_refuse_a_focused_review() {
    // The baseline is acknowledged without a commit, so the previous source
    // bytes are not recoverable from local history.
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// uncommitted first change\n");
    project.ack_ok("src/corpus/README.md");
    project.append("src/corpus/types.rs", "// second change\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(
        fallback_codes(&value).contains(&"baseline_unavailable".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    assert_eq!(
        get_str(&value, &["data", "baseline", "evidence_status"]),
        "unavailable"
    );
    // The reason names the input and the exact history reason code, and it
    // never invents a hunk.
    let Json::Array(reasons) = get(&value, &["data", "review", "fallback_reasons"]) else {
        panic!()
    };
    let entry = reasons
        .iter()
        .find(|r| get_str(r, &["code"]) == "baseline_unavailable")
        .unwrap();
    assert!(
        get_str(entry, &["message"]).contains("src/corpus/types.rs"),
        "{}",
        get_str(entry, &["message"])
    );
    // A valid acknowledgement is still possible without historical bytes.
    project.ack_ok("src/corpus/README.md");
}

#[test]
fn policy_guidance_and_semantic_changes_each_force_a_full_baseline() {
    // Effective selection policy.
    let project = mapped_project();
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("version = 2\n", "version = 2\ninclude = [\"nothing/**\"]\n"),
    );
    let value = review(&project, "src/corpus/README.md");
    assert!(fallback_codes(&value).contains(&"policy_changed".to_string()));
    assert_eq!(mode(&value), "full_baseline");

    // Authored guidance.
    let project = mapped_project();
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("Use short sentences.", "Use very short sentences."),
    );
    project.append("src/corpus/types.rs", "// change\n");
    let value = review(&project, "src/corpus/README.md");
    assert!(
        fallback_codes(&value).contains(&"guidance_changed".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    assert!(get_bool(
        &value,
        &["data", "guidance", "changed_since_review"]
    ));

    // An explicit semantic invalidation is a review obligation that no
    // section tag can narrow.
    let project = mapped_project();
    assert_eq!(
        project
            .json(&[
                "invalidate",
                "doc:src/corpus/README.md",
                "--reason",
                "Explain the ordering guarantees clearly.",
            ])
            .0,
        0
    );
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(fallback_codes(&value).contains(&"semantic_invalidation".to_string()));
    let Json::Array(covered) = get(&value, &["data", "covered_invalidations"]) else {
        panic!()
    };
    assert_eq!(covered.len(), 1);
}

#[test]
fn a_changed_import_requires_the_provider_first_and_a_full_baseline() {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    let body = project.read_string("src/execution/README.md");
    project.write(
        "src/execution/README.md",
        body.replace("one at a time.", "one at a time, carefully."),
    );
    project.ack_ok("src/execution/README.md");
    assert_eq!(project.run(&["render"]).status.code(), Some(0));
    // The direct consumer of the changed export is reviewed next.
    let value = review(&project, "src/retrieval/naive/README.md");
    assert_eq!(mode(&value), "full_baseline");
    assert!(
        fallback_codes(&value).contains(&"imports_changed".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    // The changed import is a suggested read with its current identity.
    let Json::Array(inputs) = get(&value, &["data", "inputs"]) else {
        panic!()
    };
    assert!(
        inputs.iter().any(|i| get_str(i, &["kind"]) == "import"
            && get_str(i, &["role"]) == "current_import"),
        "{inputs:?}"
    );
}

#[test]
fn a_new_boundary_over_an_existing_path_is_an_ownership_change() {
    let project = Project::seed();
    // The corpus README owns a nested source before any child boundary exists.
    project.write("src/corpus/inner/thing.rs", "// nested source\n");
    project.baseline();
    project.commit_all("one owner");
    // A new descendant README terminates the parent's coverage of that exact
    // path. The path does not move; only its owner does.
    project.write(
        "src/corpus/inner/README.md",
        "# Inner\n\nInner owns `thing.rs`.\n",
    );
    project.append("src/corpus/README.md", "\nSee [inner](inner/).\n");
    let value = review(&project, "src/corpus/README.md");
    assert_eq!(mode(&value), "full_baseline");
    let codes = fallback_codes(&value);
    assert!(
        codes.contains(&"ownership_changed".to_string()),
        "a path that now answers to another README is an ownership move: {codes:?}"
    );
    let Json::Array(reasons) = get(&value, &["data", "review", "fallback_reasons"]) else {
        panic!()
    };
    let entry = reasons
        .iter()
        .find(|r| get_str(r, &["code"]) == "ownership_changed")
        .unwrap();
    assert_eq!(get_str(entry, &["identity"]), "src/corpus/inner/thing.rs");
}

#[test]
fn the_token_binds_every_component_of_the_review_context() {
    let base = mapped_project();
    base.append("src/corpus/types.rs", "// mapped change\n");
    let before = token_of(&base, "src/corpus/README.md");
    assert!(before.starts_with("mrv3."));
    assert_eq!(before.len(), 21);
    // The same stable snapshot produces the same token twice, and the full
    // export reports the same one.
    assert_eq!(before, token_of(&base, "src/corpus/README.md"));
    let (_, full_token) = base.review_full("src/corpus/README.md");
    assert_eq!(before, full_token);

    // Each independent change moves the token.
    let cases: Vec<Mutation> = vec![
        (
            "an unrelated owned source",
            Box::new(|p: &Project| p.write("src/corpus/unrelated.rs", "// added\n")),
        ),
        (
            "the README body",
            Box::new(|p: &Project| p.append("src/corpus/README.md", "\nMore prose.\n")),
        ),
        (
            "the mapping association",
            Box::new(|p: &Project| {
                let body = p.read_string("src/corpus/README.md");
                p.write(
                    "src/corpus/README.md",
                    body.replace("files=\"types.rs\"", "files=\"unrelated.rs\""),
                );
                p.write("src/corpus/unrelated.rs", "// added\n");
            }),
        ),
        (
            "the effective guidance",
            Box::new(|p: &Project| {
                p.write(
                    "memoria.toml",
                    p.read_string("memoria.toml")
                        .replace("Use short sentences.", "Use very short sentences."),
                )
            }),
        ),
        (
            "the effective selection policy",
            Box::new(|p: &Project| {
                p.write(
                    "memoria.toml",
                    p.read_string("memoria.toml")
                        .replace("version = 2\n", "version = 2\ninclude = [\"nothing/**\"]\n"),
                )
            }),
        ),
    ];
    for (label, mutate) in cases {
        let project = mapped_project();
        project.append("src/corpus/types.rs", "// mapped change\n");
        let unchanged = token_of(&project, "src/corpus/README.md");
        mutate(&project);
        assert_ne!(
            token_of(&project, "src/corpus/README.md"),
            unchanged,
            "{label} must change the token"
        );
    }
}

#[test]
fn an_edit_outside_the_suggested_sections_still_refuses_acknowledgement() {
    let project = mapped_project();
    project.write("src/corpus/other.rs", "// another owned source\n");
    project.ack_ok("src/corpus/README.md");
    project.commit_all("two owned sources");
    project.append("src/corpus/types.rs", "// mapped change\n");
    let (packet, token) = project.review_packet("src/corpus/README.md");
    let before = project.state();
    // A source the manifest never suggested reading changes after capture.
    project.append("src/corpus/other.rs", "// changed behind the reviewer\n");
    let output = ack_json(&project, "src/corpus/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    assert_eq!(
        project.state(),
        before,
        "a refused acknowledgement writes nothing"
    );
    assert_eq!(project.status_label("src/corpus/README.md"), "pending");
}

#[test]
fn a_guidance_edit_after_capture_refuses_acknowledgement() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// mapped change\n");
    let (packet, token) = project.review_packet("src/corpus/README.md");
    let before = project.state();
    project.write(
        "memoria.toml",
        project
            .read_string("memoria.toml")
            .replace("Use short sentences.", "Use very short sentences."),
    );
    let output = ack_json(&project, "src/corpus/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["guidance_changed"]
    );
    assert_eq!(project.state(), before);
    // A fresh manifest reconciles and acknowledges.
    project.ack_ok("src/corpus/README.md");
}

#[test]
fn a_section_edit_after_capture_refuses_acknowledgement() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// mapped change\n");
    let (packet, token) = project.review_packet("src/corpus/README.md");
    let before = project.state();
    let body = project.read_string("src/corpus/README.md");
    project.write(
        "src/corpus/README.md",
        body.replace("## Types", "## Types and shapes"),
    );
    let output = ack_json(&project, "src/corpus/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    assert_eq!(project.state(), before);
}

#[test]
fn the_offline_view_refuses_a_manifest_and_says_what_to_do() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// change\n");
    let (packet, _) = project.review_packet("src/corpus/README.md");
    let output = project.run_in(
        project.packets.path(),
        &[
            "packet",
            "view",
            packet.to_str().unwrap(),
            "--section",
            "content",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["packet_content_unavailable"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let message = get_str(&diagnostics[0], &["message"]);
    assert!(message.contains("--full"), "{message}");
    // The full export is readable offline exactly as before.
    let (full, _) = project.review_full("src/corpus/README.md");
    let output = project.run_in(
        project.packets.path(),
        &[
            "packet",
            "view",
            full.to_str().unwrap(),
            "--section",
            "content",
            "--format",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
}

#[test]
fn artifacts_from_earlier_releases_are_refused_with_regeneration_instructions() {
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// change\n");
    let (packet, token) = project.review_full("src/corpus/README.md");
    let original = parse_json(&fs::read(&packet).unwrap());
    let hasher = memoria_infrastructure::Xxh3Hasher;
    let redigest = |mut value: Json| {
        if let Json::Object(map) = &mut value
            && let Some(Json::Object(data)) = map.get_mut("data")
        {
            data.remove("packet_digest");
        }
        let digest = memoria_infrastructure::packet::packet_digest(&hasher, &value);
        set_path(&mut value, &["data", "packet_digest"], Json::String(digest));
        value
    };
    let write = |name: &str, value: &Json| {
        let path = project.packets.path().join(name);
        fs::write(&path, memoria_infrastructure::json::to_pretty(value)).unwrap();
        path
    };
    let refusal = |output: &std::process::Output| -> (Vec<String>, String) {
        let value = parse_json(&output.stdout);
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        (
            diagnostic_codes(&value),
            get_str(&diagnostics[0], &["message"]).to_string(),
        )
    };

    // SR-003: an artifact from an earlier release was hashed under that
    // release's integrity domain, so the current digest never matches it.
    // Leaving the digest untouched reproduces exactly that shape. The version
    // must be reported, not corruption.
    let mut old_domain = original.clone();
    set_path(&mut old_domain, &["schema_version"], Json::Number(2));
    let old_domain = write("old-domain-envelope.json", &old_domain);
    for output in [
        ack_json(&project, "src/corpus/README.md", &old_domain, &token),
        project.run_in(
            project.packets.path(),
            &[
                "packet",
                "view",
                old_domain.to_str().unwrap(),
                "--format",
                "json",
            ],
        ),
    ] {
        assert_eq!(output.status.code(), Some(2));
        let (codes, message) = refusal(&output);
        assert_eq!(codes, vec!["packet_schema_invalid"], "{message}");
        assert!(message.contains("schema_version is 2"), "{message}");
        assert!(message.contains("memoria review"), "{message}");
        assert!(message.contains("not converted"), "{message}");
        assert!(
            !message.contains("modified after"),
            "an intact old artifact is not corruption: {message}"
        );
    }

    // The same requirement for an old packet body whose envelope version is
    // current, again with the old digest left in place.
    let mut old_packet = original.clone();
    set_path(
        &mut old_packet,
        &["data", "packet_version"],
        Json::Number(2),
    );
    let old_packet = write("old-domain-packet.json", &old_packet);
    let output = ack_json(&project, "src/corpus/README.md", &old_packet, &token);
    assert_eq!(output.status.code(), Some(2));
    let (codes, message) = refusal(&output);
    assert_eq!(codes, vec!["packet_schema_invalid"]);
    assert!(message.contains("packet_version is 2"), "{message}");
    assert!(message.contains("--full"), "{message}");
    assert!(message.contains("not converted"), "{message}");

    // A redigested old version is refused identically: the version decides,
    // whichever domain produced the digest.
    for (label, mutate) in [
        (
            "envelope",
            Box::new(|v: &mut Json| set_path(v, &["schema_version"], Json::Number(2)))
                as Box<dyn Fn(&mut Json)>,
        ),
        (
            "packet",
            Box::new(|v: &mut Json| set_path(v, &["data", "packet_version"], Json::Number(2))),
        ),
    ] {
        let mut value = original.clone();
        mutate(&mut value);
        let path = write(&format!("redigested-{label}.json"), &redigest(value));
        let output = ack_json(&project, "src/corpus/README.md", &path, &token);
        assert_eq!(output.status.code(), Some(2), "{label}");
        let (codes, message) = refusal(&output);
        assert_eq!(codes, vec!["packet_schema_invalid"], "{label}");
        assert!(message.contains("not converted"), "{label}: {message}");
    }

    // An old manifest version gets the manifest regeneration command.
    let (manifest, manifest_token) = project.review_packet("src/corpus/README.md");
    let mut old_manifest = parse_json(&fs::read(&manifest).unwrap());
    set_path(
        &mut old_manifest,
        &["data", "manifest_version"],
        Json::Number(2),
    );
    let path = write("old-manifest.json", &old_manifest);
    let output = ack_json(&project, "src/corpus/README.md", &path, &manifest_token);
    assert_eq!(output.status.code(), Some(2));
    let (codes, message) = refusal(&output);
    assert_eq!(codes, vec!["packet_schema_invalid"]);
    assert!(message.contains("manifest_version is 2"), "{message}");
    assert!(
        !message.contains("--full"),
        "a manifest regenerates without --full: {message}"
    );

    // Positive control: a supported version with tampered content still fails
    // the integrity check. The version fix must not weaken that.
    let mut tampered = original.clone();
    set_path(
        &mut tampered,
        &["data", "content", "readme", "body"],
        Json::String("x".into()),
    );
    let path = write("tampered-current.json", &tampered);
    let output = ack_json(&project, "src/corpus/README.md", &path, &token);
    assert_eq!(output.status.code(), Some(2));
    let (codes, message) = refusal(&output);
    assert_eq!(codes, vec!["packet_integrity_failed"], "{message}");
    assert!(message.contains("modified after"), "{message}");

    // A malformed version header is a schema problem, not a version problem.
    let mut malformed = original.clone();
    set_path(
        &mut malformed,
        &["schema_version"],
        Json::String("3".into()),
    );
    let path = write("malformed-version.json", &redigest(malformed));
    let output = ack_json(&project, "src/corpus/README.md", &path, &token);
    assert_eq!(output.status.code(), Some(2));
    let (codes, _) = refusal(&output);
    assert_eq!(codes, vec!["packet_schema_invalid"]);

    // A retired token spelling is refused before the artifact is read.
    let retired = format!("mrv2.{}", &token[5..]);
    let output = ack_json(&project, "src/corpus/README.md", &packet, &retired);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["token_invalid"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let message = get_str(&diagnostics[0], &["message"]);
    assert!(message.contains("earlier release"), "{message}");
    assert!(message.contains("memoria review"), "{message}");
}

#[test]
fn a_manifest_that_violates_its_own_schema_cannot_acknowledge() {
    // SR-001: the three independently reproduced artifacts. Each one was
    // redigested under the current domain, so only the schema rules stop it.
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// change\n");
    let (path, token) = project.review_packet("src/corpus/README.md");
    let original = parse_json(&fs::read(&path).unwrap());
    let state_before = project.state();
    let hasher = memoria_infrastructure::Xxh3Hasher;
    let redigest = |mut value: Json| {
        if let Json::Object(map) = &mut value
            && let Some(Json::Object(data)) = map.get_mut("data")
        {
            data.remove("artifact_digest");
        }
        let digest = memoria_infrastructure::packet::manifest_digest(&hasher, &value);
        set_path(
            &mut value,
            &["data", "artifact_digest"],
            Json::String(digest),
        );
        value
    };

    let mutations: Vec<JsonCase> = vec![
        (
            "selection-version",
            Box::new(|v: &mut Json| {
                set_path(
                    v,
                    &["data", "snapshot", "selection_version"],
                    Json::Number(99),
                )
            }),
            "selection_version",
        ),
        (
            "missing-inputs",
            Box::new(|v: &mut Json| set_path(v, &["data", "inputs"], Json::Array(vec![]))),
            "whole-README pass",
        ),
        (
            "inconsistent-count",
            Box::new(|v: &mut Json| {
                set_path(v, &["data", "counts", "selected_files"], Json::Number(0))
            }),
            "selected files",
        ),
    ];
    for (label, mutate, expected) in mutations {
        let mut value = original.clone();
        mutate(&mut value);
        let file = project.packets.path().join(format!("{label}.json"));
        fs::write(
            &file,
            memoria_infrastructure::json::to_pretty(&redigest(value)),
        )
        .unwrap();
        let output = ack_json(&project, "src/corpus/README.md", &file, &token);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: {}",
            stdout(&output)
        );
        let value = parse_json(&output.stdout);
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        let message = get_str(&diagnostics[0], &["message"]);
        assert!(message.contains(expected), "{label}: {message}");
        assert_eq!(
            project.state(),
            state_before,
            "{label}: a refused artifact writes nothing"
        );
    }

    // Two more intrinsic rules from the same contract.
    let extra: Vec<JsonCase> = vec![
        (
            "duplicate-identity",
            Box::new(|v: &mut Json| {
                let Json::Array(inputs) = get(v, &["data", "inputs"]).clone() else {
                    panic!()
                };
                // Duplicate a source entry, so the identity rule is the one
                // under test rather than the single-document rule.
                let mut doubled = inputs.clone();
                doubled.push(inputs.last().unwrap().clone());
                set_path(v, &["data", "inputs"], Json::Array(doubled));
            }),
            "repeats a reading identity",
        ),
        (
            "wrong-document",
            Box::new(|v: &mut Json| {
                let Json::Array(inputs) = get(v, &["data", "inputs"]).clone() else {
                    panic!()
                };
                let mut changed = inputs.clone();
                set_path(&mut changed[0], &["path"], Json::String("README.md".into()));
                set_path(v, &["data", "inputs"], Json::Array(changed));
            }),
            "but the artifact describes",
        ),
    ];
    for (label, mutate, expected) in extra {
        let mut value = original.clone();
        mutate(&mut value);
        let file = project.packets.path().join(format!("{label}.json"));
        fs::write(
            &file,
            memoria_infrastructure::json::to_pretty(&redigest(value)),
        )
        .unwrap();
        let output = ack_json(&project, "src/corpus/README.md", &file, &token);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: {}",
            stdout(&output)
        );
        let value = parse_json(&output.stdout);
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        assert!(
            get_str(&diagnostics[0], &["message"]).contains(expected),
            "{label}: {}",
            get_str(&diagnostics[0], &["message"])
        );
        assert_eq!(project.state(), state_before, "{label}");
    }

    // Positive control: the untouched manifest still acknowledges.
    let output = ack_json(&project, "src/corpus/README.md", &path, &token);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_ne!(project.state(), state_before);
}

#[test]
fn a_full_export_with_fabricated_counts_cannot_acknowledge() {
    // SR-001: the shared requirements validation also covers the embedded
    // requirements of a full export, which can be checked against its own
    // complete manifest before the write lock.
    let project = mapped_project();
    project.append("src/corpus/types.rs", "// change\n");
    let (path, token) = project.review_full("src/corpus/README.md");
    let original = parse_json(&fs::read(&path).unwrap());
    let state_before = project.state();
    let hasher = memoria_infrastructure::Xxh3Hasher;
    let redigest = |mut value: Json| {
        if let Json::Object(map) = &mut value
            && let Some(Json::Object(data)) = map.get_mut("data")
        {
            data.remove("packet_digest");
        }
        let digest = memoria_infrastructure::packet::packet_digest(&hasher, &value);
        set_path(&mut value, &["data", "packet_digest"], Json::String(digest));
        value
    };
    for (label, path_to, value_to, expected) in [
        (
            "selection-version",
            vec!["data", "requirements", "snapshot", "selection_version"],
            Json::Number(99),
            "selection_version",
        ),
        (
            "counts",
            vec!["data", "requirements", "counts", "selected_files"],
            Json::Number(0),
            "selected files",
        ),
    ] {
        let mut value = original.clone();
        set_path(&mut value, &path_to, value_to);
        let file = project.packets.path().join(format!("full-{label}.json"));
        fs::write(
            &file,
            memoria_infrastructure::json::to_pretty(&redigest(value)),
        )
        .unwrap();
        let output = ack_json(&project, "src/corpus/README.md", &file, &token);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: {}",
            stdout(&output)
        );
        let value = parse_json(&output.stdout);
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        assert!(
            get_str(&diagnostics[0], &["message"]).contains(expected),
            "{label}: {}",
            get_str(&diagnostics[0], &["message"])
        );
        assert_eq!(project.state(), state_before, "{label}");
    }
    // Positive control.
    let output = ack_json(&project, "src/corpus/README.md", &path, &token);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
}

#[test]
fn section_markers_never_change_selection_or_export_identity() {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    project.append("src/corpus/types.rs", "// pending\n");
    let before_export = {
        let (packet, _) = project.review_full("src/corpus/README.md");
        let value = parse_json(&fs::read(&packet).unwrap());
        let Json::Array(exports) = get(&value, &["data", "context", "exports"]) else {
            panic!()
        };
        get_str(&exports[0], &["hash"]).to_string()
    };
    // Adding a section outside the export body leaves the export alone.
    let body = project.read_string("src/corpus/README.md");
    project.write("src/corpus/README.md", format!("{body}\n{SECTION}"));
    let (packet, _) = project.review_full("src/corpus/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(exports) = get(&value, &["data", "context", "exports"]) else {
        panic!()
    };
    assert_eq!(get_str(&exports[0], &["hash"]), before_export);
    // The selected file set is unchanged: a section names inputs, it does
    // not select them.
    assert_eq!(
        get_u64(
            &value,
            &["data", "requirements", "counts", "selected_files"]
        ),
        1
    );
}

#[test]
fn a_provider_context_change_with_an_equal_export_refuses_the_consumer_token() {
    // The provider closure is deliberately conservative: a provider context
    // change can invalidate a consumer token even when the imported export
    // body stays byte-identical and no document becomes stale.
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    project.append("src/retrieval/naive/search.rs", "// consumer change\n");
    let before = token_of(&project, "src/retrieval/naive/README.md");
    let (packet, token) = project.review_packet("src/retrieval/naive/README.md");
    let state_before = project.state();
    let consumer_guidance = get_str(
        &review(&project, "src/retrieval/naive/README.md"),
        &["data", "guidance", "digest"],
    )
    .to_string();
    let exported = {
        let (full, _) = project.review_full("src/retrieval/naive/README.md");
        let value = parse_json(&fs::read(&full).unwrap());
        let Json::Array(imports) = get(&value, &["data", "content", "imports"]) else {
            panic!()
        };
        get_str(&imports[0], &["hash"]).to_string()
    };

    // Guidance scoped to the provider's directory. Guidance is advisory, so
    // it creates no pending status anywhere, and it does not reach the
    // consumer's own effective guidance.
    project.write(
        "src/execution/README.memoria.toml",
        "[documentation]\nguidance = [\"Name the runner's failure modes.\"]\n",
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        Vec::<String>::new(),
        "advisory guidance never makes a document stale"
    );
    assert_eq!(project.status_label("src/execution/README.md"), "current");

    let value = review(&project, "src/retrieval/naive/README.md");
    assert_eq!(
        get_str(&value, &["data", "guidance", "digest"]),
        consumer_guidance,
        "the consumer's own guidance is untouched"
    );
    let after = get_str(&value, &["data", "token"]).to_string();
    assert_ne!(before, after, "the provider closure enters the token");
    let (full, _) = project.review_full("src/retrieval/naive/README.md");
    let value = parse_json(&fs::read(&full).unwrap());
    let Json::Array(imports) = get(&value, &["data", "content", "imports"]) else {
        panic!()
    };
    assert_eq!(
        get_str(&imports[0], &["hash"]),
        exported,
        "the export body is byte-identical"
    );

    // The captured artifact no longer matches the live context.
    let output = ack_json(&project, "src/retrieval/naive/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3), "{}", stdout(&output));
    let value = parse_json(&output.stdout);
    assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    let Json::Array(changed) = get(&diagnostics[0], &["details", "changed"]) else {
        panic!()
    };
    assert_eq!(changed, &vec![Json::String("context".into())]);
    assert_eq!(project.state(), state_before);
    // A fresh artifact acknowledges the same review.
    project.ack_ok("src/retrieval/naive/README.md");
}

#[test]
fn section_markers_inside_generated_import_bodies_are_inert() {
    let project = Project::seed();
    // The provider publishes a section-like line inside its export body.
    let body = project.read_string("src/execution/README.md");
    project.write(
        "src/execution/README.md",
        body.replace(
            "The runner executes retrieved documents one at a time.",
            "The runner executes documents one at a time.\n\n```markdown\n<!-- memoria:section id=\"fake\" files=\"runner.rs\" -->\n```",
        ),
    );
    project.baseline();
    // The rendered consumer copy carries the same literal text.
    let consumer = project.read_string("src/retrieval/naive/README.md");
    assert!(
        consumer.contains("memoria:section"),
        "the generated body copies the literal example"
    );
    // Neither document declares a section: the text sits inside a fenced
    // example, and a generated body never claims consumer ownership.
    project.append("src/retrieval/naive/search.rs", "// change\n");
    let value = review(&project, "src/retrieval/naive/README.md");
    assert!(section_ids(&value).is_empty());
    assert!(
        !fallback_codes(&value).contains(&"mapping_invalid".to_string()),
        "{:?}",
        fallback_codes(&value)
    );
    assert!(
        !diagnostic_codes(&value).contains(&"section_mapping_invalid".to_string()),
        "{:?}",
        diagnostic_codes(&value)
    );
    assert_eq!(project.json(&["lint"]).0, 0);
}

#[test]
fn the_documented_manifest_example_is_a_real_artifact() {
    // SR-005: the CLI reference carries one captured manifest. The production
    // decoder must accept exactly those bytes, so the documentation and the
    // schema cannot drift apart.
    use memoria_application::packet::ReviewArtifact;
    use memoria_application::ports::ReviewPacketCodec;

    let reference = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/cli.md"),
    )
    .unwrap();
    let marker = "<!-- documented-manifest-example -->\n```json\n";
    let start = reference
        .find(marker)
        .expect("the CLI reference documents a complete manifest")
        + marker.len();
    let end = start
        + reference[start..]
            .find("\n```")
            .expect("the example block is closed");
    let documented = &reference[start..end];

    // It is valid JSON.
    let parsed = memoria_infrastructure::json::parse(
        documented.as_bytes(),
        memoria_infrastructure::json::Limits::PACKET,
    )
    .expect("the documented example parses");
    assert_eq!(
        get(&parsed, &["schema_version"]),
        &Json::Number(3),
        "the example uses the current envelope"
    );

    // The production decoder accepts it, with its own digest intact.
    let hasher = memoria_infrastructure::Xxh3Hasher;
    let codec = memoria_infrastructure::JsonPacketCodec::new(&hasher);
    let artifact = codec
        .decode(documented.as_bytes())
        .expect("the current decoder accepts the documented artifact");
    let ReviewArtifact::Manifest(manifest) = artifact else {
        panic!("the documented example is a review manifest")
    };

    // The decoded values are the ones the reference explains.
    assert_eq!(manifest.document.as_str(), "README.md");
    assert_eq!(manifest.snapshot.selection_version, 1);
    assert_eq!(
        manifest.mode,
        memoria_application::review::ReviewMode::FocusedCandidate
    );
    assert!(manifest.fallback_reasons.is_empty());
    assert_eq!(manifest.sections.len(), 1);
    assert_eq!(manifest.sections[0].id, "persistence");
    assert_eq!(manifest.sections[0].heading, "Saving and synchronizing");
    assert_eq!(manifest.sections[0].sources, vec!["service.go".to_string()]);
    assert_eq!(manifest.inputs.len(), 2);
    assert_eq!(manifest.selected_files, 1);
    assert_eq!(manifest.imports, 0);
    assert!(manifest.token.starts_with("mrv3."));
    assert_eq!(manifest.token.len(), 21);
    assert!(manifest.baseline.is_some());

    // The retained capture under the release artifacts is the same document.
    let capture = std::path::Path::new(
        "/tmp/rs-memoria-0.6.0-section-reviews/impl-round-2-manifest-fixture/manifest.json",
    );
    if let Ok(raw) = std::fs::read_to_string(capture) {
        assert_eq!(
            raw.trim_end(),
            documented,
            "the reference must quote the retained capture verbatim"
        );
    }
}
