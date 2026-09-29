//! Discovery: which Markdown files are tracked documents (plan §3).
//!
//! Every `README.md` is a document. Another selected Markdown file opts in
//! only through a recognized Memoria marker outside code. Links never opt a
//! file in.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::Json;

/// A committed project with a root README and `files`.
fn project_with(files: &[(&str, &[u8])]) -> Project {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n\nSee [the notes](docs/).\n");
    project.write("memoria.toml", "version = 3\n");
    for (path, body) in files {
        project.write(path, body);
    }
    project.commit_all("documents");
    project
}

fn documents(project: &Project) -> Vec<(String, String)> {
    let (_, graph) = project.json(&["graph"]);
    let Json::Array(nodes) = get(&graph, &["data", "nodes"]) else {
        panic!()
    };
    nodes
        .iter()
        .map(|n| {
            (
                get_str(n, &["document"]).to_string(),
                get_str(n, &["kind"]).to_string(),
            )
        })
        .collect()
}

fn is_document(project: &Project, path: &str) -> bool {
    documents(project).iter().any(|(doc, _)| doc == path)
}

const EXPORT: &str =
    "# Guide\n\n<!-- memoria:export id=\"summary\" -->\nThe summary.\n<!-- /memoria:export -->\n";

#[test]
fn every_line_shape_decides_opt_in_as_specified() {
    let cases: Vec<(&str, &str, bool)> = vec![
        ("column-zero export", EXPORT, true),
        (
            "column-zero import",
            "# G\n\n<!-- memoria:import src=\"../README.md#x\" -->\n<!-- /memoria:import -->\n",
            true,
        ),
        (
            "section",
            "# G\n\n<!-- memoria:section id=\"a\" files=\"a.rs\" -->\n## A\n\nText.\n<!-- /memoria:section -->\n",
            true,
        ),
        ("lonely closer", "# G\n\n<!-- /memoria:export -->\n", true),
        (
            "malformed marker",
            "# G\n\n<!-- memoria:exprt id=\"x\" -->\n",
            true,
        ),
        (
            "indented section-like comment",
            "# G\n\n   <!-- memoria:section broken -->\n",
            true,
        ),
        (
            "export indented two spaces",
            "# G\n\n  <!-- memoria:export id=\"x\" -->\n  <!-- /memoria:export -->\n",
            false,
        ),
        (
            "fenced code",
            "# G\n\n```\n<!-- memoria:export id=\"x\" -->\n<!-- /memoria:export -->\n```\n",
            false,
        ),
        (
            "indented code",
            "# G\n\n    <!-- memoria:export id=\"x\" -->\n    <!-- /memoria:export -->\n",
            false,
        ),
        (
            "inline code",
            "# G\n\n`<!-- memoria:export id=\"x\" -->`\n",
            false,
        ),
        (
            "list item",
            "# G\n\n- <!-- memoria:export id=\"x\" -->\n",
            false,
        ),
        (
            "blockquote",
            "# G\n\n> <!-- memoria:export id=\"x\" -->\n",
            false,
        ),
        (
            "table",
            "# G\n\n| <!-- memoria:export id=\"x\" --> |\n| --- |\n",
            false,
        ),
        ("plain comment", "# G\n\n<!-- a note -->\n", false),
        (
            "no marker text",
            "# G\n\nPlain prose about memoria: nothing more.\n",
            false,
        ),
    ];
    for (label, body, expected) in cases {
        let project = project_with(&[("docs/guide.md", body.as_bytes())]);
        assert_eq!(
            is_document(&project, "docs/guide.md"),
            expected,
            "{label}: {:?}",
            documents(&project)
        );
        if !expected {
            assert_eq!(project.covered_by("docs/guide.md"), "README.md", "{label}");
        }
    }
}

#[test]
fn a_malformed_marker_opts_in_and_fails_lint() {
    let project = project_with(&[("docs/guide.md", b"# G\n\n<!-- memoria:exprt id=\"x\" -->\n")]);
    assert!(is_document(&project, "docs/guide.md"));
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"marker_malformed".to_string()));
}

#[test]
fn links_never_opt_a_file_in() {
    let project = project_with(&[
        ("docs/notes.md", b"# Notes\n"),
        ("docs/guide.md", EXPORT.as_bytes()),
    ]);
    project.append(
        "README.md",
        "\nSee [notes](docs/notes.md) and [the guide](docs/guide.md).\n",
    );
    project.append("docs/guide.md", "\nSee [notes](notes.md).\n");
    assert!(!is_document(&project, "docs/notes.md"));
    assert!(is_document(&project, "docs/guide.md"));
    assert_eq!(project.covered_by("docs/notes.md"), "docs/guide.md");
}

#[test]
fn name_variants_and_extensions() {
    let project = project_with(&[
        ("a/readme.md", EXPORT.as_bytes()),
        ("b/README.MD", EXPORT.as_bytes()),
        ("c/Guide.Markdown", EXPORT.as_bytes()),
        ("d/plain.MD", b"# Plain\n"),
        ("e/notes.txt", EXPORT.as_bytes()),
    ]);
    let found = documents(&project);
    for (path, kind) in [
        ("a/readme.md", "opted_in"),
        ("b/README.MD", "opted_in"),
        ("c/Guide.Markdown", "opted_in"),
        ("README.md", "readme"),
    ] {
        assert!(
            found.contains(&(path.to_string(), kind.to_string())),
            "{path}: {found:?}"
        );
    }
    assert!(!is_document(&project, "d/plain.MD"));
    assert!(!is_document(&project, "e/notes.txt"));
}

#[test]
fn unselected_files_are_never_documents() {
    let project = project_with(&[("vendor/guide.md", EXPORT.as_bytes())]);
    project.write("memoria.toml", "version = 3\nignore = [\"vendor/**\"]\n");
    assert!(!is_document(&project, "vendor/guide.md"));
    // A Git-ignored untracked file is not eligible at all.
    let project = project_with(&[(".gitignore", b"scratch/\n")]);
    project.write("scratch/guide.md", EXPORT);
    assert!(!is_document(&project, "scratch/guide.md"));
}

#[test]
fn invalid_utf8_stays_a_source_with_a_warning() {
    let mut body = b"<!-- memoria:export id=\"x\" -->\n".to_vec();
    body.extend_from_slice(b"\xff\xfe\n<!-- /memoria:export -->\n");
    let project = project_with(&[("docs/broken.md", &body)]);
    assert!(!is_document(&project, "docs/broken.md"));
    let warnings = project.lint_diagnostics("document_encoding_invalid");
    assert_eq!(warnings.len(), 1);
    assert_eq!(get_str(&warnings[0], &["severity"]), "warning");
    // Invalid UTF-8 without a marker-like line gets no warning.
    let project = project_with(&[("docs/binary.md", b"\xff\xfe plain\n")]);
    assert!(
        project
            .lint_diagnostics("document_encoding_invalid")
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn a_selected_symlink_is_unsupported_not_a_document() {
    let project = project_with(&[("docs/real.txt", EXPORT.as_bytes())]);
    std::os::unix::fs::symlink("real.txt", project.root.join("docs/link.md")).unwrap();
    project.commit_all("symlink");
    let (code, lint) = project.json(&["lint"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&lint).contains(&"path_unsupported".to_string()));
    assert!(!is_document(&project, "docs/link.md"));
}

#[test]
fn nested_repositories_hold_no_documents() {
    let project = project_with(&[]);
    fs::create_dir_all(project.root.join("vendor/lib")).unwrap();
    project.git_in(&project.root.join("vendor/lib"), &["init", "-q"]);
    project.write("vendor/lib/guide.md", EXPORT);
    assert!(!is_document(&project, "vendor/lib/guide.md"));
}

#[test]
fn every_command_accepts_an_opted_in_document() {
    let project = project_with(&[("docs/guide.md", EXPORT.as_bytes())]);
    project.baseline();
    for args in [
        vec!["explain", "docs/guide.md"],
        vec!["guidance", "docs/guide.md"],
        vec!["render", "docs/guide.md"],
        vec!["status", "--explain", "docs/guide.md"],
    ] {
        let (code, value) = project.json(&args);
        assert_eq!(code, 0, "{args:?}: {value:?}");
    }
    let (_, explained) = project.json(&["status", "--explain", "docs/guide.md"]);
    assert_eq!(
        get_str(&explained, &["data", "explanation", "outcome"]),
        "document"
    );
    assert_eq!(
        get_str(&explained, &["data", "explanation", "document", "kind"]),
        "opted_in"
    );
    let (code, value) = project.json(&[
        "invalidate",
        "doc:docs/guide.md",
        "--reason",
        "the guide needs a semantic look",
    ]);
    assert_eq!(code, 0, "{value:?}");
    assert_eq!(
        project.pending(),
        vec!["docs/guide.md"],
        "{:?}",
        project.doc_status("README.md")
    );
    project.ack_ok("docs/guide.md");
    assert!(project.pending().is_empty());
    // A path that is not Markdown is not a document identity.
    let (code, value) = project.json(&["review", "src/main.rs"]);
    assert_eq!(code, 2);
    assert_eq!(diagnostic_codes(&value), vec!["document_invalid"]);
    // An unmarked Markdown source says who covers it and how to track it.
    project.write("docs/notes.md", "# Notes\n");
    let (code, value) = project.json(&["review", "docs/notes.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&value), vec!["document_not_found"]);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert_eq!(
        strings(get(&diagnostics[0], &["details", "covered_by"])),
        vec!["README.md", "docs/guide.md"]
    );
    assert!(
        get_str(&diagnostics[0], &["message"]).contains("add an import, export, or section marker")
    );
}
